use std::collections::HashMap;
use std::net::SocketAddr;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::get,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;

use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::embedder::{Embedder, FastembedEmbedder};
use crate::engine::fingerprint::FingerprintAuditLog;
use crate::engine::fetcher::Fetcher;
use crate::engine::graph_summary::build_graph_summary;
use crate::engine::indexer::Indexer;
use crate::engine::pagerank_cache::PageRankCache;
use crate::engine::proxy_pool::ProxyPool;
use crate::engine::ranker::Ranker;
use crate::mcp::WebfindMcpServer;
use crate::report::format_response;
use crate::schema::content::StructuredContent;
use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
use crate::schema::response::{ScoreBreakdown, SearchResponse, SearchResult};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

/// Shared application state for the HTTP API.
pub struct ApiState {
    indexer: Arc<Mutex<Indexer>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
    query_log: Option<Arc<crate::engine::query_log::QueryLogService>>,
    categories: Option<Arc<crate::engine::categories::CategoryService>>,
}

impl ApiState {
    pub fn new(
        indexer: Indexer,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            indexer: Arc::new(Mutex::new(indexer)),
            graph_store,
            audit_store,
            data_dir,
            query_log: None,
            categories: None,
        }
    }

    pub fn with_query_log(
        mut self,
        query_log: Option<Arc<crate::engine::query_log::QueryLogService>>,
    ) -> Self {
        self.query_log = query_log;
        self
    }

    pub fn with_categories(
        mut self,
        categories: Option<Arc<crate::engine::categories::CategoryService>>,
    ) -> Self {
        self.categories = categories;
        self
    }

    pub fn indexer(&self) -> Arc<Mutex<Indexer>> {
        self.indexer.clone()
    }

    pub fn graph_store(&self) -> Option<Arc<dyn CrawlGraphStore + Send + Sync>> {
        self.graph_store.clone()
    }

    pub fn data_dir(&self) -> &PathBuf {
        &self.data_dir
    }

    pub fn query_log(&self) -> Option<Arc<crate::engine::query_log::QueryLogService>> {
        self.query_log.clone()
    }

    pub fn categories(&self) -> Option<Arc<crate::engine::categories::CategoryService>> {
        self.categories.clone()
    }
}

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub include_graph: bool,
    /// Return full page content for each result.
    #[serde(default)]
    pub include_content: bool,
    /// Comma-separated proxy URLs to route re-fetch requests through.
    #[serde(default)]
    pub proxies: Option<String>,
    /// Enable BM25 + vector hybrid re-ranking.
    #[serde(default)]
    pub hybrid: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResearchParams {
    /// Seed URL to crawl. If omitted, WebFind auto-discovers seeds from its index, graph, and query-derived candidates.
    #[serde(default)]
    pub seed: Option<String>,
    pub q: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    #[serde(default = "default_max_pages")]
    pub max_pages: u32,
    #[serde(default = "default_delay")]
    pub delay: u32,
    #[serde(default)]
    pub hybrid: bool,
    #[serde(default)]
    pub include_graph: bool,
    #[serde(default)]
    pub include_content: bool,
    /// Comma-separated proxy URLs to route crawl requests through.
    #[serde(default)]
    pub proxies: Option<String>,
    /// Follow external (cross-domain) links during crawl.
    #[serde(default)]
    pub follow_external: bool,
    /// Force-fetch URLs at or below this hop depth from the seed (default 3).
    #[serde(default = "default_min_depth")]
    pub min_depth: u32,
    /// Maximum discovery depth — stop following links beyond this hop count (default 5).
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Query topics for content-aware link prioritization (comma-separated).
    #[serde(default)]
    pub topics: Option<String>,
    /// Additional seed URLs for multi-seed crawling (comma-separated).
    #[serde(default)]
    pub seeds: Option<String>,
}

fn default_limit() -> u32 {
    10
}

fn default_max_pages() -> u32 {
    50
}

