use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use rmcp::{
    ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock},
    schemars, tool, tool_router,
};
use rquickjs::{CatchResultExt, Ctx, Function, Runtime};
use serde::Deserialize;
use tokio::sync::RwLock;

use crate::engine::bg_worker::BackgroundWorker;
use crate::engine::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use crate::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph, TraversalDirection};
use crate::engine::device_profile::{SessionManager, StickySessions};
use crate::engine::embedder::{Embedder, FastembedEmbedder};
use crate::engine::fetcher::{Fetcher, RotateUserAgent};
use crate::engine::fingerprint::FingerprintAuditLog;
use crate::engine::graph_summary::build_graph_summary;
use crate::engine::indexer::attach_content;
use crate::engine::pagerank_cache::PageRankCache;
use crate::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use crate::engine::ranker::Ranker;
use crate::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use crate::schema::content::StructuredContent;
use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
use crate::schema::response::SearchResponse;

// Global state for Code Mode (FR-10) - accessed by synchronous JS functions
static CODE_MODE_STATE: OnceLock<CodeModeState> = OnceLock::new();

struct CodeModeState {
    indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    data_dir: PathBuf,
}

fn get_code_mode_state() -> &'static CodeModeState {
    CODE_MODE_STATE
        .get()
        .expect("Code Mode state not initialized")
}

fn init_code_mode_state(
    indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    data_dir: PathBuf,
) {
    CODE_MODE_STATE
        .set(CodeModeState {
            indexer,
            graph_store,
            data_dir,
        })
        .ok();
}

#[derive(Clone)]
pub struct WebfindMcpServer {
    indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
}

