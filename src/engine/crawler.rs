#![allow(dead_code)]

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use spider::website::Website;

use super::device_profile::SessionManager;
use super::embedder::Embedder;
use super::fetcher::Fetcher;
use super::indexer::Indexer;
use super::proxy_pool::{ProxyPool, ProxyProtocol};
use super::vector::VectorEngine;
use crate::schema::content::StructuredContent;
use crate::storage::cache_store::{CacheEntry, CacheStore, ReCrawlPolicy, SimHash};

/// Crawler: spider for discovery + Fetcher for extraction + Indexer for storage.
pub struct Crawler {
    seed_url: String,
    max_pages: u32,
    delay_ms: u32,
    respect_robots: bool,
    subdomains: bool,
    cache: Option<CacheStore>,
    recrawl_policy: ReCrawlPolicy,
    skip_cached: bool,
    proxy_list: Vec<String>,
    proxy_cidr: Option<String>,
    proxy_protocol: ProxyProtocol,
    rotate_ua: bool,
    sticky_sessions: bool,
    rps: u32,
    dynamic_fallback: bool,
    dynamic_wait_ms: u64,
    embedder: Option<Arc<dyn Embedder>>,
}

/// Statistics from a crawl run.
#[derive(Debug, Default)]
pub struct CrawlStats {
    pub pages_discovered: usize,
    pub pages_fetched: usize,
    pub pages_skipped_cache: usize,
    pub pages_skipped_dup: usize,
    pub pages_indexed: usize,
    pub fetch_errors: usize,
    pub index_errors: usize,
}

impl Crawler {
    pub fn new(seed_url: &str, max_pages: u32, delay_ms: u32) -> Self {
        Self {
            seed_url: seed_url.to_string(),
            max_pages,
            delay_ms,
            respect_robots: true,
            subdomains: false,
            cache: None,
            recrawl_policy: ReCrawlPolicy::default(),
            skip_cached: false,
            proxy_list: vec![],
            proxy_cidr: None,
            proxy_protocol: ProxyProtocol::Http,
            rotate_ua: false,
            sticky_sessions: true,
            rps: 1,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            embedder: None,
        }
    }

    pub fn with_robots_txt(mut self, respect: bool) -> Self {
        self.respect_robots = respect;
        self
    }

    pub fn with_subdomains(mut self, allow: bool) -> Self {
        self.subdomains = allow;
        self
    }

    /// Configure proxy pool and User-Agent rotation.
    ///
    /// * `proxy_list` — explicit proxy URLs (http://, https://, socks5://).
    /// * `proxy_cidr` — generate random proxy endpoints inside this CIDR (e.g. "10.0.0.0/24").
    /// * `proxy_protocol` — protocol for generated proxies.
    /// * `rotate_ua` — generate random User-Agent per request.
    /// * `sticky_sessions` — keep same UA+proxy for a given domain.
    /// * `rps` — per-domain requests per second.
    pub fn with_human_mode(
        mut self,
        proxy_list: Vec<String>,
        proxy_cidr: Option<String>,
        proxy_protocol: ProxyProtocol,
        rotate_ua: bool,
        sticky_sessions: bool,
        rps: u32,
    ) -> Self {
        self.proxy_list = proxy_list;
        self.proxy_cidr = proxy_cidr;
        self.proxy_protocol = proxy_protocol;
        self.rotate_ua = rotate_ua;
        self.sticky_sessions = sticky_sessions;
        self.rps = rps.max(1);
        self
    }

    /// Enable Chromium-based dynamic rendering fallback.
    pub fn with_dynamic_fallback(mut self, wait_ms: u64) -> Self {
        self.dynamic_fallback = true;
        self.dynamic_wait_ms = wait_ms.max(500);
        self
    }

    /// Attach a dense-vector embedder to enable hybrid indexing.
    pub fn with_embedder(mut self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        self.embedder = embedder;
        self
    }