fn default_delay() -> u32 {
    1000
}

fn default_min_depth() -> u32 {
    0
}

fn default_max_depth() -> u32 {
    5
}

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: &'static str,
    pub version: &'static str,
    pub index_size: u64,
}

pub async fn health(State(state): State<Arc<ApiState>>) -> impl IntoResponse {
    let size = state.indexer.lock().await.doc_count().await.unwrap_or(0);
    Json(HealthResponse {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        index_size: size,
    })
}

pub async fn search(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    // /search: find URLs already present in the SurrealDB knowledge graph, then
    // re-fetch those URLs from the internet to return fresh content. It does NOT
    // discover or crawl new URLs.
    // Input validation
    let query = params.q.trim();
    if query.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "query must not be empty"})),
        )
            .into_response();
    }
    if query.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "query must be 200 characters or fewer"})),
        )
            .into_response();
    }
    if params.limit == 0 || params.limit > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "limit must be between 1 and 100"})),
        )
            .into_response();
    }

    let start = Instant::now();
    let limit = params.limit as usize;

    // 1. Find known URLs in SurrealDB that match the query.
    let mut db_results = match state.indexer.lock().await.search_bm25(query, limit).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    // 2. Re-fetch the matched URLs from the internet for fresh content.
    let proxy_pool = ProxyPool::new();
    if let Some(ref proxies_str) = params.proxies {
        for url in proxies_str.split(',') {
            let url = url.trim();
            if !url.is_empty() {
                if let Ok(ep) = crate::engine::proxy_pool::ProxyEndpoint::from_url(url) {
                    proxy_pool.add(ep);
                }
            }
        }
    }

    let fetcher = if proxy_pool.is_empty() {
        match Fetcher::new() {
            Ok(f) => f,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("failed to build fetcher: {}", e)})),
                )
                    .into_response();
            }
        }
    } else {
        let proxy_url = match proxy_pool.select(None) {
            Some(p) => p.url,
            None => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": "proxy pool is empty"})),
                )
                    .into_response();
            }
        };
        let proxy = match reqwest::Proxy::all(&proxy_url) {
            Ok(p) => p,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("invalid proxy: {}", e)})),
                )
                    .into_response();
            }
        };
        let client = match reqwest::Client::builder()
            .proxy(proxy)
            .timeout(Duration::from_secs(10))
            .build()
        {
            Ok(c) => c,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("failed to build client: {}", e)})),
                )
                    .into_response();
            }
        };
        match Fetcher::from_client(client) {
            Ok(f) => f,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": format!("failed to build fetcher: {}", e)})),
                )
                    .into_response();
            }
        }
    };
    let fetcher = Arc::new(fetcher);

    let semaphore = Arc::new(Semaphore::new(8));
    let mut fetch_tasks = Vec::with_capacity(db_results.len());
    for result in &db_results {
        let fetcher = fetcher.clone();
        let url = result.url.clone();
        let permit = semaphore.clone();
        fetch_tasks.push(tokio::spawn(async move {
            let _permit = permit.acquire().await.ok()?;
            match tokio::time::timeout(Duration::from_secs(10), fetcher.fetch_url(&url)).await {
                Ok(Ok(content)) if content.is_valid_content => Some(content),
                _ => None,
            }
        }));
    }

    let mut fresh_contents: Vec<StructuredContent> = Vec::new();
    for (i, task) in fetch_tasks.into_iter().enumerate() {
        if let Ok(Some(content)) = task.await {
            if content.is_valid_content {
                db_results[i].title = content.title.clone();
                db_results[i].snippet = content.excerpt.clone();
                db_results[i].crawled_at = content.fetched_at;
                db_results[i].author = content.author.clone();
                db_results[i].site_name = content.site_name.clone();
                if params.include_content {
                    db_results[i].content = Some(crate::schema::response::ContentBlock {
                        text: content.content_text.clone(),
                        excerpt: content.excerpt.clone(),
                        word_count: content.word_count.max(0) as u32,
                        reading_time_seconds: content.reading_time_seconds.max(0) as u32,
                        html: Some(content.content_html.clone()),
                        markdown: Some(content.content_markdown.clone()),
                    });
                }
                fresh_contents.push(content);
            }
        }
    }

    // Drop unfetchable results unless we have nothing left.
    let mut results: Vec<SearchResult> = db_results
        .into_iter()
        .filter(|r| !r.title.is_empty() || !r.snippet.is_empty() || r.content.is_some())
        .collect();
    if results.is_empty() && !fresh_contents.is_empty() {
        results = fresh_contents
            .iter()
            .enumerate()
            .map(|(i, c)| SearchResult {
                rank: (i + 1) as u32,
                url: c.url.clone(),
                title: c.title.clone(),
                snippet: c.excerpt.clone(),
                domain: url::Url::parse(&c.url)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default(),
                published_at: c.published_at,
                modified_at: c.modified_at,
                crawled_at: c.fetched_at,
                author: c.author.clone(),
                site_name: c.site_name.clone(),
                score: 0.0,
                scores: ScoreBreakdown {
                    bm25: 0.0,
                    vector: None,
                    graph: None,
                    freshness: None,
                    quality: None,
                    final_score: 0.0,
                },
                content: if params.include_content {
                    Some(crate::schema::response::ContentBlock {
                        text: c.content_text.clone(),
                        excerpt: c.excerpt.clone(),
                        word_count: c.word_count.max(0) as u32,
                        reading_time_seconds: c.reading_time_seconds.max(0) as u32,
                        html: Some(c.content_html.clone()),
                        markdown: Some(c.content_markdown.clone()),
                    })
                } else {
                    None
                },
                keywords: None,
                metrics: None,
                favicon: c.favicon.clone(),
                thumbnail: None,
                language: c.language.clone(),
                content_type: ContentType::Any,
            })
            .collect();
    }

    // 3. Re-rank using offline signals.
    let request = SearchRequest {
        query: query.to_string(),
        depth: SearchDepth::Standard,
        limit: params.limit,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: Some(ContentType::Any),
        include_content: params.include_content,
        include_graph: params.include_graph,
        include_keywords: false,
        include_metrics: false,
        hybrid: params.hybrid,
    };

    let graph_scores: Option<HashMap<String, f64>> = match &state.graph_store {
        Some(store) => {
            let cache = PageRankCache::new(&state.data_dir);
            match cache.get_or_compute(store.as_ref(), 20, 0.85).await {
                Ok(scores) => Some(scores),
                Err(e) => {
                    tracing::warn!("failed to load PageRank cache: {}", e);
                    None
                }
            }
        }
        None => None,
    };

    let vector_scores: Option<HashMap<String, f64>> = if params.hybrid {
        match state.indexer.lock().await.search_vector(query, limit).await {
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
    let mut results = ranker.rank(results, &request, graph_scores.as_ref(), vector_scores.as_ref());

    results.truncate(limit);

    let graph_summary = if params.include_graph {
        match &state.graph_store {
            Some(store) => build_graph_summary(store.as_ref(), &results).await,
            None => None,
        }
    } else {
        None
    };

    let mut signals = vec!["bm25".to_string()];
    if params.hybrid {
        signals.push("vector".to_string());
    }
    if graph_scores.is_some() {
        signals.push("graph".to_string());
    }
    let meta = match state.indexer.lock().await.metadata(signals).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query: query.to_string(),
        depth: SearchDepth::Standard,
        total_results: results.len() as u64,
        returned: results.len() as u32,
        latency_ms: start.elapsed().as_millis() as u64,
        results,
        suggestions: vec![],
        related: vec![],
        graph: graph_summary,
        metadata: meta,
    };

    let output_format = match params.output.as_deref() {
        Some("json") => OutputFormat::Json,
        Some("report") => OutputFormat::Report,
        Some("markdown") => OutputFormat::Markdown,
        _ => OutputFormat::Json,
    };

    let body = format_response(&response, &output_format);
    (StatusCode::OK, body).into_response()
}

