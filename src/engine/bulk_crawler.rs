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
use super::device_profile::{DeviceProfile, SessionManager, StickySessions};
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
            if title_lower.contains(t) || excerpt_lower.contains(t) || kw_lower.contains(*t) {
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

/// Path depth of a URL: number of non-empty path segments. Used to auto-map
/// crawl depth from the site's own map (sitemap/llms.txt URLs) instead of a
/// manual hop count. `https://example.com/a/b/c` -> 3, root -> 0.
fn path_depth(url: &str) -> u32 {
    reqwest::Url::parse(url)
        .map(|u| u.path().split('/').filter(|s| !s.is_empty()).count() as u32)
        .unwrap_or(0)
}

/// Normalize a URL for scope membership: lowercase scheme+host, strip
/// fragment and trailing slash. Two URLs that point at the same page compare
/// equal so link-following can stay inside the site's declared map.
fn normalize_url(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_fragment(None);
            let mut s = parsed.to_string();
            if s.ends_with('/') && !s.ends_with("://") {
                s.pop();
            }
            s.to_lowercase()
        }
        Err(_) => url.trim_end_matches('/').to_lowercase(),
    }
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
/// Structural awareness (site-driven methodology, "be a polite research bot"):
/// - Fetches `/robots.txt`, `/sitemap.xml`, and the site's curated LLM index
///   (`/llms.txt`, falling back to `/llm.txt`) in parallel before crawling.
/// - Seeds the queue from sitemap entries sorted by priority, plus llms.txt
///   entries (the site's own statement of which pages matter most).
/// - Respects robots.txt `Disallow` and `Crawl-delay` directives.
/// - Auto-maps crawl depth from the site map when `auto_depth` is enabled:
///   depth is derived from each mapped URL's path instead of a manual hop
///   count, and link-following stays inside the site's declared content
///   surface (content privacy) instead of wandering the open web.
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
    /// When true (default), crawl depth is derived from the site's own map
    /// (sitemap + llms.txt path depth) and link-following is bounded to the
    /// mapped URL set. Manual min/max depth become the fallback for sites
    /// that publish no map.
    auto_depth: bool,
    /// Query topics for content-aware link prioritization.
    topics: Vec<String>,
    /// Render JS-heavy / bot-protected pages via headless Chromium (CDP) when
    /// a plain HTTP fetch yields no meaningful content.
    dynamic: bool,
    /// Deep research mode: CDP stealth + infinite-scroll, and no crawl
    /// deadline or backoff caps so long-running investigations can finish.
    deep: bool,
    /// Per-request HTTP timeout. Deep mode raises this so slow JS-heavy pages
    /// are not abandoned prematurely.
    request_timeout: Duration,
}

/// Whether to follow links to external domains during crawling.
#[derive(Clone, Copy)]
pub enum FollowExternalLinks {
    Follow,
    Ignore,
}

/// Whether to respect robots.txt directives.
#[derive(Clone, Copy)]
pub enum RespectRobots {
    Yes,
    No,
}

impl BulkDomainCrawler {
    pub fn new(
        proxy_pool: ProxyPool,
        pages_per_session: usize,
        session_max_age_minutes: u64,
        delay_ms: u64,
        rps: u32,
        max_pages: usize,
        follow_external: FollowExternalLinks,
        min_depth: u32,
        max_depth: u32,
    ) -> Self {
        Self {
            proxy_pool,
            session_manager: SessionManager::new(StickySessions::Sticky),
            domains: Mutex::new(HashMap::new()),
            pages_per_session: pages_per_session.max(1),
            session_max_age: Duration::from_secs(session_max_age_minutes.max(1) * 60),
            delay_ms: delay_ms.max(100),
            rps: (rps.max(1) as f64),
            max_pages,
            respect_robots: true,
            graph_store: None,
            background_worker: None,
            follow_external: match follow_external {
                FollowExternalLinks::Follow => true,
                FollowExternalLinks::Ignore => false,
            },
            min_depth,
            max_depth,
            auto_depth: true,
            topics: Vec::new(),
            dynamic: false,
            deep: false,
            request_timeout: Duration::from_secs(30),
        }
    }

    /// Enable CDP browser rendering fallback for JS-heavy / bot-protected pages.
    pub fn with_dynamic(mut self, dynamic: bool) -> Self {
        self.dynamic = dynamic;
        self
    }

    /// Enable deep research: CDP stealth + infinite-scroll, and disable the
    /// crawl deadline and backoff caps so long investigations can complete.
    pub fn with_deep(mut self, deep: bool) -> Self {
        self.deep = deep;
        if deep {
            // Slow JS-heavy pages deserve a longer per-request timeout.
            self.request_timeout = Duration::from_secs(90);
        }
        self
    }

    pub fn with_respect_robots(mut self, respect: RespectRobots) -> Self {
        self.respect_robots = match respect {
            RespectRobots::Yes => true,
            RespectRobots::No => false,
        };
        self
    }

