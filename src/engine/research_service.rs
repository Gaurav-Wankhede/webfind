use std::collections::HashSet;
use std::sync::Arc;

use anyhow::Result;
use tokio::sync::RwLock;

use super::bg_worker::BackgroundWorker;
use super::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use super::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
use super::discovery;
use super::embedder::{Embedder, FastembedEmbedder};
use super::proxy_pool::{ProxyEndpoint, ProxyPool};
use super::search_engine::SearchEngine;
use crate::schema::content::StructuredContent;

/// Progress update emitted during a research job.
#[derive(Debug, Clone)]
pub struct ResearchProgress {
    pub stage: &'static str,
    pub crawled: usize,
    pub total: usize,
    pub current_url: Option<String>,
}

/// Options for executing a research crawl.
#[derive(Debug, Clone)]
pub struct ResearchOptions {
    pub query: String,
    pub seed: Option<String>,
    pub seeds: Option<String>,
    pub max_pages: u32,
    pub delay_ms: u32,
    pub follow_external: bool,
    pub min_depth: u32,
    pub max_depth: u32,
    /// Auto-map crawl depth from the site's own map (sitemap/llms.txt) and
    /// bound link-following to the site's declared content surface.
    pub auto_depth: bool,
    pub topics: Option<String>,
    pub proxies: Option<String>,
    /// Domain filter: only return results whose domain matches one of these.
    pub domain_filter: Vec<String>,
    /// Render JS-heavy / bot-protected pages in headless Chromium (CDP).
    pub dynamic: bool,
    /// Deep research: no crawl deadline / backoff caps + CDP stealth + scroll.
    pub deep: bool,
}