impl WebfindMcpServer {
    pub fn new(
        indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            indexer,
            graph_store,
            audit_store,
            data_dir,
        }
    }

    pub async fn run_stdio(
        indexer: Arc<dyn SearchEngine + Send + Sync>,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
        data_dir: PathBuf,
    ) -> anyhow::Result<()> {
        let server = Self::new(
            Arc::new(RwLock::new(indexer)),
            graph_store,
            audit_store,
            data_dir,
        );
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
    /// Seed URL to crawl. If omitted, WebFind will auto-discover seeds from its index, graph, and query-derived candidates.
    #[serde(default)]
    seed: Option<String>,
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

/// Parameters for the Code Mode tool (FR-10): execute JavaScript with WebFind functions.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct RunToolParams {
    /// JavaScript code to execute. Has access to `search()`, `fetch()`, `research()` functions.
    code: String,
    /// Maximum execution time in milliseconds (default: 30000).
    #[serde(default)]
    timeout_ms: Option<u64>,
    /// Memory limit in MB (default: 64).
    #[serde(default)]
    memory_limit_mb: Option<u64>,
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
                let has_year = chars
                    .windows(4)
                    .any(|w| w.iter().all(|c| c.is_ascii_digit()));
                if !has_year {
                    query = format!("{} {}", query.trim(), year);
                }
            }
        }

        let mut results = self
            .indexer
            .read()
            .await
            .search_bm25(&query, limit)
            .await
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
                .read()
                .await
                .search_vector(&params.query, limit)
                .await
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

    async fn research_one(&self, params: ResearchToolParams) -> Result<SearchResponse, String> {
        let limit = params.limit.unwrap_or(10).clamp(1, 100);
        let max_pages = params.max_pages.unwrap_or(50).clamp(1, 500);
        let delay_ms = params.delay.unwrap_or(1000).max(100) as u64;
        let hybrid = params.hybrid.unwrap_or(false);
        let include_graph = params.include_graph.unwrap_or(false);
        // Research defaults to returning full content inline to the AI agent.
        let include_content = params.include_content.unwrap_or(true);
        let follow_external = params.follow_external.unwrap_or(false);
        let min_depth = params.min_depth.unwrap_or(0);
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
                let has_year = chars
                    .windows(4)
                    .any(|w| w.iter().all(|c| c.is_ascii_digit()));
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

        // Always generate embeddings for the knowledge graph / vector DB.
        let embedder: Result<Arc<dyn Embedder>, String> = FastembedEmbedder::new()
            .map(|e| Arc::new(e) as Arc<dyn Embedder>)
            .map_err(|e| format!("failed to load embedder: {}", e));
        let embedder = embedder?;

        let background_worker = Arc::new(BackgroundWorker::new(
            graph.clone(),
            Some(embedder.clone()),
            256,
        ));
        let auto_discover = params.seed.is_none();
        let mut all_seeds: Vec<String> = Vec::new();
        if let Some(ref seed) = params.seed {
            all_seeds.push(seed.clone());
        }
        all_seeds.extend(seeds);

        if all_seeds.is_empty() {
            // Auto-discover seeds from WebFind's own index, graph, and query-derived candidates.
            let indexer = self.indexer.read().await;
            let discovered = crate::engine::discovery::discover_seeds(
                indexer.as_ref(),
                Some(graph.as_ref()),
                &query,
            )
            .await
            .map_err(|e| e.to_string())?;
            drop(indexer);
            if discovered.is_empty() {
                return Err(
                    "No seeds discovered for the query. Provide a seed URL or rephrase the query."
                        .to_string(),
                );
            }
            all_seeds = discovered;
        }

        // When auto-discovering, always follow external links to escape seed domains.
        let follow_external = if auto_discover { true } else { follow_external };

        let proxy_pool = ProxyPool::new();
        if let Some(ref proxies_str) = params.proxies {
            for url in proxies_str.split(',') {
                let url = url.trim().to_string();
                if !url.is_empty() {
                    let ep = ProxyEndpoint::from_url(&url).map_err(|e| e.to_string())?;
                    proxy_pool.add(ep).map_err(|e| e.to_string())?;
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
            if follow_external {
                FollowExternalLinks::Follow
            } else {
                FollowExternalLinks::Ignore
            },
            min_depth,
            max_depth,
        )
        .with_respect_robots(RespectRobots::Yes)
        .with_graph_store(graph.clone())
        .with_background_worker(background_worker.clone())
        .with_topics(topics);

        // Multi-seed: crawl all seeds, merge and deduplicate.
        let mut contents: Vec<StructuredContent> = Vec::new();
        let mut seen_urls: HashSet<String> = HashSet::new();
        for seed_url in &all_seeds {
            let crawled =
                tokio::time::timeout(std::time::Duration::from_secs(60), crawler.crawl(seed_url))
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

        {
            let mut indexer = self.indexer.write().await;
            if hybrid {
                *indexer = Arc::new(InMemorySearchEngine::with_embedder(embedder));
            }
            indexer
                .index_batch(&contents)
                .await
                .map_err(|e| e.to_string())?;
        }

        let indexer = self.indexer.read().await;
        let mut results = indexer
            .search_bm25(&query, limit)
            .await
            .map_err(|e| e.to_string())?;

        let vector_scores: Option<HashMap<String, f64>> = if hybrid {
            match indexer.search_vector(&query, limit).await {
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

        // Always attach full content for research responses.
        attach_content(&mut results, &contents);

        // Wait for background knowledge-graph + vector persistence to drain.
        background_worker.close().await;

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
        let text =
            serde_json::to_string_pretty(&response.to_llm_value()).map_err(|e| e.to_string())?;
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

    async fn fetch_one(&self, params: FetchToolParams) -> Result<serde_json::Value, String> {
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
                let ep = ProxyEndpoint::from_url(url).map_err(|e| e.to_string())?;
                pool.add(ep).map_err(|e| e.to_string())?;
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
                    let session_manager = SessionManager::new(StickySessions::Sticky);
                    let mut proxy_fetcher = Fetcher::new_human(
                        Some(pool),
                        Some(session_manager),
                        RotateUserAgent::Rotate,
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
            "entities": content.entities,
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

    /// Code Mode tool (FR-10): Execute JavaScript with WebFind functions in a sandboxed QuickJS runtime.
    /// Collapses N tools into a single `run()` tool — 99.9% token reduction per Cloudflare's Code Mode pattern.
    #[tool(
        description = "Execute JavaScript code with access to WebFind search, fetch, and research functions. Use for complex multi-step workflows. Returns the last expression's value as JSON."
    )]
    async fn webfind_run(
        &self,
        Parameters(params): Parameters<RunToolParams>,
    ) -> Result<CallToolResult, String> {
        let timeout_ms = params.timeout_ms.unwrap_or(30_000).clamp(1_000, 120_000);
        let memory_limit_mb = params.memory_limit_mb.unwrap_or(64).clamp(16, 512);

        // Initialize global state for synchronous JS functions
        init_code_mode_state(
            self.indexer.clone(),
            self.graph_store.clone(),
            self.data_dir.clone(),
        );
        let code = params.code.clone();

        // Execute in a blocking task to avoid blocking the async runtime, and
        // bound it with an outer timeout so a runaway JS loop cannot hold a
        // blocking thread indefinitely (FR-10 sandbox CPU limit).
        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            tokio::task::spawn_blocking(move || {
                // Create a QuickJS runtime with memory limits
                let runtime = Runtime::new().map_err(|e| format!("Failed to create JS runtime: {}", e))?;
                runtime.set_memory_limit((memory_limit_mb as usize) * 1024 * 1024);

            let context = rquickjs::Context::full(&runtime).map_err(|e| format!("Failed to create JS context: {}", e))?;

            context.with(|ctx| {
                // Inject search function (synchronous, uses block_on internally)
                fn search_impl(
                    _ctx: Ctx<'_>,
                    query: String,
                    limit: Option<u32>,
                ) -> rquickjs::Result<String> {
                    let state = get_code_mode_state();
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async move {
                        let limit = limit.unwrap_or(10).clamp(1, 100) as usize;
                        let mut results = state.indexer
                            .read()
                            .await
                            .search_bm25(&query, limit)
                            .await
                            .map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;

                        let request = SearchRequest {
                            query: query.clone(),
                            depth: SearchDepth::Standard,
                            limit: limit as u32,
                            output: OutputFormat::Json,
                            language: None,
                            date_range: None,
                            domains: None,
                            content_type: Some(ContentType::Any),
                            include_content: false,
                            include_graph: false,
                            include_keywords: false,
                            include_metrics: false,
                            hybrid: false,
                        };

                        let graph_scores: Option<HashMap<String, f64>> = match &state.graph_store {
                            Some(store) => {
                                let cache = PageRankCache::new(&state.data_dir);
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

                        let ranker = Ranker::new();
                        results = ranker.rank(results, &request, graph_scores.as_ref(), None);
                        results.truncate(limit);

                        let json_results: Vec<serde_json::Value> = results
                            .into_iter()
                            .map(|r| serde_json::json!({
                                "rank": r.rank,
                                "url": r.url,
                                "title": r.title,
                                "snippet": r.snippet,
                                "domain": r.domain,
                                "score": r.score,
                            }))
                            .collect();

                        Ok(serde_json::to_string(&json_results).unwrap())
                    })
                }

                let search_fn = Function::new(ctx.clone(), search_impl).map_err(|e| format!("Failed to create search function: {}", e))?;
                ctx.globals().set("search", search_fn).map_err(|e| format!("Failed to set search global: {}", e))?;

                // Inject fetch function
                fn fetch_impl(
                    _ctx: Ctx<'_>,
                    url: String,
                ) -> rquickjs::Result<String> {
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async move {
                        let fetcher = Fetcher::new().map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;
                        let content = fetcher.fetch_url(&url).await.map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;
                        Ok(serde_json::to_string(&serde_json::json!({
                            "url": content.url,
                            "title": content.title,
                            "excerpt": content.excerpt,
                            "content_text": content.content_text,
                            "content_markdown": content.content_markdown,
                            "word_count": content.word_count,
                            "language": content.language,
                        })).unwrap())
                    })
                }

                let fetch_fn = Function::new(ctx.clone(), fetch_impl).map_err(|e| format!("Failed to create fetch function: {}", e))?;
                ctx.globals().set("fetch", fetch_fn).map_err(|e| format!("Failed to set fetch global: {}", e))?;

                // Inject research function
                fn research_impl(
                    _ctx: Ctx<'_>,
                    query: String,
                    seed: Option<String>,
                    max_pages: Option<u32>,
                ) -> rquickjs::Result<String> {
                    let state = get_code_mode_state();
                    let rt = tokio::runtime::Handle::current();
                    rt.block_on(async move {
                        let max_pages = max_pages.unwrap_or(50).clamp(1, 500) as usize;
                        let embedder: Arc<dyn Embedder> = match FastembedEmbedder::new() {
                            Ok(e) => Arc::new(e),
                            Err(e) => {
                                tracing::warn!("fastembed unavailable, using dummy: {}", e);
                                Arc::new(crate::engine::embedder::DummyEmbedder)
                            }
                        };

                        let graph: Arc<dyn CrawlGraphStore + Send + Sync> = state.graph_store.as_ref()
                            .map(|s| s.clone())
                            .unwrap_or_else(|| Arc::new(InMemoryCrawlGraph::new()));

                        let background_worker = Arc::new(BackgroundWorker::new(graph.clone(), Some(embedder.clone()), 256));
                        let mut all_seeds = Vec::new();
                        if let Some(s) = seed { all_seeds.push(s); }

                        let proxy_pool = ProxyPool::new();
                        let crawler = BulkDomainCrawler::new(
                            proxy_pool, 100, 30, 1000, 1, max_pages,
                            FollowExternalLinks::Ignore, 0, 5,
                        )
                        .with_respect_robots(RespectRobots::Yes)
                        .with_graph_store(graph.clone())
                        .with_background_worker(background_worker.clone());

                        let mut contents = Vec::new();
                        let mut seen = HashSet::new();
                        for seed_url in &all_seeds {
                            let crawled = tokio::time::timeout(Duration::from_secs(60), crawler.crawl(seed_url))
                                .await
                                .map_err(|_| rquickjs::Error::new_resolving_message("crawl_timeout".to_string(), "WebFind", "crawl timeout".to_string()))?
                                .map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;
                            for c in crawled {
                                if seen.insert(c.url.clone()) { contents.push(c); }
                            }
                        }

                        if contents.is_empty() {
                            return Ok(serde_json::to_string(&serde_json::json!({"results": []})).unwrap());
                        }

                        let mut indexer_guard = state.indexer.write().await;
                        *indexer_guard = Arc::new(InMemorySearchEngine::with_embedder(embedder));
                        indexer_guard.index_batch(&contents).await.map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;
                        drop(indexer_guard);

                        let indexer_read = state.indexer.read().await;
                        let results = indexer_read.search_bm25(&query, 10).await.map_err(|e| rquickjs::Error::new_resolving_message(e.to_string(), "WebFind", e.to_string()))?;

                        let json_results: Vec<serde_json::Value> = results.into_iter().map(|r| serde_json::json!({
                            "rank": r.rank, "url": r.url, "title": r.title, "snippet": r.snippet, "domain": r.domain, "score": r.score
                        })).collect();

                        background_worker.close().await;
                        Ok(serde_json::to_string(&serde_json::json!({"results": json_results})).unwrap())
                    })
                }

                let research_fn = Function::new(ctx.clone(), research_impl).map_err(|e| format!("Failed to create research function: {}", e))?;
                ctx.globals().set("research", research_fn).map_err(|e| format!("Failed to set research global: {}", e))?;

                // Inject console.log for debugging
                let console_log = Function::new(ctx.clone(), |_ctx: Ctx<'_>, msg: String| {
                    tracing::info!("[CodeMode] {}", msg);
                    Ok::<_, rquickjs::Error>(())
                }).map_err(|e| format!("Failed to create console.log: {}", e))?;
                let console = rquickjs::Object::new(ctx.clone()).map_err(|e| format!("Failed to create console object: {}", e))?;
                console.set("log", console_log).map_err(|e| format!("Failed to set console.log: {}", e))?;
                ctx.globals().set("console", console).map_err(|e| format!("Failed to set console global: {}", e))?;

                // Execute the user code and capture the result
                let wrapped_code = format!(
                    "(async () => {{ const result = await (async () => {{ {} }})(); return typeof result === 'string' ? result : JSON.stringify(result); }})()",
                    code
                );

                let result_val: String = ctx.eval(wrapped_code.as_str()).catch(&ctx).map_err(|e| format!("Code execution error: {}", e))?;

                Ok(result_val)
            })
        }))
        .await;

        // `timeout` returns Ok(join_result) | Err(elapsed); the inner
        // spawn_blocking result is Result<String, String>.
        let output = match result {
            Ok(join_result) => join_result.map_err(|e| format!("Task join error: {}", e))?,
            Err(_elapsed) => {
                tracing::warn!("Code Mode execution timed out after {} ms", timeout_ms);
                return Err("Code execution timed out (CPU limit exceeded)".to_string());
            }
        };

        match output {
            Ok(o) => Ok(CallToolResult::success(vec![ContentBlock::text(o)])),
            Err(e) => Err(e),
        }
    }
}
