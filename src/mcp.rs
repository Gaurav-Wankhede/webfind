use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars, tool, tool_router,
};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::engine::bulk_crawler::BulkDomainCrawler;
use crate::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph, TraversalDirection};
use crate::engine::device_profile::SessionManager;
use crate::engine::embedder::FastembedEmbedder;
use crate::engine::fetcher::Fetcher;
use crate::engine::fingerprint::FingerprintAuditLog;
use crate::engine::graph_summary::build_graph_summary;
use crate::engine::indexer::Indexer;
use crate::engine::pagerank_cache::PageRankCache;
use crate::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use crate::engine::ranker::Ranker;
use crate::engine::vector::VectorEngine;
use crate::schema::content::StructuredContent;
use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
use crate::schema::response::SearchResponse;

#[derive(Clone)]
pub struct WebfindMcpServer {
    indexer: Arc<Mutex<Indexer>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
    indexer_queue: Arc<tokio::sync::Mutex<()>>,
}

impl WebfindMcpServer {
    pub fn new(
        indexer: Arc<Mutex<Indexer>>,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            indexer,
            graph_store,
            audit_store,
            data_dir,
            indexer_queue: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    pub async fn run_stdio(
        indexer: Indexer,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
        data_dir: PathBuf,
    ) -> anyhow::Result<()> {
        let server = Self::new(Arc::new(Mutex::new(indexer)), graph_store, audit_store, data_dir);
        let service = server.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    }
}



#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct SearchToolParams {
    /// Search query string.
    query: String,
    /// Maximum number of results to return (default 10).
    #[serde(default)]
    limit: Option<usize>,
    /// Include graph relationships in the response.
    #[serde(default)]
    include_graph: Option<bool>,
    /// Enable BM25 + vector hybrid re-ranking.
    #[serde(default)]
    hybrid: Option<bool>,
    /// Current timestamp from system_timestamp tool (ISO8601). Used to prioritize recent content.
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct ResearchToolParams {
    /// Seed URL to crawl.
    seed: String,
    /// Query to run against the freshly indexed pages.
    query: String,
    /// Maximum pages to crawl (default 50, max 500).
    #[serde(default)]
    max_pages: Option<usize>,
    /// Delay between requests in milliseconds (default 1000).
    #[serde(default)]
    delay: Option<u32>,
    /// Enable BM25 + vector hybrid re-ranking.
    #[serde(default)]
    hybrid: Option<bool>,
    /// Maximum number of results to return (default 10).
    #[serde(default)]
    limit: Option<usize>,
    /// Include graph relationships discovered during the crawl.
    #[serde(default)]
    include_graph: Option<bool>,
    /// Include the full scraped page content for each result.
    #[serde(default)]
    include_content: Option<bool>,
    /// Comma-separated proxy URLs to route crawl requests through (http://, https://, socks5://).
    #[serde(default)]
    proxies: Option<String>,
    /// Follow external (cross-domain) links during crawl (default false).
    #[serde(default)]
    follow_external: Option<bool>,
    /// Force-fetch URLs at or below this hop depth from the seed (default 3).
    #[serde(default)]
    min_depth: Option<u32>,
    /// Maximum discovery depth — stop following links beyond this hop count (default 5).
    #[serde(default)]
    max_depth: Option<u32>,
    /// Query topics for content-aware link prioritization (comma-separated).
    #[serde(default)]
    topics: Option<String>,
    /// Additional seed URLs for multi-seed crawling (comma-separated).
    #[serde(default)]
    seeds: Option<String>,
    /// Current timestamp from system_timestamp tool (ISO8601). Used to enhance queries with current year.
    #[serde(default)]
    timestamp: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct ResearchParallelParams {
    /// Multiple research jobs to run in parallel.
    #[schemars(description = "Multiple research jobs to run in parallel")]
    jobs: Vec<ResearchToolParams>,
}

#[derive(Clone)]
struct JobSpec {
    index: usize,
    seed: String,
    query: String,
    limit: usize,
    max_pages: usize,
    delay_ms: u64,
    hybrid: bool,
    include_graph: bool,
    include_content: bool,
    proxies: String,
    follow_external: bool,
    min_depth: u32,
    max_depth: u32,
    topics: Vec<String>,
    seeds: Vec<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct GraphToolParams {
    /// Starting URL.
    url: String,
    /// Traversal depth (default 1).
    #[serde(default)]
    depth: Option<u32>,
    /// Direction: inbound, outbound, or both (default both).
    direction: Option<String>,
}

#[derive(Debug, Clone, Deserialize, schemars::JsonSchema)]
struct FetchToolParams {
    /// URL to fetch.
    url: String,
    /// Comma-separated proxy URLs to route the request through.
    #[serde(default)]
    proxies: Option<String>,
    /// Use Chromium headless browser fallback for JS-rendered pages.
    #[serde(default)]
    dynamic: Option<bool>,
    /// Milliseconds to wait for JS execution in dynamic mode.
    #[serde(default)]
    dynamic_wait_ms: Option<u64>,
    /// Include extracted internal/external links in the response.
    #[serde(default)]
    extract_links: Option<bool>,
    /// Include extracted keywords in the response.
    #[serde(default)]
    extract_keywords: Option<bool>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct FetchParallelParams {
    /// Multiple URLs to fetch in parallel.
    #[schemars(description = "Multiple URLs to fetch in parallel")]
    urls: Vec<String>,
    /// Comma-separated proxy URLs to route every request through.
    #[serde(default)]
    proxies: Option<String>,
    /// Use Chromium headless browser fallback for JS-rendered pages.
    #[serde(default)]
    dynamic: Option<bool>,
    /// Milliseconds to wait for JS execution in dynamic mode.
    #[serde(default)]
    dynamic_wait_ms: Option<u64>,
    /// Include extracted internal/external links in every response.
    #[serde(default)]
    extract_links: Option<bool>,
    /// Include extracted keywords in every response.
    #[serde(default)]
    extract_keywords: Option<bool>,
}

#[tool_router(server_handler)]
impl WebfindMcpServer {
    #[tool(description = "Search the WebFind index for the given query.")]
    async fn webfind_search(
        &self,
        Parameters(params): Parameters<SearchToolParams>,
    ) -> Result<CallToolResult, String> {
        let limit = params.limit.unwrap_or(10).clamp(1, 100);
        let include_graph = params.include_graph.unwrap_or(false);
        let hybrid = params.hybrid.unwrap_or(false);

        // Enhance query with current year from timestamp if not already present
        let mut query = params.query.clone();
        if let Some(ref ts) = params.timestamp {
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) {
                let year = parsed.format("%Y").to_string();
                let chars: Vec<char> = query.chars().collect();
                let has_year = chars.windows(4).any(|w| w.iter().all(|c| c.is_ascii_digit()));
                if !has_year {
                    query = format!("{} {}", query.trim(), year);
                }
            }
        }

        let mut results = self
            .indexer
            .lock()
            .await
            .search_bm25(&query, limit)
            .map_err(|e| e.to_string())?;

        let request = SearchRequest {
            query,
            depth: SearchDepth::Standard,
            limit: limit as u32,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content: false,
            include_graph,
            include_keywords: false,
            include_metrics: false,
            hybrid,
        };

        let graph_scores: Option<HashMap<String, f64>> = match &self.graph_store {
            Some(store) => {
                let cache = PageRankCache::new(&self.data_dir);
                match cache.get_or_compute(store.as_ref(), 20, 0.85).await {
                    Ok(scores) => Some(scores),
                    Err(e) => {
                        tracing::warn!("PageRank cache failed: {}", e);
                        None
                    }
                }
            }
            None => None,
        };

        let vector_scores: Option<HashMap<String, f64>> = if hybrid {
            match self
                .indexer
                .lock()
                .await
                .search_vector(&params.query, limit)
            {
                Ok(scores) => Some(scores),
                Err(e) => {
                    tracing::warn!("vector search failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let ranker = Ranker::new();
        results = ranker.rank(
            results,
            &request,
            graph_scores.as_ref(),
            vector_scores.as_ref(),
        );

        let graph_summary = if include_graph {
            match &self.graph_store {
                Some(store) => build_graph_summary(store.as_ref(), &results).await,
                None => None,
            }
        } else {
            None
        };

        let mut signals = vec!["bm25".to_string()];
        if hybrid {
            signals.push("vector".to_string());
        }
        if graph_scores.is_some() {
            signals.push("graph".to_string());
        }

        let response = SearchResponse {
            request_id: uuid::Uuid::new_v4().to_string(),
            query: params.query,
            depth: SearchDepth::Standard,
            total_results: results.len() as u64,
            returned: results.len() as u32,
            latency_ms: 0,
            results,
            suggestions: vec![],
            related: vec![],
            graph: graph_summary,
            metadata: crate::schema::response::SearchMetadata {
                index_version: "1".to_string(),
                index_size: 0,
                engine_version: env!("CARGO_PKG_VERSION").to_string(),
                searched_at: chrono::Utc::now(),
                signals_used: signals,
                index_freshness: crate::schema::response::IndexFreshness {
                    oldest_page: None,
                    newest_page: None,
                    avg_age_days: 0.0,
                },
            },
        };

        let text =
            serde_json::to_string_pretty(&response.to_llm_value()).map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    async fn research_one(
        &self,
        params: ResearchToolParams,
    ) -> Result<SearchResponse, String> {
        let limit = params.limit.unwrap_or(10).clamp(1, 100);
        let max_pages = params.max_pages.unwrap_or(50).clamp(1, 500);
        let delay_ms = params.delay.unwrap_or(1000).max(100) as u64;
        let hybrid = params.hybrid.unwrap_or(false);
        let include_graph = params.include_graph.unwrap_or(false);
        let include_content = params.include_content.unwrap_or(false);
        let follow_external = params.follow_external.unwrap_or(false);
        let min_depth = params.min_depth.unwrap_or(3);
        let max_depth = params.max_depth.unwrap_or(5);
        let topics: Vec<String> = params
            .topics
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let seeds: Vec<String> = params
            .seeds
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_string())
                    .filter(|t| !t.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        // Enhance query with current year from timestamp if not already present
        let mut query = params.query.clone();
        if let Some(ref ts) = params.timestamp {
            if let Ok(parsed) = chrono::DateTime::parse_from_rfc3339(ts) {
                let year = parsed.format("%Y").to_string();
                // Only add year if query doesn't already contain a 4-digit year
                let chars: Vec<char> = query.chars().collect();
                let has_year = chars.windows(4).any(|w| w.iter().all(|c| c.is_ascii_digit()));
                if !has_year {
                    query = format!("{} {}", query.trim(), year);
                }
            }
        }

        let graph: Arc<dyn CrawlGraphStore + Send + Sync> = self
            .graph_store
            .clone()
            .map(|s| s as Arc<dyn CrawlGraphStore + Send + Sync>)
            .unwrap_or_else(|| Arc::new(InMemoryCrawlGraph::new()));

        let proxy_pool = ProxyPool::new();
        if let Some(ref proxies_str) = params.proxies {
            for url in proxies_str.split(',') {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    proxy_pool.add(ProxyEndpoint::from_url(&url).map_err(|e| e.to_string())?);
                }
            }
        }

        let crawler = BulkDomainCrawler::new(
            proxy_pool,
            100,
            30,
            delay_ms,
            1,
            max_pages,
            follow_external,
            min_depth,
            max_depth,
        )
        .with_respect_robots(true)
        .with_graph_store(graph.clone())
        .with_topics(topics);

        // Multi-seed: crawl primary seed + additional seeds, merge and deduplicate.
        let mut all_seeds = vec![params.seed.clone()];
        all_seeds.extend(seeds);
        let mut contents: Vec<StructuredContent> = Vec::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        for seed_url in &all_seeds {
            let crawled = tokio::time::timeout(
                std::time::Duration::from_secs(60),
                crawler.crawl(seed_url),
            )
            .await
            .map_err(|_| "crawl timed out after 60s".to_string())
            .and_then(|r| r.map_err(|e| e.to_string()))?;
            for c in crawled {
                if seen_urls.insert(c.url.clone()) {
                    contents.push(c);
                }
            }
        }

        if contents.is_empty() {
            return Ok(SearchResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                query,
                depth: SearchDepth::Standard,
                total_results: 0,
                returned: 0,
                latency_ms: 0,
                results: vec![],
                suggestions: vec![],
                related: vec![],
                graph: None,
                metadata: crate::schema::response::SearchMetadata {
                    index_version: "1".to_string(),
                    index_size: 0,
                    engine_version: env!("CARGO_PKG_VERSION").to_string(),
                    searched_at: chrono::Utc::now(),
                    signals_used: vec!["bm25".to_string()],
                    index_freshness: crate::schema::response::IndexFreshness {
                        oldest_page: None,
                        newest_page: None,
                        avg_age_days: 0.0,
                    },
                },
            });
        }

        // Serialize writes to the Tantivy index through a FIFO queue. Crawling
        // happens concurrently; only indexing + commit + search are ordered.
        let _queue_guard = self.indexer_queue.lock().await;

        {
            let mut indexer = self.indexer.lock().await;
            if hybrid {
                let embedder = FastembedEmbedder::new().map_err(|e| e.to_string())?;
                indexer.attach_vector_engine(VectorEngine::new(Arc::new(embedder)));
            }
            indexer.index_batch(&contents).map_err(|e| e.to_string())?;
        }

        let indexer = self.indexer.lock().await;
        let mut results = indexer
            .search_bm25(&query, limit)
            .map_err(|e| e.to_string())?;

        let vector_scores: Option<HashMap<String, f64>> = if hybrid {
            match indexer.search_vector(&query, limit) {
                Ok(scores) => Some(scores),
                Err(e) => {
                    tracing::warn!("vector search failed: {}", e);
                    None
                }
            }
        } else {
            None
        };

        let request = SearchRequest {
            query: query.clone(),
            depth: SearchDepth::Standard,
            limit: limit as u32,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content,
            include_graph,
            include_keywords: false,
            include_metrics: false,
            hybrid,
        };

        let ranker = Ranker::new();
        results = ranker.rank(results, &request, None, vector_scores.as_ref());

        if include_content {
            Indexer::attach_content(&mut results, &contents);
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

        Ok(SearchResponse {
            request_id: uuid::Uuid::new_v4().to_string(),
            query,
            depth: SearchDepth::Standard,
            total_results: results.len() as u64,
            returned: results.len() as u32,
            latency_ms: 0,
            results,
            suggestions: vec![],
            related: vec![],
            graph: graph_summary,
            metadata: crate::schema::response::SearchMetadata {
                index_version: "1".to_string(),
                index_size: contents.len() as u64,
                engine_version: env!("CARGO_PKG_VERSION").to_string(),
                searched_at: chrono::Utc::now(),
                signals_used: signals,
                index_freshness: crate::schema::response::IndexFreshness {
                    oldest_page: None,
                    newest_page: None,
                    avg_age_days: 0.0,
                },
            },
        })
    }

    #[tool(description = "Crawl a seed URL and search the freshly indexed content.")]
    async fn webfind_research(
        &self,
        Parameters(params): Parameters<ResearchToolParams>,
    ) -> Result<CallToolResult, String> {
        let response = self.research_one(params).await?;
        let text = serde_json::to_string_pretty(&response.to_llm_value())
            .map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Run multiple research crawls in parallel. Crawls run concurrently; indexing + commit + search are serialized through a FIFO queue, with a single shared commit for all jobs.")]
    async fn webfind_research_parallel(
        &self,
        Parameters(params): Parameters<ResearchParallelParams>,
    ) -> Result<CallToolResult, String> {
        let jobs: Vec<JobSpec> = params
            .jobs
            .into_iter()
            .enumerate()
            .map(|(index, job)| -> Result<JobSpec, String> {
                Ok(JobSpec {
                    index,
                    seed: job.seed,
                    query: job.query,
                    limit: job.limit.unwrap_or(10).clamp(1, 100),
                    max_pages: job.max_pages.unwrap_or(50).clamp(1, 500),
                    delay_ms: job.delay.unwrap_or(1000).max(100) as u64,
                    hybrid: job.hybrid.unwrap_or(false),
                    include_graph: job.include_graph.unwrap_or(false),
                    include_content: job.include_content.unwrap_or(false),
                    proxies: job.proxies.unwrap_or_default(),
                    follow_external: job.follow_external.unwrap_or(false),
                    min_depth: job.min_depth.unwrap_or(3),
                    max_depth: job.max_depth.unwrap_or(5),
                    topics: job
                        .topics
                        .as_deref()
                        .map(|s| {
                            s.split(',')
                                .map(|t| t.trim().to_lowercase())
                                .filter(|t| !t.is_empty())
                                .collect()
                        })
                        .unwrap_or_default(),
                    seeds: job
                        .seeds
                        .as_deref()
                        .map(|s| {
                            s.split(',')
                                .map(|t| t.trim().to_string())
                                .filter(|t| !t.is_empty())
                                .collect()
                        })
                        .unwrap_or_default(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let default_graph: Arc<dyn CrawlGraphStore + Send + Sync> =
            Arc::new(InMemoryCrawlGraph::new());
        let graph = self
            .graph_store
            .clone()
            .map(|s| s as Arc<dyn CrawlGraphStore + Send + Sync>)
            .unwrap_or_else(|| default_graph.clone());

        // Phase 1: crawl all seeds concurrently.
        let crawl_futures = jobs.into_iter().map(|job| {
            let graph = graph.clone();
            async move {
                let proxy_pool = ProxyPool::new();
                if !job.proxies.is_empty() {
                    for url in job.proxies.split(',') {
                        let url = url.trim().to_string();
                        if !url.is_empty() {
                            if let Ok(ep) = ProxyEndpoint::from_url(&url) {
                                proxy_pool.add(ep);
                            }
                        }
                    }
                }
                let crawler = BulkDomainCrawler::new(
                    proxy_pool,
                    100,
                    30,
                    job.delay_ms,
                    1,
                    job.max_pages,
                    job.follow_external,
                    job.min_depth,
                    job.max_depth,
                )
                .with_respect_robots(true)
                .with_graph_store(graph)
                .with_topics(job.topics.clone());

                // Multi-seed: crawl primary seed + additional seeds, merge and deduplicate.
                let mut all_seeds = vec![job.seed.clone()];
                all_seeds.extend(job.seeds.clone());
                let mut contents: Vec<StructuredContent> = Vec::new();
                let mut seen_urls: HashSet<String> = HashSet::new();
                for seed_url in &all_seeds {
                    match tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        crawler.crawl(seed_url),
                    )
                    .await
                    {
                        Ok(Ok(crawled)) => {
                            for c in crawled {
                                if seen_urls.insert(c.url.clone()) {
                                    contents.push(c);
                                }
                            }
                        }
                        Ok(Err(err)) => return Err((job, err.to_string())),
                        Err(_) => return Err((job, "crawl timed out after 60s".to_string())),
                    }
                }
                Ok((job, contents))
            }
        });
        let crawled = futures::future::join_all(crawl_futures).await;

        let mut job_results: Vec<serde_json::Value> = Vec::new();
        let mut all_contents: Vec<StructuredContent> = Vec::new();
        let mut success_jobs: Vec<(JobSpec, Vec<StructuredContent>)> = Vec::new();

        for result in crawled {
            match result {
                Ok((job, contents)) if contents.is_empty() => {
                    job_results.push(serde_json::json!({
                        "index": job.index,
                        "seed": job.seed,
                        "query": job.query,
                        "success": true,
                        "result": SearchResponse {
                            request_id: uuid::Uuid::new_v4().to_string(),
                            query: job.query,
                            depth: SearchDepth::Standard,
                            total_results: 0,
                            returned: 0,
                            latency_ms: 0,
                            results: vec![],
                            suggestions: vec![],
                            related: vec![],
                            graph: None,
                            metadata: crate::schema::response::SearchMetadata {
                                index_version: "1".to_string(),
                                index_size: 0,
                                engine_version: env!("CARGO_PKG_VERSION").to_string(),
                                searched_at: chrono::Utc::now(),
                                signals_used: vec!["bm25".to_string()],
                                index_freshness: crate::schema::response::IndexFreshness {
                                    oldest_page: None,
                                    newest_page: None,
                                    avg_age_days: 0.0,
                                },
                            },
                        }.to_llm_value(),
                    }));
                }
                Ok((job, contents)) => {
                    all_contents.extend(contents.clone());
                    success_jobs.push((job, contents));
                }
                Err((job, err)) => {
                    job_results.push(serde_json::json!({
                        "index": job.index,
                        "seed": job.seed,
                        "query": job.query,
                        "success": false,
                        "error": err,
                    }));
                }
            }
        }

        // Phase 2: index everything once and search each query under the FIFO queue.
        if !all_contents.is_empty() {
            let _queue_guard = self.indexer_queue.lock().await;

            let any_hybrid = success_jobs.iter().any(|(job, _)| job.hybrid);
            {
                let mut indexer = self.indexer.lock().await;
                if any_hybrid {
                    let embedder = FastembedEmbedder::new().map_err(|e| e.to_string())?;
                    indexer.attach_vector_engine(VectorEngine::new(Arc::new(embedder)));
                }
                indexer.index_batch(&all_contents).map_err(|e| e.to_string())?;
            }

            let indexer = self.indexer.lock().await;
            for (job, contents) in success_jobs {
                let mut results = indexer
                    .search_bm25(&job.query, job.limit)
                    .map_err(|e| e.to_string())?;

                let vector_scores: Option<HashMap<String, f64>> = if job.hybrid {
                    match indexer.search_vector(&job.query, job.limit) {
                        Ok(scores) => Some(scores),
                        Err(e) => {
                            tracing::warn!("vector search failed: {}", e);
                            None
                        }
                    }
                } else {
                    None
                };

                let request = SearchRequest {
                    query: job.query.clone(),
                    depth: SearchDepth::Standard,
                    limit: job.limit as u32,
                    output: OutputFormat::Json,
                    language: None,
                    date_range: None,
                    domains: None,
                    content_type: Some(ContentType::Any),
                    include_content: job.include_content,
                    include_graph: job.include_graph,
                    include_keywords: false,
                    include_metrics: false,
                    hybrid: job.hybrid,
                };

                let ranker = Ranker::new();
                results = ranker.rank(results, &request, None, vector_scores.as_ref());

                if job.include_content {
                    Indexer::attach_content(&mut results, &contents);
                }

                let graph_summary = if job.include_graph {
                    build_graph_summary(graph.as_ref(), &results).await
                } else {
                    None
                };

                let mut signals = vec!["bm25".to_string()];
                if job.hybrid {
                    signals.push("vector".to_string());
                }

                let response = SearchResponse {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    query: job.query,
                    depth: SearchDepth::Standard,
                    total_results: results.len() as u64,
                    returned: results.len() as u32,
                    latency_ms: 0,
                    results,
                    suggestions: vec![],
                    related: vec![],
                    graph: graph_summary,
                    metadata: crate::schema::response::SearchMetadata {
                        index_version: "1".to_string(),
                        index_size: all_contents.len() as u64,
                        engine_version: env!("CARGO_PKG_VERSION").to_string(),
                        searched_at: chrono::Utc::now(),
                        signals_used: signals,
                        index_freshness: crate::schema::response::IndexFreshness {
                            oldest_page: None,
                            newest_page: None,
                            avg_age_days: 0.0,
                        },
                    },
                };

                job_results.push(serde_json::json!({
                    "index": job.index,
                    "seed": job.seed,
                    "query": response.query,
                    "success": true,
                    "result": response.to_llm_value(),
                }));
            }
        }

        job_results.sort_by(|a, b| {
            let ai = a.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            let bi = b.get("index").and_then(|v| v.as_u64()).unwrap_or(0);
            ai.cmp(&bi)
        });

        let payload = serde_json::json!({
            "jobs": job_results.len(),
            "results": job_results,
        });
        let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Traverse the WebFind crawl graph from a starting URL.")]
    async fn webfind_graph(
        &self,
        Parameters(params): Parameters<GraphToolParams>,
    ) -> Result<CallToolResult, String> {
        let store = self
            .graph_store
            .as_ref()
            .ok_or("no graph store configured")?;
        let depth = params.depth.unwrap_or(1);
        let direction = match params.direction.as_deref() {
            Some("inbound") => TraversalDirection::Inbound,
            Some("outbound") => TraversalDirection::Outbound,
            _ => TraversalDirection::Both,
        };

        let visited = crate::engine::crawl_graph::traverse_graph(
            store.clone(),
            &params.url,
            depth,
            direction,
        )
        .await;

        let text = serde_json::to_string_pretty(&serde_json::json!({
            "start": params.url,
            "depth": depth,
            "direction": format!("{:?}", direction).to_lowercase(),
            "urls": visited,
        }))
        .map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    async fn fetch_one(
        &self,
        params: FetchToolParams,
    ) -> Result<serde_json::Value, String> {
        let proxy_list: Vec<String> = params
            .proxies
            .as_ref()
            .map(|s| {
                s.split(',')
                    .map(|x| x.trim().to_string())
                    .filter(|x| !x.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let proxy_pool = if proxy_list.is_empty() {
            None
        } else {
            let pool = ProxyPool::new();
            for url in &proxy_list {
                pool.add(ProxyEndpoint::from_url(url).map_err(|e| e.to_string())?);
            }
            Some(pool)
        };

        let dynamic = params.dynamic.unwrap_or(false);
        let dynamic_wait_ms = params.dynamic_wait_ms.unwrap_or(2000);

        // First try a direct fetch.
        let direct_fetcher = Fetcher::new().map_err(|e| e.to_string())?;
        let direct_fetcher = if dynamic {
            direct_fetcher.with_dynamic_fallback(dynamic_wait_ms)
        } else {
            direct_fetcher
        };

        let content = match direct_fetcher.fetch_url(&params.url).await {
            Ok(c) => c,
            Err(direct_err) => {
                // If proxies were supplied, retry through the proxy pool with a
                // random user-agent + rotating egress IP.
                if let Some(pool) = proxy_pool {
                    let session_manager = SessionManager::new(true);
                    let mut proxy_fetcher = Fetcher::new_human(
                        Some(pool),
                        Some(session_manager),
                        true, // random UA per request
                        1,
                    )
                    .map_err(|e| e.to_string())?;
                    if let Some(ref audit) = self.audit_store {
                        proxy_fetcher = proxy_fetcher.with_audit_log(audit.clone());
                    }
                    let proxy_fetcher = if dynamic {
                        proxy_fetcher.with_dynamic_fallback(dynamic_wait_ms)
                    } else {
                        proxy_fetcher
                    };
                    proxy_fetcher
                        .fetch_url(&params.url)
                        .await
                        .map_err(|proxy_err| {
                            format!(
                                "direct fetch failed: {}; proxy fallback also failed: {}",
                                direct_err, proxy_err
                            )
                        })?
                } else {
                    return Err(direct_err.to_string());
                }
            }
        };

        let include_links = params.extract_links.unwrap_or(false);
        let include_keywords = params.extract_keywords.unwrap_or(false);

        Ok(serde_json::json!({
            "url": content.url,
            "final_url": content.final_url,
            "title": content.title,
            "status_code": content.status_code,
            "excerpt": content.excerpt,
            "published_at": content.published_at,
            "modified_at": content.modified_at,
            "author": content.author,
            "site_name": content.site_name,
            "content_text": content.content_text,
            "content_markdown": content.content_markdown,
            "word_count": content.word_count,
            "reading_time_seconds": content.reading_time_seconds,
            "language": content.language,
            "language_confidence": content.language_confidence,
            "is_valid_content": content.is_valid_content,
            "internal_links": if include_links { Some(&content.internal_links) } else { None as Option<&Vec<String>> },
            "external_links": if include_links { Some(&content.external_links) } else { None as Option<&Vec<String>> },
            "keywords": if include_keywords { Some(&content.keywords) } else { None as Option<&Vec<crate::schema::response::Keyword>> },
        }))
    }

    #[tool(
        description = "Fetch and extract content from a URL, falling back to proxies with random identity if the direct request fails."
    )]
    async fn webfind_fetch(
        &self,
        Parameters(params): Parameters<FetchToolParams>,
    ) -> Result<CallToolResult, String> {
        let response = self.fetch_one(params).await?;
        let text = serde_json::to_string_pretty(&response).map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }

    #[tool(description = "Fetch and extract content from multiple URLs in parallel.")]
    async fn webfind_fetch_parallel(
        &self,
        Parameters(params): Parameters<FetchParallelParams>,
    ) -> Result<CallToolResult, String> {
        let dynamic = params.dynamic.unwrap_or(false);
        let dynamic_wait_ms = params.dynamic_wait_ms.unwrap_or(2000);
        let extract_links = params.extract_links.unwrap_or(false);
        let extract_keywords = params.extract_keywords.unwrap_or(false);

        let futures = params.urls.into_iter().enumerate().map(|(idx, url)| {
            let server = self.clone();
            let url_for_error = url.clone();
            let job = FetchToolParams {
                url,
                proxies: params.proxies.clone(),
                dynamic: Some(dynamic),
                dynamic_wait_ms: Some(dynamic_wait_ms),
                extract_links: Some(extract_links),
                extract_keywords: Some(extract_keywords),
            };
            async move {
                match server.fetch_one(job).await {
                    Ok(value) => serde_json::json!({
                        "index": idx,
                        "success": true,
                        "result": value,
                    }),
                    Err(err) => serde_json::json!({
                        "index": idx,
                        "url": url_for_error,
                        "success": false,
                        "error": err,
                    }),
                }
            }
        });

        let results = futures::future::join_all(futures).await;
        let payload = serde_json::json!({
            "jobs": results.len(),
            "results": results,
        });
        let text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
        Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
    }
}