    /// Enable URL cache with a re-crawl policy.
    ///
    /// `skip_cached=true` means URLs that are still fresh in the cache will not
    /// be extracted or indexed again (saves CPU/indexing work; note that the
    /// spider may still fetch the page depending on its own scheduler).
    pub fn with_cache(
        mut self,
        base: impl AsRef<Path>,
        policy: ReCrawlPolicy,
        skip_cached: bool,
    ) -> Result<Self> {
        self.cache = Some(CacheStore::open(base)?.with_policy(policy));
        self.recrawl_policy = policy;
        self.skip_cached = skip_cached;
        Ok(self)
    }

    /// Run the full crawl → fetch → index pipeline.
    pub async fn crawl_and_index(&self, index_dir: &Path) -> Result<CrawlStats> {
        let mut stats = CrawlStats::default();

        // 1. Build spider
        let mut website = Website::new(&self.seed_url);
        website
            .with_limit(self.max_pages)
            .with_delay(self.delay_ms as u64)
            .with_respect_robots_txt(self.respect_robots)
            .with_subdomains(self.subdomains)
            .with_tld(false);

        // Subscribe to receive crawled pages
        let mut rx = website.subscribe(100);

        // 2. Start crawling in background
        let crawl_handle = tokio::spawn(async move {
            website.crawl().await;
            website.unsubscribe();
        });

        // 3. Build fetcher + indexer
        let proxy_pool = if let Some(ref cidr) = self.proxy_cidr {
            Some(ProxyPool::from_cidr(
                cidr,
                8080,
                self.proxy_protocol.clone(),
                10,
            )?)
        } else if !self.proxy_list.is_empty() {
            Some(ProxyPool::from_list(&self.proxy_list)?)
        } else {
            None
        };
        let session_manager = if self.rotate_ua || proxy_pool.is_some() {
            Some(SessionManager::new(self.sticky_sessions))
        } else {
            None
        };
        let fetcher = if proxy_pool.is_some() || self.rotate_ua {
            Fetcher::new_human(proxy_pool, session_manager, self.rotate_ua, self.rps)
                .context("failed to create human-like fetcher")?
                .with_dynamic_fallback(self.dynamic_wait_ms)
        } else if self.dynamic_fallback {
            Fetcher::new()
                .context("failed to create fetcher")?
                .with_dynamic_fallback(self.dynamic_wait_ms)
        } else {
            Fetcher::new().context("failed to create fetcher")?
        };
        let mut indexer = Indexer::open_at(index_dir).context("failed to open/create index")?;
        if let Some(embedder) = &self.embedder {
            indexer = indexer.with_vector_engine(VectorEngine::new(embedder.clone()));
        }

        // 4. Process pages as they come in
        let batch_size = 50;
        let mut batch: Vec<StructuredContent> = Vec::new();

        while let Ok(page) = rx.recv().await {
            stats.pages_discovered += 1;

            let url = page.get_url().to_string();
            let html = page.get_content().to_string();
            let status = page.status_code.as_u16();

            // Skip non-200 pages
            if status != 200 || html.is_empty() {
                stats.fetch_errors += 1;
                continue;
            }

            // Cache-aware extraction: skip recently-cached URLs when requested.
            if self.skip_cached {
                if let Some(ref cache) = self.cache {
                    if !cache.should_fetch(&url) {
                        stats.pages_skipped_cache += 1;
                        continue;
                    }
                }
            }

            // Extract structured content from the HTML we already have
            match fetcher.extract_from_html(
                &html,
                &url,
                &url,
                status,
                url.starts_with("https://"),
                0, // already fetched by spider
                None,
            ) {
                Ok(content) => {
                    stats.pages_fetched += 1;

                    // Skip pages with no meaningful content
                    if !content.is_valid_content {
                        continue;
                    }

                    // Near-duplicate detection against previously cached entries.
                    if let Some(ref cache) = self.cache {
                        if let Some(existing) = cache.get(&url) {
                            if let (Some(old), Some(new)) = (
                                existing.simhash,
                                Some(SimHash::compute(&content.content_text)),
                            ) {
                                if SimHash::hamming_distance(old, new) <= 15 {
                                    stats.pages_skipped_dup += 1;
                                    continue;
                                }
                            }
                        }
                    }

                    batch.push(content);

                    // Index in batches
                    if batch.len() >= batch_size {
                        let batch_count = batch.len();
                        match indexer.index_batch(&batch) {
                            Ok(indexed) => {
                                stats.pages_indexed += indexed as usize;
                                self.persist_cache_entries(&batch);
                                eprintln!(
                                    "  indexed {} pages (total: {})",
                                    indexed, stats.pages_indexed
                                );
                            }
                            Err(e) => {
                                stats.index_errors += batch_count;
                                eprintln!("  index error: {}", e);
                            }
                        }
                        batch.clear();
                    }
                }
                Err(e) => {
                    stats.fetch_errors += 1;
                    eprintln!("  fetch error [{}]: {}", url, e);
                }
            }
        }

        // Index remaining batch
        if !batch.is_empty() {
            let batch_count = batch.len();
            match indexer.index_batch(&batch) {
                Ok(indexed) => {
                    stats.pages_indexed += indexed as usize;
                    self.persist_cache_entries(&batch);
                }
                Err(e) => {
                    stats.index_errors += batch_count;
                    eprintln!("  final batch index error: {}", e);
                }
            }
        }

        // Wait for crawl to finish
        let _ = crawl_handle.await;

        Ok(stats)
    }

