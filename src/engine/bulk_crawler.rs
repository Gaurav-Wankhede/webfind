use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rand::Rng;
use tokio::sync::Mutex;
use tokio::time::sleep;
use tracing::{debug, info, warn};

use super::bg_worker::BackgroundWorker;
use super::crawl_graph::{CrawlGraphStore, DiscoverySource, InMemoryCrawlGraph, UrlNode};
use super::device_profile::{DeviceProfile, SessionManager};
use super::fetcher::Fetcher;
use super::proxy_pool::ProxyPool;
use super::site_policy::{RobotsPolicy, SiteExplorer};
use super::util;
use crate::schema::content::StructuredContent;

/// Score relevance of a URL + its content against query topics.
/// Returns 0.0..=1.0 where 1.0 = perfect topic match.
fn score_relevance(url: &str, content: Option<&StructuredContent>, topics: &[String]) -> f32 {
    if topics.is_empty() {
        return 0.5; // neutral when no topics specified
    }

    let topic_set: HashSet<&str> = topics.iter().map(|s| s.as_str()).collect();

    // URL path signal: how many topic words appear in the URL.
    let url_lower = url.to_lowercase();
    let url_hits = topic_set.iter().filter(|t| url_lower.contains(*t)).count();
    let url_score = (url_hits as f32 / topics.len() as f32).min(1.0);

    // Content signal: title + excerpt + keywords.
    let content_score = if let Some(c) = content {
        let title_lower = c.title.to_lowercase();
        let excerpt_lower = c.excerpt.to_lowercase();
        let kw_lower: HashSet<String> = c.keywords.iter().map(|k| k.text.to_lowercase()).collect();

        let mut hits = 0;
        let mut total = 0;
        for t in &topic_set {
            total += 1;
            if title_lower.contains(t)
                || excerpt_lower.contains(t)
                || kw_lower.contains(*t)
            {
                hits += 1;
            }
        }
        if total > 0 {
            hits as f32 / total as f32
        } else {
            0.0
        }
    } else {
        0.0
    };

    // Combine: 40% URL signal, 60% content signal.
    (0.4 * url_score + 0.6 * content_score).clamp(0.0, 1.0)
}

/// A crawling session binds one egress IP + device fingerprint + cookie jar together.
/// It is rotated after `max_requests` or `max_age` to avoid burning out one identity.
#[derive(Debug)]
pub struct CrawlSession {
    pub id: String,
    pub domain: String,
    pub proxy_url: Option<String>,
    pub profile: DeviceProfile,
    pub created_at: Instant,
    pub request_count: usize,
    pub max_requests: usize,
    pub max_age: Duration,
    pub consecutive_failures: usize,
}

impl CrawlSession {
    pub fn should_rotate(&self) -> bool {
        self.request_count >= self.max_requests
            || self.created_at.elapsed() >= self.max_age
            || self.consecutive_failures >= 3
    }
}

/// Per-domain crawl state.
#[derive(Debug)]
struct DomainState {
    /// Queue entries: (url, depth, relevance_score).
    /// Relevance score drives topic-aware best-first scheduling.
    queue: VecDeque<(String, u32, f32)>,
    seen: HashSet<String>,
    last_request: Option<Instant>,
    backoff_until: Option<Instant>,
    success_count: usize,
    failure_count: usize,
    session: Option<CrawlSession>,
    policy: Option<RobotsPolicy>,
}

impl DomainState {
    fn new() -> Self {
        Self {
            queue: VecDeque::new(),
            seen: HashSet::new(),
            last_request: None,
            backoff_until: None,
            success_count: 0,
            failure_count: 0,
            session: None,
            policy: None,
        }
    }
}

