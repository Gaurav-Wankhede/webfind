//! Background curation daemon.
//!
//! Runs a continuous, low-priority crawl loop over the curated seed catalog
//! ([`crate::engine::seed_catalog`]), persisting fresh content into the
//! knowledge graph + vector store so the local index has persistent, topic-
//! aware coverage that stays fresh over time — without filling the disk (the
//! storage budget handles retention).
//!
//! Each source is re-crawled on its configured cadence (daily / weekly /
//! monthly), so churn-heavy domains (finance, news) refresh more often than
//! stable primary sources (standards bodies, language docs). The daemon is
//! polite: it respects robots.txt, rate-limits per domain, and pauses between
//! cycles so it runs on idle bandwidth/CPU.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::Mutex;

use super::bg_worker::BackgroundWorker;
use super::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use super::crawl_graph::CrawlGraphStore;
use super::embedder::Embedder;
use super::proxy_pool::ProxyPool;
use super::seed_catalog::{CuratedDomain, Recrawl};
use crate::schema::content::StructuredContent;

/// Configuration for the background curation daemon.
#[derive(Debug, Clone)]
pub struct DaemonConfig {
    /// Delay (ms) between requests within a crawl.
    pub delay_ms: u64,
    /// Maximum pages to fetch per source per cycle.
    pub max_pages_per_source: usize,
    /// Seconds to sleep between full catalog sweeps.
    pub sweep_interval_secs: u64,
    /// Optional filter of domain slugs to crawl (empty = all curated domains).
    pub only_domains: Vec<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            delay_ms: 1000,
            max_pages_per_source: 50,
            sweep_interval_secs: 3600,
            only_domains: Vec::new(),
        }
    }
}

/// Statistics from one catalog sweep.
#[derive(Debug, Default)]
pub struct SweepStats {
    pub sources_crawled: usize,
    pub pages_fetched: usize,
    pub pages_indexed: usize,
    pub errors: usize,
}

/// The background curation daemon.
pub struct CurationDaemon {
    config: DaemonConfig,
    graph_store: Arc<dyn CrawlGraphStore + Send + Sync>,
    background_worker: Arc<BackgroundWorker>,
    embedder: Option<Arc<dyn Embedder>>,
    /// Last crawl time per source URL, used to honor recrawl cadence.
    last_crawl: Mutex<std::collections::HashMap<String, Instant>>,
}

impl CurationDaemon {
    pub fn new(
        config: DaemonConfig,
        graph_store: Arc<dyn CrawlGraphStore + Send + Sync>,
        background_worker: Arc<BackgroundWorker>,
        embedder: Option<Arc<dyn Embedder>>,
    ) -> Self {
        Self {
            config,
            graph_store,
            background_worker,
            embedder,
            last_crawl: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn embedder(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.clone()
    }

    /// Run the daemon forever: repeatedly sweep the curated catalog, honoring
    /// each source's recrawl cadence and pausing between sweeps.
    pub async fn run_forever(&self) -> Result<()> {
        tracing::info!(
            "curation daemon started: {} domains, sweep every {}s",
            self.active_domains().len(),
            self.config.sweep_interval_secs
        );
        loop {
            let stats = self.sweep().await;
            tracing::info!(
                "curation sweep done: {} sources, {} pages fetched, {} indexed, {} errors",
                stats.sources_crawled,
                stats.pages_fetched,
                stats.pages_indexed,
                stats.errors
            );
            tokio::time::sleep(Duration::from_secs(self.config.sweep_interval_secs)).await;
        }
    }

    /// Run a single sweep over all due sources. Returns aggregated stats.
    pub async fn sweep(&self) -> SweepStats {
        let mut stats = SweepStats::default();
        for domain in self.active_domains() {
            for source in domain.sources {
                if !self.is_due(source.url).await {
                    continue;
                }
                stats.sources_crawled += 1;
                match self.crawl_source(domain, source.url).await {
                    Ok((fetched, indexed)) => {
                        stats.pages_fetched += fetched;
                        stats.pages_indexed += indexed;
                    }
                    Err(e) => {
                        tracing::warn!("daemon crawl of {} failed: {}", source.url, e);
                        stats.errors += 1;
                    }
                }
                self.mark_crawled(source.url).await;
            }
        }
        stats
    }

    /// Crawl a single curated source with the domain's topic prioritization.
    async fn crawl_source(
        &self,
        domain: &'static CuratedDomain,
        url: &str,
    ) -> Result<(usize, usize)> {
        let proxy_pool = ProxyPool::new();
        let crawler = BulkDomainCrawler::new(
            proxy_pool,
            30,
            30,
            self.config.delay_ms.max(100),
            1,
            self.config.max_pages_per_source.clamp(1, 500),
            FollowExternalLinks::Ignore,
            0,
            2,
        )
        .with_respect_robots(RespectRobots::Yes)
        .with_graph_store(self.graph_store.clone())
        .with_background_worker(self.background_worker.clone())
        .with_topics(domain.topics.iter().map(|s| s.to_string()).collect());

        let contents: Vec<StructuredContent> = crawler.crawl(url).await?;
        let indexed = contents.iter().filter(|c| c.is_valid_content).count();
        Ok((contents.len(), indexed))
    }

    /// Domains the daemon should crawl, filtered by config.
    fn active_domains(&self) -> Vec<&'static CuratedDomain> {
        if self.config.only_domains.is_empty() {
            return super::seed_catalog::DOMAINS.iter().collect();
        }
        super::seed_catalog::DOMAINS
            .iter()
            .filter(|d| self.config.only_domains.contains(&d.slug.to_string()))
            .collect()
    }

    async fn is_due(&self, url: &str) -> bool {
        let last = self.last_crawl.lock().await.get(url).copied();
        let recrawl = super::seed_catalog::all_source_urls()
            .iter()
            .find(|(u, _)| *u == url)
            .map(|(_, r)| *r)
            .unwrap_or(Recrawl::Weekly);
        match last {
            None => true,
            Some(t) => t.elapsed() >= recrawl.interval(),
        }
    }

    async fn mark_crawled(&self, url: &str) {
        self.last_crawl
            .lock()
            .await
            .insert(url.to_string(), Instant::now());
    }
}