/// Errors that can occur during research.
#[derive(Debug, thiserror::Error)]
pub enum ResearchError {
    #[error("failed to initialize embedder: {0}")]
    Embedder(anyhow::Error),
    #[error("no seeds discovered for query")]
    NoSeeds,
    #[error("crawl failed: {0}")]
    Crawl(#[from] anyhow::Error),
}

/// Run the core research crawl and return the fetched content plus the graph store
/// that was used to record discovered URLs and links.
pub async fn execute_research<F>(
    indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    embedder: Option<Arc<dyn Embedder + Send + Sync>>,
    options: ResearchOptions,
    progress: F,
) -> Result<
    (
        Vec<StructuredContent>,
        Arc<dyn CrawlGraphStore + Send + Sync>,
    ),
    ResearchError,
>
where
    F: Fn(ResearchProgress) + Send + Sync + 'static,
{
    progress(ResearchProgress {
        stage: "discovering",
        crawled: 0,
        total: 0,
        current_url: None,
    });

    // Use provided embedder or create one for KG + vector persistence.
    // When the real ONNX model is unavailable (first run, offline, CI), degrade
    // to the deterministic dummy embedder instead of failing the whole research
    // job. Production deployments with a loaded model keep real embeddings.
    let embedder: Arc<dyn Embedder> = match embedder {
        Some(e) => e,
        None => match FastembedEmbedder::new() {
            Ok(e) => Arc::new(e),
            Err(e) => {
                tracing::warn!("fastembed model unavailable, using dummy embedder: {}", e);
                Arc::new(crate::engine::embedder::DummyEmbedder)
            }
        },
    };

    let graph: Arc<dyn CrawlGraphStore + Send + Sync> = graph_store
        .clone()
        .unwrap_or_else(|| Arc::new(InMemoryCrawlGraph::new()));

    let background_worker = Arc::new(BackgroundWorker::new(
        graph.clone(),
        Some(embedder.clone()),
        256,
    ));

    let topics: Vec<String> = options
        .topics
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // Determine seeds: explicit seed, additional seeds, or auto-discovery.
    let auto_discover = options.seed.is_none();
    let mut all_seeds: Vec<String> = Vec::new();
    if let Some(seed) = &options.seed {
        all_seeds.push(seed.trim().to_string());
    }
    if let Some(seeds_str) = &options.seeds {
        all_seeds.extend(
            seeds_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }

    if all_seeds.is_empty() {
        let indexer_guard = indexer.read().await;
        let discovered =
            discovery::discover_seeds(indexer_guard.as_ref(), Some(graph.as_ref()), &options.query)
                .await;
        drop(indexer_guard);

        match discovered {
            Ok(seeds) if !seeds.is_empty() => all_seeds = seeds,
            Ok(_) => return Err(ResearchError::NoSeeds),
            Err(e) => return Err(ResearchError::Crawl(e)),
        }
    }

    let follow_external = if auto_discover {
        true
    } else {
        options.follow_external
    };

    let proxy_pool = ProxyPool::new();
    if let Some(proxies_str) = &options.proxies {
        for url in proxies_str.split(',') {
            let url = url.trim();
            if url.is_empty() {
                continue;
            }
            if let Ok(ep) = ProxyEndpoint::from_url(url) {
                let _ = proxy_pool.add(ep);
            }
        }
    }

    let crawler = BulkDomainCrawler::new(
        proxy_pool,
        100,
        30,
        options.delay_ms.max(100) as u64,
        1,
        options.max_pages.clamp(1, 500) as usize,
        if follow_external {
            FollowExternalLinks::Follow
        } else {
            FollowExternalLinks::Ignore
        },
        options.min_depth,
        options.max_depth,
    )
    .with_respect_robots(RespectRobots::Yes)
    .with_auto_depth(options.auto_depth)
    .with_graph_store(graph.clone())
    .with_background_worker(background_worker.clone())
    .with_topics(topics)
    .with_dynamic(options.dynamic)
    .with_deep(options.deep);

    let max_pages = options.max_pages.clamp(1, 500) as usize;
    let total_estimate = max_pages;

    // Multi-seed crawl with periodic progress updates. The total page budget
    // is split across seeds so `--max-pages N` means N pages total, not N per
    // seed (auto-discovery can return up to 5 seeds).
    let mut contents: Vec<StructuredContent> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut remaining = max_pages;

    for seed_url in &all_seeds {
        if remaining == 0 {
            break;
        }
        progress(ResearchProgress {
            stage: "crawling",
            crawled: contents.len(),
            total: total_estimate,
            current_url: Some(seed_url.clone()),
        });

        let crawled = crawler
            .crawl_with_limit(seed_url, remaining)
            .await
            .map_err(ResearchError::Crawl)?;
        for c in crawled {
            if seen_urls.insert(c.url.clone()) {
                contents.push(c);
                remaining = remaining.saturating_sub(1);
                progress(ResearchProgress {
                    stage: "crawling",
                    crawled: contents.len(),
                    total: total_estimate,
                    current_url: Some(seed_url.clone()),
                });
            }
        }
    }

    progress(ResearchProgress {
        stage: "persisting",
        crawled: contents.len(),
        total: total_estimate,
        current_url: None,
    });

    // Wait for background knowledge-graph + vector persistence to drain.
    background_worker.close().await;

    progress(ResearchProgress {
        stage: "complete",
        crawled: contents.len(),
        total: total_estimate,
        current_url: None,
    });

    // Apply domain filter if specified (post-crawl filtering).
    let filtered_contents = if options.domain_filter.is_empty() {
        contents
    } else {
        let filter: std::collections::HashSet<String> = options
            .domain_filter
            .iter()
            .map(|d| d.to_lowercase())
            .collect();
        contents
            .into_iter()
            .filter(|c| {
                url::Url::parse(&c.url)
                    .ok()
                    .and_then(|u| u.host_str().map(|h| h.to_lowercase()))
                    .map(|h| filter.contains(&h))
                    .unwrap_or(false)
            })
            .collect()
    };

    // Return the contents and the graph store used.
    Ok((filtered_contents, graph))
}