/// Bulk domain crawler: fetches many pages from the same domain using rotating
/// sessions while keeping IP + UA + cookies consistent within each session.
///
/// Structural awareness:
/// - Fetches `/robots.txt` and `/sitemap.xml` in parallel before crawling.
/// - Seeds the queue from sitemap entries sorted by priority.
/// - Respects robots.txt `Disallow` and `Crawl-delay` directives.
/// - Records discovered URLs and links into an optional `CrawlGraphStore`
///   so a SurrealDB graph-vector backend can run PageRank / vector retrieval.
///
/// Google-inspired 3-phase crawl:
/// 1. DISCOVER: BFS through link graph (fast, collects URLs with depth)
/// 2. PRIORITIZE: Score URLs by depth + link count
/// 3. FETCH: Budget-constrained, per-host rate-limited fetch of top-scored URLs
pub struct BulkDomainCrawler {
    proxy_pool: ProxyPool,
    session_manager: SessionManager,
    domains: Mutex<HashMap<String, DomainState>>,
    pages_per_session: usize,
    session_max_age: Duration,
    delay_ms: u64,
    rps: f64,
    max_pages: usize,
    respect_robots: bool,
    graph_store: Option<Arc<dyn CrawlGraphStore>>,
    background_worker: Option<Arc<BackgroundWorker>>,
    follow_external: bool,
    min_depth: u32,
    max_depth: u32,
    /// Query topics for content-aware link prioritization.
    topics: Vec<String>,
}

impl BulkDomainCrawler {
    pub fn new(
        proxy_pool: ProxyPool,
        pages_per_session: usize,
        session_max_age_minutes: u64,
        delay_ms: u64,
        rps: u32,
        max_pages: usize,
        follow_external: bool,
        min_depth: u32,
        max_depth: u32,
    ) -> Self {
        Self {
            proxy_pool,
            session_manager: SessionManager::new(true),
            domains: Mutex::new(HashMap::new()),
            pages_per_session: pages_per_session.max(1),
            session_max_age: Duration::from_secs(session_max_age_minutes.max(1) * 60),
            delay_ms: delay_ms.max(100),
            rps: (rps.max(1) as f64),
            max_pages,
            respect_robots: true,
            graph_store: None,
            background_worker: None,
            follow_external,
            min_depth,
            max_depth,
            topics: Vec::new(),
        }
    }

    pub fn with_respect_robots(mut self, respect: bool) -> Self {
        self.respect_robots = respect;
        self
    }

    /// Attach a graph store (e.g. SurrealDB) for vector + link analysis.
    pub fn with_graph_store(mut self, store: Arc<dyn CrawlGraphStore>) -> Self {
        self.graph_store = Some(store);
        self
    }

    /// Attach a background worker to persist full fetched content + embeddings
    /// into the knowledge graph without blocking the crawl.
    pub fn with_background_worker(mut self, worker: Arc<BackgroundWorker>) -> Self {
        self.background_worker = Some(worker);
        self
    }

    /// Use the default in-memory graph store for local crawls.
    pub fn with_memory_graph(mut self) -> Self {
        self.graph_store = Some(Arc::new(InMemoryCrawlGraph::new()));
        self
    }

    /// Set query topics for content-aware link prioritization.
    pub fn with_topics(mut self, topics: Vec<String>) -> Self {
        self.topics = topics;
        self
    }