    /// Enable (default) or disable site-map-derived crawl depth. When enabled,
    /// depth is auto-mapped from the site's sitemap/llms.txt path structure
    /// and link-following is bounded to the site's declared content surface.
    pub fn with_auto_depth(mut self, auto: bool) -> Self {
        self.auto_depth = auto;
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
        self.crawl_with_limit(seed, self.max_pages).await
    }

    /// Crawl from a seed with an explicit page budget, overriding the
    /// crawler-wide `max_pages` for this call. Multi-seed callers (research)
    /// use this to split one total budget across seeds instead of granting
    /// each seed the full `max_pages`.
    pub async fn crawl_with_limit(
        &self,
        seed: &str,
        limit: usize,
    ) -> Result<Vec<StructuredContent>> {
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

        // Auto-map crawl depth from the site's own map (sitemap + llms.txt
        // path depth) and bound link-following to the site's declared content
        // surface. Manual min/max depth is the fallback for sites that
        // publish no map.
        let (effective_max_depth, scope) = if self.auto_depth {
            let mut depths: Vec<u32> = Vec::new();
            let mut scope: HashSet<String> = HashSet::new();
            for entry in &blueprint.sitemap_urls {
                depths.push(path_depth(&entry.url));
                scope.insert(normalize_url(&entry.url));
            }
            if let Some(ref llms) = blueprint.llms {
                for url in llms.urls() {
                    depths.push(path_depth(url));
                    scope.insert(normalize_url(url));
                }
            }
            if scope.is_empty() {
                (self.max_depth, None)
            } else {
                let derived = depths.into_iter().max().unwrap_or(0).clamp(1, 20);
                (derived, Some(scope))
            }
        } else {
            (self.max_depth, None)
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
            self.record_url(
                seed,
                &seed_domain,
                DiscoverySource::Seed,
                0,
                1.0,
                None,
                None,
            )
            .await;
        }

        // Seed queue from sitemap (highest priority first) and the site's curated
        // LLM index (llms.txt / llm.txt), then the user seed if new. Map
        // entries carry their auto-mapped path depth when auto_depth is on.
        let seed_relevance = score_relevance(seed, None, &self.topics);
        for entry in &blueprint.sitemap_urls {
            let relevance = score_relevance(&entry.url, None, &self.topics);
            let depth = if self.auto_depth {
                path_depth(&entry.url)
            } else {
                1
            };
            self.record_url(
                &entry.url,
                &seed_domain,
                DiscoverySource::Sitemap,
                depth,
                entry.priority,
                entry.lastmod,
                entry.changefreq.as_deref(),
            )
            .await;
            self.enqueue_url(&seed_domain, &entry.url, depth, relevance)
                .await;
        }
        // llms.txt entries are the site's own statement of which pages matter
        // most: seed them at top priority, gated by robots.txt like anything
        // else.
        if let Some(ref llms) = blueprint.llms {
            for url in llms.urls() {
                let relevance = score_relevance(url, None, &self.topics).max(0.9);
                let depth = if self.auto_depth { path_depth(url) } else { 1 };
                self.record_url(
                    url,
                    &seed_domain,
                    DiscoverySource::LlmsTxt,
                    depth,
                    1.0,
                    None,
                    None,
                )
                .await;
                self.enqueue_url(&seed_domain, url, depth, relevance).await;
            }
        }
        if !seed_already_crawled {
            self.enqueue_url(&seed_domain, seed, 0, seed_relevance)
                .await;
        }

        // Phase 1+2: DISCOVER + FETCH in interleaved BFS with depth tracking.
        let mut results: Vec<StructuredContent> = Vec::with_capacity(limit);

        while results.len() < limit || {
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
                        // Dequeue before skipping. The selected URL is the
                        // highest-relevance entry in the queue; leaving it in
                        // place makes the next iteration select it again and
                        // `continue` forever (re-crawls re-enqueue sitemap /
                        // link URLs that earlier crawls already marked crawled).
                        let mut domains = self.domains.lock().await;
                        if let Some(d) = domains.get_mut(&domain) {
                            if let Some(pos) = d.queue.iter().position(|(u, _, _)| u == &url) {
                                d.queue.remove(pos);
                            }
                        }
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

                    // Scope check: when the site publishes a map (sitemap /
                    // llms.txt), link-following stays inside the declared
                    // content surface — content privacy. Without a map, all
                    // links are eligible (bounded by effective depth).
                    let in_scope = |link: &str| {
                        scope
                            .as_ref()
                            .map(|s| s.contains(&normalize_url(link)))
                            .unwrap_or(true)
                    };

                    // Internal links: same domain, enqueue if within scope and depth.
                    for link in &content.internal_links {
                        if in_scope(link) && next_depth <= effective_max_depth {
                            let relevance = score_relevance(link, Some(&content), &self.topics);
                            let is_new =
                                self.enqueue_url(&domain, link, next_depth, relevance).await;
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

                    // External links: cross-domain, enqueue if follow_external && within scope && depth.
                    if self.follow_external {
                        for link in &content.external_links {
                            let ext_domain =
                                util::extract_domain(link).unwrap_or_else(|| "unknown".to_string());
                            if ext_domain != domain
                                && in_scope(link)
                                && next_depth <= effective_max_depth
                            {
                                let relevance =
                                    score_relevance(link, Some(&content), &self.topics) * 0.3;
                                let is_new = self
                                    .enqueue_url(&ext_domain, link, next_depth, relevance)
                                    .await;
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
        let proxy = self.proxy_pool.select(Some(domain))?;
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
            .timeout(self.request_timeout)
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .cookie_store(true);

        if let Some(url) = proxy_url {
            builder = builder.proxy(reqwest::Proxy::all(url)?);
        }

        let client = builder.build()?;
        let mut fetcher = Fetcher::from_client(client)?.with_profile(profile.clone());
        if self.dynamic {
            // Chromium fallback when a static fetch yields no meaningful text.
            // Deep mode enables stealth + infinite-scroll for progressive/bot-
            // protected pages.
            fetcher = fetcher
                .with_dynamic_fallback(if self.deep { 4000 } else { 2000 })
                .with_dynamic_deep(self.deep);
        }
        Ok(fetcher)
    }

    async fn apply_backoff(&self, domain: &str, state: &mut DomainState) {
        // Deep research mode never backs off: a failed fetch is retried without
        // penalty so a single transient failure can't stall a long investigation.
        if self.deep {
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Re-crawls re-enqueue URLs that earlier crawls marked crawled. The crawl
    /// loop must dequeue-and-skip them instead of re-selecting the top entry
    /// forever (regression: infinite loop hung research requests).
    #[tokio::test]
    async fn test_crawl_skips_already_crawled_urls_without_hanging() {
        let graph = Arc::new(InMemoryCrawlGraph::new());
        let seed = "https://example.com/";
        let crawled_page = "https://example.com/crawled-page";
        for url in [seed, crawled_page] {
            graph
                .record_url(UrlNode {
                    url: url.to_string(),
                    domain: "example.com".to_string(),
                    source: DiscoverySource::Seed,
                    depth: 0,
                    priority: 1.0,
                    lastmod: None,
                    changefreq: None,
                    discovered_at: Utc::now(),
                    crawled: true,
                })
                .await;
        }

        let crawler = BulkDomainCrawler::new(
            ProxyPool::new(),
            100,
            30,
            100,
            1,
            50,
            FollowExternalLinks::Ignore,
            0,
            5,
        )
        .with_respect_robots(RespectRobots::No)
        .with_graph_store(graph.clone());

        // Queue an already-crawled URL as the only work item. Before the fix
        // the loop selected it, saw `crawled`, and `continue`d without
        // dequeuing, spinning forever.
        assert!(
            crawler
                .enqueue_url("example.com", crawled_page, 0, 1.0)
                .await
        );

        let result = tokio::time::timeout(Duration::from_secs(5), crawler.crawl(seed))
            .await
            .expect("crawl hung: already-crawled URL was re-selected forever");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_path_depth() {
        // Root has no path segments.
        assert_eq!(path_depth("https://example.com"), 0);
        assert_eq!(path_depth("https://example.com/"), 0);
        assert_eq!(path_depth("https://example.com/index.html"), 1);
        // One segment per non-empty path part.
        assert_eq!(path_depth("https://example.com/docs"), 1);
        assert_eq!(path_depth("https://example.com/docs/"), 1);
        assert_eq!(path_depth("https://example.com/docs/guide"), 2);
        assert_eq!(path_depth("https://example.com/a/b/c"), 3);
        // Query strings and fragments do not add depth.
        assert_eq!(path_depth("https://example.com/docs?ref=1"), 1);
        assert_eq!(path_depth("https://example.com/docs/#top"), 1);
        // Trailing slashes collapse; double slashes are ignored.
        assert_eq!(path_depth("https://example.com/a/b/"), 2);
        assert_eq!(path_depth("https://example.com//a//b"), 2);
    }

    #[test]
    fn test_normalize_url() {
        // Scheme/authority lowercased, fragment dropped, trailing slash kept off.
        assert_eq!(
            normalize_url("HTTPS://Example.COM/Docs"),
            "https://example.com/docs"
        );
        assert_eq!(
            normalize_url("https://example.com/#section"),
            "https://example.com"
        );
        assert_eq!(normalize_url("https://example.com"), "https://example.com");
        assert_eq!(normalize_url("https://example.com/"), "https://example.com");
        // Path case is preserved (servers may be case-sensitive), only host case
        // is folded — map URLs and discovered links must compare equal on host.
        assert_eq!(
            normalize_url("https://Example.com/Docs/Guide"),
            "https://example.com/docs/guide"
        );
        // Query strings are preserved (they distinguish real URLs).
        assert_eq!(
            normalize_url("https://example.com/search?q=rust"),
            "https://example.com/search?q=rust"
        );
    }
}