    /// Simple crawl-and-fetch (no indexing) — returns extracted content.
    pub async fn crawl_only(&self) -> Result<Vec<StructuredContent>> {
        let mut website = Website::new(&self.seed_url);
        website
            .with_limit(self.max_pages)
            .with_delay(self.delay_ms as u64)
            .with_respect_robots_txt(self.respect_robots)
            .with_subdomains(self.subdomains)
            .with_tld(false);

        let mut rx = website.subscribe(100);

        let crawl_handle = tokio::spawn(async move {
            website.crawl().await;
            website.unsubscribe();
        });

        let proxy_pool = if let Some(ref cidr) = self.proxy_cidr {
            Some(ProxyPool::from_cidr(
                cidr,
                8080,
                self.proxy_protocol.clone(),
                10,
            )?)
        } else if !self.proxy_list.is_empty() {
            Some(ProxyPool::from_list(&self.proxy_list)?)
        } else {
            None
        };
        let session_manager = if self.rotate_ua || proxy_pool.is_some() {
            Some(SessionManager::new(self.sticky_sessions))
        } else {
            None
        };
        let fetcher = if proxy_pool.is_some() || self.rotate_ua {
            Fetcher::new_human(proxy_pool, session_manager, self.rotate_ua, self.rps)
                .context("failed to create human-like fetcher")?
                .with_dynamic_fallback(self.dynamic_wait_ms)
        } else if self.dynamic_fallback {
            Fetcher::new()
                .context("failed to create fetcher")?
                .with_dynamic_fallback(self.dynamic_wait_ms)
        } else {
            Fetcher::new().context("failed to create fetcher")?
        };
        let mut results = vec![];

        while let Ok(page) = rx.recv().await {
            let url = page.get_url().to_string();
            let html = page.get_content().to_string();
            let status = page.status_code.as_u16();

            if status != 200 || html.is_empty() {
                continue;
            }

            if let Some(ref cache) = self.cache {
                if self.skip_cached && !cache.should_fetch(&url) {
                    continue;
                }
            }

            if let Ok(content) = fetcher.extract_from_html(
                &html,
                &url,
                &url,
                status,
                url.starts_with("https://"),
                0,
                None,
            ) {
                if content.is_valid_content {
                    results.push(content);
                }
            }
        }

        let _ = crawl_handle.await;
        Ok(results)
    }

    fn persist_cache_entries(&self, batch: &[StructuredContent]) {
        if let Some(ref cache) = self.cache {
            for content in batch {
                let entry = CacheEntry {
                    url: content.url.clone(),
                    content_hash: CacheEntry::hash_content(&content.content_text),
                    first_seen: content.fetched_at,
                    last_fetched: content.fetched_at,
                    fetch_count: 1,
                    last_status: content.status_code,
                    title: Some(content.title.clone()).filter(|t| !t.is_empty()),
                    simhash: Some(SimHash::compute(&content.content_text)),
                };
                if let Err(e) = cache.insert(entry) {
                    eprintln!("  cache write error: {}", e);
                }
            }
        }
    }
}
