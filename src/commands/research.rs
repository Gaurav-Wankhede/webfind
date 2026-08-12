use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;

use webfind::engine::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::graph_summary::build_graph_summary;
use webfind::engine::indexer::{attach_content, build_metadata};
use webfind::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::report::format_response;
use webfind::schema::content::StructuredContent;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::SearchResponse;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    seed: Option<String>,
    query: String,
    max_pages: u32,
    delay: u32,
    respect_robots: bool,
    proxies: Option<String>,
    hybrid: bool,
    limit: u32,
    include_graph: bool,
    include_content: bool,
    follow_external: bool,
    min_depth: u32,
    max_depth: u32,
    topics: Option<String>,
    seeds: Option<String>,
) -> anyhow::Result<()> {
    let proxy_list: Vec<String> = proxies
        .as_ref()
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let proxy_pool = ProxyPool::new();
    for url in &proxy_list {
        proxy_pool.add(ProxyEndpoint::from_url(url)?);
    }

    let embedder: Option<Arc<dyn Embedder>> = if hybrid {
        Some(Arc::new(
            FastembedEmbedder::new().context("load embedding model for hybrid research")?,
        ))
    } else {
        None
    };

    let graph: Arc<dyn CrawlGraphStore + Send + Sync> = Arc::new(InMemoryCrawlGraph::new());
    let topics: Vec<String> = topics
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_lowercase())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let auto_discover = seed.is_none();
    let mut all_seeds: Vec<String> = Vec::new();
    if let Some(ref seed) = seed {
        all_seeds.push(seed.clone());
    }
    if let Some(ref seeds_str) = seeds {
        all_seeds.extend(
            seeds_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }

    if all_seeds.is_empty() {
        let temp_engine = Arc::new(InMemorySearchEngine::new());
        let temp_indexer = temp_engine;
        let discovered = webfind::engine::discovery::discover_seeds(
            &*temp_indexer,
            Some(graph.as_ref()),
            &query,
        )
        .await?;
        if discovered.is_empty() {
            anyhow::bail!(
                "No seeds discovered for the query. Provide a seed URL or rephrase the query."
            );
        }
        all_seeds = discovered;
    }

    let follow_external = if auto_discover { true } else { follow_external };

    let crawler = BulkDomainCrawler::new(
        proxy_pool,
        100,
        30,
        delay as u64,
        1,
        max_pages as usize,
        if follow_external {
            FollowExternalLinks::Follow
        } else {
            FollowExternalLinks::Ignore
        },
        min_depth,
        max_depth,
    )
    .with_respect_robots(if respect_robots {
        RespectRobots::Yes
    } else {
        RespectRobots::No
    })
    .with_graph_store(graph.clone())
    .with_topics(topics);

    println!("Researching: {}", all_seeds.join(", "));
    println!("Query: {}", query);
    let crawl_start = Instant::now();
    let mut contents: Vec<StructuredContent> = Vec::new();
    let mut seen_urls: HashSet<String> = HashSet::new();
    for seed_url in &all_seeds {
        let crawled = crawler.crawl(seed_url).await?;
        for c in crawled {
            if seen_urls.insert(c.url.clone()) {
                contents.push(c);
            }
        }
    }
    println!(
        "Crawl + index complete in {:.2}s ({} pages)",
        crawl_start.elapsed().as_secs_f64(),
        contents.len()
    );

    let indexer = Arc::new(
        embedder
            .as_ref()
            .map(|e| InMemorySearchEngine::with_embedder(e.clone()))
            .unwrap_or_else(InMemorySearchEngine::new),
    );
    indexer.index_batch(&contents).await?;

    let mut results = indexer.search_bm25(&query, limit as usize).await?;

    let request = SearchRequest {
        query: query.clone(),
        depth: SearchDepth::Standard,
        limit,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: None,
        include_content,
        include_graph,
        include_keywords: false,
        include_metrics: false,
        hybrid,
    };

    let vector_scores: Option<HashMap<String, f64>> = if hybrid {
        Some(indexer.search_vector(&query, limit as usize).await?)
    } else {
        None
    };

    let ranker = Ranker::new();
    results = ranker.rank(results, &request, None, vector_scores.as_ref());

    if include_content {
        attach_content(&mut results, &contents);
    }

    let graph_summary = if include_graph {
        build_graph_summary(graph.as_ref(), &results).await
    } else {
        None
    };

    let mut signals = vec!["bm25".to_string()];
    if hybrid {
        signals.push("vector".to_string());
    }
    let meta = build_metadata(&*indexer, signals).await?;

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query,
        depth: SearchDepth::Standard,
        total_results: results.len() as u64,
        returned: results.len() as u32,
        latency_ms: crawl_start.elapsed().as_millis() as u64,
        results,
        suggestions: vec![],
        related: vec![],
        graph: graph_summary,
        metadata: meta,
    };

    let body = format_response(&response, &OutputFormat::Json);
    print!("{}", body);
    Ok(())
}