    /// Crawl up to `max_pages` URLs discovered from the seed.
    ///
    /// 1. DISCOVER: fetch seed, extract links, BFS through link graph with depth tracking.
    /// 2. PRIORITIZE: score discovered URLs by depth (shallower = higher priority).
    /// 3. FETCH: budget-constrained, per-host rate-limited fetch of top-scored URLs.
    pub async fn crawl(&self, seed: &str) -> Result<Vec<StructuredContent>> {
        let seed_domain = util::extract_domain(seed).unwrap_or_else(|| "unknown".to_string());

        // Build structural awareness in parallel before touching any page.
        let blueprint = if self.respect_robots {
            let explorer = SiteExplorer::new()?;
            let mut bp = explorer.explore(seed).await.unwrap_or_default();
            // Enrich with content-derived topics from sampled sitemap pages.
            explorer.sample_topics(&mut bp, 5).await;
            bp
        } else {
            Default::default()
        };

        // Merge user topics with sitemap-derived topic keywords.
        let mut effective_topics = self.topics.clone();
        for kw in &blueprint.topic_keywords {
            if !effective_topics.contains(kw) {
                effective_topics.push(kw.clone());
            }
        }

        {
            let mut domains = self.domains.lock().await;
            let state = domains
                .entry(seed_domain.clone())
                .or_insert_with(DomainState::new);
            state.policy = Some(blueprint.robots.clone());
        }

        // Record seed in graph unless it is already a crawled page.
        let seed_already_crawled = if let Some(ref store) = self.graph_store {
            store
                .get_url(seed)
                .await
                .map(|n| n.crawled)
                .unwrap_or(false)
        } else {
            false
        };

        if !seed_already_crawled {
            self.record_url(seed, &seed_domain, DiscoverySource::Seed, 0, 1.0, None, None)
                .await;
        }

        // Seed queue from sitemap (highest priority first), then the user seed if new.
        let seed_relevance = score_relevance(seed, None, &self.topics);
        for entry in &blueprint.sitemap_urls {
            let relevance = score_relevance(&entry.url, None, &self.topics);
            self.record_url(
                &entry.url,
                &seed_domain,
                DiscoverySource::Sitemap,
                1, // sitemap entries are depth 1 from seed
                entry.priority,
                entry.lastmod,
                entry.changefreq.as_deref(),
            )
            .await;
            self.enqueue_url(&seed_domain, &entry.url, 1, relevance).await;
        }
        if !seed_already_crawled {
            self.enqueue_url(&seed_domain, seed, 0, seed_relevance).await;
        }

        // Phase 1+2: DISCOVER + FETCH in interleaved BFS with depth tracking.
        let mut results: Vec<StructuredContent> = Vec::with_capacity(self.max_pages);

        while results.len() < self.max_pages || {
            let domains = self.domains.lock().await;
            domains
                .values()
                .flat_map(|d| d.queue.iter())
                .any(|(_, depth, _)| *depth <= self.min_depth)
        } {
            let next = {
                let domains = self.domains.lock().await;
                // Pick highest-relevance URL across all domains (topic-aware best-first).
                // Break ties by shallowest depth.
                domains
                    .values()
                    .flat_map(|d| d.queue.iter())
                    .max_by(|a, b| {
                        b.2.partial_cmp(&a.2)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| a.1.cmp(&b.1).reverse())
                    })
                    .cloned()
            };

            let (url, depth, _relevance) = match next {
                Some(u) => u,
                None => {
                    info!("crawl queue empty after {} pages", results.len());
                    break;
                }
            };

            let domain = util::extract_domain(&url).unwrap_or_else(|| "unknown".to_string());

            // /research should only discover and fetch URLs that are not already
            // present in the knowledge graph as crawled pages.
            if let Some(ref store) = self.graph_store {
                if let Some(node) = store.get_url(&url).await {
                    if node.crawled {
                        continue;
                    }
                }
            }

            // Wait for per-domain rate limit / backoff.
            self.wait_for_slot(&domain).await;

            // Get or rotate session.
            let session = self.session_for_domain(&domain).await?;
            let proxy_url = session.proxy_url.clone();
            let profile = session.profile.clone();
            let session_id = session.id.clone();

            // Build a request with the session's identity.
            let fetcher = self.fetcher_for_session(&profile, proxy_url.as_deref())?;

            // Dequeue and mark seen.
            {
                let mut domains = self.domains.lock().await;
                if let Some(d) = domains.get_mut(&domain) {
                    if let Some(pos) = d.queue.iter().position(|(u, _, _)| u == &url) {
                        d.queue.remove(pos);
                    }
                    d.last_request = Some(Instant::now());
                }
            }

            // Fetch.
            let start = Instant::now();
            let fetch_result = fetcher.fetch_url(&url).await;
            let elapsed_ms = start.elapsed().as_millis() as u64;

            // Update session/domain stats.
            let mut session_should_rotate = false;
            {
                let mut domains = self.domains.lock().await;
                if let Some(d) = domains.get_mut(&domain) {
                    if let Some(ref mut s) = d.session {
                        s.request_count += 1;
                    }
                    match fetch_result {
                        Ok(ref content) if content.is_valid_content => {
                            d.success_count += 1;
                            if let Some(ref mut s) = d.session {
                                s.consecutive_failures = 0;
                            }
                        }
                        _ => {
                            d.failure_count += 1;
                            if let Some(ref mut s) = d.session {
                                s.consecutive_failures += 1;
                            }
                            session_should_rotate = true;
                            self.apply_backoff(&domain, d).await;
                        }
                    }
                    if d.session
                        .as_ref()
                        .map(|s| s.should_rotate())
                        .unwrap_or(false)
                    {
                        session_should_rotate = true;
                    }
                }
            }

            if session_should_rotate {
                let _ = self.rotate_session(&domain).await;
            }

            // Extract links and enqueue new ones (depth-aware + relevance-scored).
            match fetch_result {
                Ok(content) => {
                    debug!(
                        "fetched {} (depth {}) in {} ms using session {}",
                        url, depth, elapsed_ms, session_id
                    );

                    let next_depth = depth + 1;

                    // Internal links: same domain, enqueue if within max_depth.
                    for link in &content.internal_links {
                        if next_depth <= self.max_depth {
                            let relevance = score_relevance(link, Some(&content), &self.topics);
                            let is_new = self.enqueue_url(&domain, link, next_depth, relevance).await;
                            if is_new {
                                self.record_url(
                                    link,
                                    &domain,
                                    DiscoverySource::LinkCrawl,
                                    next_depth,
                                    relevance,
                                    None,
                                    None,
                                )
                                .await;
                            }
                        }
                        self.record_link(&url, link, None).await;
                    }

                    // External links: cross-domain, enqueue if follow_external && within max_depth.
                    if self.follow_external {
                        for link in &content.external_links {
                            let ext_domain =
                                util::extract_domain(link).unwrap_or_else(|| "unknown".to_string());
                            if ext_domain != domain && next_depth <= self.max_depth {
                                let relevance = score_relevance(link, Some(&content), &self.topics) * 0.3;
                                let is_new =
                                    self.enqueue_url(&ext_domain, link, next_depth, relevance).await;
                                if is_new {
                                    self.record_url(
                                        link,
                                        &ext_domain,
                                        DiscoverySource::ExternalLink,
                                        next_depth,
                                        0.3,
                                        None,
                                        None,
                                    )
                                    .await;
                                }
                            }
                            self.record_link(&url, link, None).await;
                        }
                    }

                    self.mark_url_crawled(&url).await;

                    // Hand full content off to the background worker for KG + vector persistence.
                    if let Some(ref worker) = self.background_worker {
                        let content_url = url.clone();
                        let content_clone = content.clone();
                        let worker = worker.clone();
                        tokio::spawn(async move {
                            if let Err(e) = worker.persist(content_url, content_clone).await {
                                tracing::warn!("failed to queue background persist: {}", e);
                            }
                        });
                    }

                    results.push(content);
                }
                Err(e) => {
                    warn!("fetch failed {}: {}", url, e);
                }
            }
        }