pub async fn research(
    State(state): State<Arc<ApiState>>,
    Query(params): Query<ResearchParams>,
) -> impl IntoResponse {
    // /research: discover and crawl URLs that are NOT already present in the
    // SurrealDB knowledge graph, then persist the new content and return it.
    // Input validation
    let query = params.q.trim();
    if query.is_empty() || query.len() > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "query must be 1-200 characters"})),
        )
            .into_response();
    }
    if params.limit == 0 || params.limit > 100 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "limit must be between 1 and 100"})),
        )
            .into_response();
    }

    let start = Instant::now();
    let limit = params.limit as usize;
    let _max_pages = params.max_pages.clamp(1, 500) as usize;
    let _delay_ms = params.delay.max(100) as u64;

    // Always generate embeddings for the knowledge graph / vector DB and reuse
    // the same embedder for the in-memory vector engine when hybrid is enabled.
    let embedder: Arc<dyn Embedder> = match FastembedEmbedder::new() {
        Ok(e) => Arc::new(e),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": format!("failed to load embedder: {}", e)})),
            )
                .into_response();
        }
    };

    let options = crate::engine::research_service::ResearchOptions {
        query: query.to_string(),
        seed: params.seed.clone(),
        seeds: params.seeds.clone(),
        max_pages: params.max_pages,
        delay_ms: params.delay,
        follow_external: params.follow_external,
        min_depth: params.min_depth,
        max_depth: params.max_depth,
        topics: params.topics.clone(),
        proxies: params.proxies.clone(),
        domain_filter: Vec::new(),
    };

    let (contents, research_graph) = match crate::engine::research_service::execute_research(
        state.indexer.clone(),
        state.graph_store.clone(),
        Some(embedder.clone()),
        options,
        |_| {},
    )
    .await
    {
        Ok(c) => c,
        Err(crate::engine::research_service::ResearchError::NoSeeds) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "No seeds discovered for the query. Provide a seed URL or rephrase the query."})),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({"error": e.to_string() })),
            )
                .into_response();
        }
    };

    if contents.is_empty() {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "request_id": uuid::Uuid::new_v4().to_string(),
                "query": query,
                "total_results": 0,
                "returned": 0,
                "results": [],
                "latency_ms": start.elapsed().as_millis() as u64,
            })),
        )
            .into_response();
    }

    // Index the newly crawled pages using the shared indexer.
    {
        let mut indexer = state.indexer.lock().await;
        if params.hybrid {
            // Swap in a vector-enabled engine using the same embedder.
            let vector_engine =
                crate::engine::search_engine::InMemorySearchEngine::with_embedder(embedder);
            *indexer = crate::engine::indexer::Indexer::new(Arc::new(vector_engine));
        }
        if let Err(e) = indexer.index_batch(&contents).await {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    }

    // Search the index.
    let indexer = state.indexer.lock().await;
    let results = match indexer.search_bm25(query, limit).await {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let vector_scores: Option<HashMap<String, f64>> = if params.hybrid {
        match indexer.search_vector(query, limit).await {
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
        query: query.to_string(),
        depth: SearchDepth::Standard,
        limit: params.limit,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: Some(ContentType::Any),
        // Research always returns full content inline to the AI agent.
        include_content: true,
        include_graph: params.include_graph,
        include_keywords: false,
        include_metrics: false,
        hybrid: params.hybrid,
    };

    let ranker = Ranker::new();
    let mut results = ranker.rank(results, &request, None, vector_scores.as_ref());

    // Always attach full content for research responses.
    Indexer::attach_content(&mut results, &contents);

    // Fallback: if BM25 returned nothing, return the best crawled pages directly.
    if results.is_empty() && !contents.is_empty() {
        let request = SearchRequest {
            query: query.to_string(),
            depth: SearchDepth::Standard,
            limit: params.limit,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content: true,
            include_graph: false,
            include_keywords: false,
            include_metrics: false,
            hybrid: false,
        };
        results = contents
            .iter()
            .take(params.limit as usize)
            .enumerate()
            .map(|(i, c)| {
                let domain = url::Url::parse(&c.url)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default();
                SearchResult {
                    rank: (i + 1) as u32,
                    url: c.url.clone(),
                    title: c.title.clone(),
                    snippet: c.excerpt.clone(),
                    domain,
                    published_at: c.published_at,
                    modified_at: c.modified_at,
                    crawled_at: c.fetched_at,
                    author: c.author.clone(),
                    site_name: c.site_name.clone(),
                    score: 0.0,
                    scores: ScoreBreakdown {
                        bm25: 0.0,
                        vector: None,
                        graph: None,
                        freshness: None,
                        quality: None,
                        final_score: 0.0,
                    },
                    content: Some(crate::schema::response::ContentBlock {
                        text: c.content_text.clone(),
                        excerpt: c.excerpt.clone(),
                        word_count: c.word_count.max(0) as u32,
                        reading_time_seconds: c.reading_time_seconds.max(0) as u32,
                        html: Some(c.content_html.clone()),
                        markdown: Some(c.content_markdown.clone()),
                    }),
                    keywords: None,
                    metrics: None,
                    favicon: c.favicon.clone(),
                    thumbnail: None,
                    language: c.language.clone(),
                    content_type: ContentType::Any,
                }
            })
            .collect();
        let ranker = Ranker::new();
        results = ranker.rank(results, &request, None, None);
    }

    let graph_summary = if params.include_graph {
        build_graph_summary(research_graph.as_ref(), &results).await
    } else {
        None
    };

    let mut signals = vec!["bm25".to_string()];
    if params.hybrid {
        signals.push("vector".to_string());
    }

    let meta = match indexer.metadata(signals).await {
        Ok(m) => m,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    };

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query: query.to_string(),
        depth: SearchDepth::Standard,
        total_results: results.len() as u64,
        returned: results.len() as u32,
        latency_ms: start.elapsed().as_millis() as u64,
        results,
        suggestions: vec![],
        related: vec![],
        graph: graph_summary,
        metadata: meta,
    };

    let body = format_response(&response, &OutputFormat::Json);
    (StatusCode::OK, body).into_response()
}

pub fn app(state: Arc<ApiState>, rate_limit: Option<NonZeroU32>) -> Router {
    let mcp_state = state.clone();
    let mcp = StreamableHttpService::new(
        move || {
            Ok::<_, std::io::Error>(WebfindMcpServer::new(
                mcp_state.indexer.clone(),
                mcp_state.graph_store.clone(),
                mcp_state.audit_store.clone(),
                mcp_state.data_dir.clone(),
            ))
        },
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_stateful_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts()
            .with_cancellation_token(CancellationToken::new()),
    );

    let mut router = Router::new()
        .route("/health", get(health))
        .route("/search", get(search))
        .route("/research", get(research))
        .nest_service("/mcp", mcp)
        .with_state(state);

    // Optional per-IP rate limiting. Off by default so local LLM agents are not throttled.
    if let Some(rps) = rate_limit {
        if let Some(governor_conf) = GovernorConfigBuilder::default()
            .per_second(rps.get().into())
            .burst_size(rps.get())
            .finish()
        {
            router = router.layer(GovernorLayer {
                config: Arc::new(governor_conf),
            });
        } else {
            tracing::warn!("invalid governor config; rate limiting disabled");
        }
    }

    router
}

pub async fn run_server(
    state: Arc<ApiState>,
    rate_limit: Option<NonZeroU32>,
    port: u16,
) -> anyhow::Result<()> {
    let app = app(state, rate_limit);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
