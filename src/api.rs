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
use tokio_util::sync::CancellationToken;
use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;

use crate::engine::bulk_crawler::BulkDomainCrawler;
use crate::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
use crate::engine::fingerprint::FingerprintAuditLog;
use crate::engine::graph_summary::build_graph_summary;
use crate::engine::indexer::Indexer;
use crate::engine::pagerank_cache::PageRankCache;
use crate::engine::proxy_pool::ProxyPool;
use crate::engine::ranker::Ranker;
use crate::mcp::WebfindMcpServer;
use crate::report::format_response;
use crate::schema::content::StructuredContent;
use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
use crate::schema::response::SearchResponse;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};

/// Shared application state for the HTTP API.
pub struct ApiState {
    indexer: Arc<Mutex<Indexer>>,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
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
    /// Enable BM25 + vector hybrid re-ranking.
    #[serde(default)]
    pub hybrid: bool,
}

#[derive(Debug, Deserialize)]
pub struct ResearchParams {
    pub seed: String,
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
    3
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
    let size = state.indexer.lock().await.doc_count().unwrap_or(0);
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

    let results = match state.indexer.lock().await.search_bm25(query, limit) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
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
        include_content: false,
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
        match state.indexer.lock().await.search_vector(query, limit) {
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
    let results = ranker.rank(
        results,
        &request,
        graph_scores.as_ref(),
        vector_scores.as_ref(),
    );

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
    let meta = match state.indexer.lock().await.metadata(signals) {
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
    // Input validation
    let seed = params.seed.trim();
    let query = params.q.trim();
    if seed.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "seed must not be empty"})),
        )
            .into_response();
    }
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
    let max_pages = params.max_pages.clamp(1, 500) as usize;
    let delay_ms = params.delay.max(100) as u64;

    // Crawl the seed without holding the indexer lock.
    let graph: Arc<dyn CrawlGraphStore + Send + Sync> = state
        .graph_store
        .clone()
        .map(|s| s as Arc<dyn CrawlGraphStore + Send + Sync>)
        .unwrap_or_else(|| Arc::new(InMemoryCrawlGraph::new()));

    let proxy_pool = ProxyPool::new();
    if let Some(ref proxies_str) = params.proxies {
        for url in proxies_str.split(',') {
            let url = url.trim().to_string();
            if !url.is_empty() {
                if let Ok(ep) = crate::engine::proxy_pool::ProxyEndpoint::from_url(&url) {
                    proxy_pool.add(ep);
                }
            }
        }
    }

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

    let crawler = BulkDomainCrawler::new(
        proxy_pool,
        100,
        30,
        delay_ms,
        1,
        max_pages,
        params.follow_external,
        params.min_depth,
        params.max_depth,
    )
    .with_respect_robots(true)
    .with_graph_store(graph.clone())
    .with_topics(topics);

    // Multi-seed: crawl primary seed + additional seeds, merge and deduplicate.
    let mut all_seeds = vec![seed.to_string()];
    if let Some(ref seeds_str) = params.seeds {
        all_seeds.extend(
            seeds_str
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty()),
        );
    }
    let mut contents: Vec<StructuredContent> = Vec::new();
    let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
    for seed_url in &all_seeds {
        match tokio::time::timeout(Duration::from_secs(60), crawler.crawl(seed_url)).await {
            Ok(Ok(crawled)) => {
                for c in crawled {
                    if seen_urls.insert(c.url.clone()) {
                        contents.push(c);
                    }
                }
            }
            Ok(Err(e)) => {
                return (
                    StatusCode::BAD_GATEWAY,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
                    .into_response();
            }
            Err(_) => {
                return (
                    StatusCode::GATEWAY_TIMEOUT,
                    Json(serde_json::json!({
                        "error": "crawl timed out after 60s",
                        "seed": seed_url,
                        "max_pages": max_pages,
                    })),
                )
                    .into_response();
            }
        }
    }

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
        if let Err(e) = indexer.index_batch(&contents) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
                .into_response();
        }
    }

    // Search the index.
    let indexer = state.indexer.lock().await;
    let results = match indexer.search_bm25(query, limit) {
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
        match indexer.search_vector(query, limit) {
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
        include_content: params.include_content,
        include_graph: params.include_graph,
        include_keywords: false,
        include_metrics: false,
        hybrid: params.hybrid,
    };

    let ranker = Ranker::new();
    let mut results = ranker.rank(results, &request, None, vector_scores.as_ref());

    if params.include_content {
        Indexer::attach_content(&mut results, &contents);
    }

    let graph_summary = if params.include_graph {
        build_graph_summary(graph.as_ref(), &results).await
    } else {
        None
    };

    let mut signals = vec!["bm25".to_string()];
    if params.hybrid {
        signals.push("vector".to_string());
    }

    let meta = match indexer.metadata(signals) {
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

pub fn app(
    indexer: Indexer,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
    rate_limit: Option<NonZeroU32>,
) -> Router {
    let state = Arc::new(ApiState {
        indexer: Arc::new(Mutex::new(indexer)),
        graph_store,
        audit_store,
        data_dir: data_dir.clone(),
    });

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
        let governor_conf = GovernorConfigBuilder::default()
            .per_second(rps.get().into())
            .burst_size(rps.get())
            .finish()
            .expect("valid governor config");
        router = router.layer(GovernorLayer {
            config: Arc::new(governor_conf),
        });
    }

    router
}

pub async fn run_server(
    indexer: Indexer,
    graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>>,
    data_dir: PathBuf,
    rate_limit: Option<NonZeroU32>,
    port: u16,
) -> anyhow::Result<()> {
    let app = app(indexer, graph_store, audit_store, data_dir, rate_limit);
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