        Ok(results)
    }

    async fn enqueue_url(&self, domain: &str, url: &str, depth: u32, relevance: f32) -> bool {
        // Honor robots.txt disallow rules.
        let allowed = {
            let domains = self.domains.lock().await;
            domains
                .get(domain)
                .and_then(|d| d.policy.as_ref())
                .map(|p| p.is_allowed(url))
                .unwrap_or(true)
        };
        if !allowed {
            debug!("robots.txt disallows {}", url);
            return false;
        }

        let mut domains = self.domains.lock().await;
        let state = domains
            .entry(domain.to_string())
            .or_insert_with(DomainState::new);
        if !state.seen.contains(url) && state.seen.len() < self.max_pages {
            state.seen.insert(url.to_string());
            state.queue.push_back((url.to_string(), depth, relevance));
            return true;
        }
        false
    }

    async fn wait_for_slot(&self, domain: &str) {
        let wait = {
            let domains = self.domains.lock().await;
            if let Some(d) = domains.get(domain) {
                // Honor explicit backoff.
                if let Some(until) = d.backoff_until {
                    let now = Instant::now();
                    if until > now {
                        return sleep(until - now).await;
                    }
                }
                // Enforce minimum delay, RPS, and robots.txt crawl-delay.
                let min_delay = Duration::from_millis(self.delay_ms);
                let rps_delay = Duration::from_secs_f64(1.0 / self.rps);
                let robots_delay = d
                    .policy
                    .as_ref()
                    .and_then(|p| p.crawl_delay_ms)
                    .map(Duration::from_millis);
                let mut required = min_delay.max(rps_delay);
                if let Some(rd) = robots_delay {
                    required = required.max(rd);
                }
                if let Some(last) = d.last_request {
                    let elapsed = last.elapsed();
                    if elapsed < required {
                        return sleep(required - elapsed).await;
                    }
                }
            }
            Duration::ZERO
        };
        if wait > Duration::ZERO {
            sleep(wait).await;
        }
    }

    async fn session_for_domain(&self, domain: &str) -> Result<CrawlSession> {
        {
            let domains = self.domains.lock().await;
            if let Some(d) = domains.get(domain)
                && let Some(ref s) = d.session
                && !s.should_rotate()
            {
                return Ok(s.clone());
            }
        }
        self.rotate_session(domain).await
    }

    async fn rotate_session(&self, domain: &str) -> Result<CrawlSession> {
        let proxy = self.proxy_pool.select(Some(domain));
        let proxy_url = proxy.as_ref().map(|p| p.url.clone());
        let session = self.session_manager.session_for(domain, proxy_url.clone());

        let crawl_session = CrawlSession {
            id: format!("{}-{}", domain, rand::rng().next_u32()),
            domain: domain.to_string(),
            proxy_url,
            profile: session.profile,
            created_at: Instant::now(),
            request_count: 0,
            max_requests: self.pages_per_session,
            max_age: self.session_max_age,
            consecutive_failures: 0,
        };

        info!(
            "rotated session for {} -> proxy {:?}",
            domain, crawl_session.proxy_url
        );

        {
            let mut domains = self.domains.lock().await;
            let state = domains
                .entry(domain.to_string())
                .or_insert_with(DomainState::new);
            state.session = Some(crawl_session.clone());
        }
        Ok(crawl_session)
    }

    fn fetcher_for_session(
        &self,
        profile: &DeviceProfile,
        proxy_url: Option<&str>,
    ) -> Result<Fetcher> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .cookie_store(true);

        if let Some(url) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(url)?);
        }

        let client = builder.build()?;
        Ok(Fetcher::from_client(client)?.with_profile(profile.clone()))
    }

    async fn apply_backoff(&self, domain: &str, state: &mut DomainState) {
        let failures = state.failure_count.saturating_sub(state.success_count);
        let backoff = Duration::from_secs((2u64.pow(failures.min(6) as u32)).min(60));
        warn!(
            "applying {}s backoff for {} after failures",
            backoff.as_secs(),
            domain
        );
        state.backoff_until = Some(Instant::now() + backoff);
    }

    async fn record_url(
        &self,
        url: &str,
        domain: &str,
        source: DiscoverySource,
        depth: u32,
        priority: f32,
        lastmod: Option<DateTime<Utc>>,
        changefreq: Option<&str>,
    ) {
        if let Some(ref store) = self.graph_store {
            store
                .record_url(UrlNode {
                    url: url.to_string(),
                    domain: domain.to_string(),
                    source,
                    depth,
                    priority,
                    lastmod,
                    changefreq: changefreq.map(String::from),
                    discovered_at: Utc::now(),
                    crawled: false,
                })
                .await;
        }
    }

    async fn record_link(&self, from: &str, to: &str, anchor_text: Option<&str>) {
        if let Some(ref store) = self.graph_store {
            store
                .record_link(super::crawl_graph::LinkEdge {
                    from: from.to_string(),
                    to: to.to_string(),
                    anchor_text: anchor_text.map(String::from),
                })
                .await;
        }
    }

    async fn mark_url_crawled(&self, url: &str) {
        if let Some(ref store) = self.graph_store
            && let Some(mut node) = store.get_url(url).await
        {
            node.crawled = true;
            store.record_url(node).await;
        }
    }
}

impl Clone for CrawlSession {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            domain: self.domain.clone(),
            proxy_url: self.proxy_url.clone(),
            profile: self.profile.clone(),
            created_at: self.created_at,
            request_count: self.request_count,
            max_requests: self.max_requests,
            max_age: self.max_age,
            consecutive_failures: self.consecutive_failures,
        }
    }
}
