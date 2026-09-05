use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use askama::Template;
use axum::response::sse::Event;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Sse},
};
use dashmap::DashMap;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::engine::embedder::FastembedEmbedder;
use crate::engine::indexer::attach_content;
use crate::engine::ranker::Ranker;
use crate::engine::research_service::{ResearchOptions, ResearchProgress, execute_research};
use crate::engine::search_engine::InMemorySearchEngine;
use crate::engine::search_engine::SearchEngine;
use crate::gui::state::GuiState;
use crate::gui::templates;
use crate::schema::content::StructuredContent;
use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
use crate::schema::response::{ScoreBreakdown, SearchResult};

/// Cache key for GUI research results: (query, max_pages, seed, categories).
type QueryCacheKey = (String, u32, Option<String>, Vec<String>);

/// In-memory cache for GUI research results.
struct CachedResults {
    results: Vec<SearchResult>,
    cached_at: Instant,
}

static QUERY_CACHE: std::sync::LazyLock<DashMap<QueryCacheKey, CachedResults>> =
    std::sync::LazyLock::new(DashMap::new);

const CACHE_TTL: Duration = Duration::from_secs(3600);

#[derive(Debug, Deserialize)]
pub struct SearchForm {
    pub q: String,
    #[serde(default = "default_max_pages")]
    pub max_pages: u32,
    #[serde(default)]
    pub force: u32,
    #[serde(default)]
    pub seed: Option<String>,
    /// Comma-separated category filter (domain or topic tag names).
    #[serde(default)]
    pub categories: Option<String>,
    /// If "1", only include the specified categories in the response.
    #[serde(default)]
    pub filter_categories: Option<String>,
    /// Content-type filter: "text", "images", "news", "videos", "documentation"
    #[serde(rename = "type")]
    pub r#type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SuggestParams {
    pub q: String,
}

#[derive(Debug, Deserialize)]
pub struct VisitParams {
    pub url: String,
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub rank: u32,
}

fn default_max_pages() -> u32 {
    10
}

fn clamp_max_pages(n: u32) -> u32 {
    n.clamp(5, 100)
}

/// Render a template to HTML, falling back to a plain error page on failure.
fn render<T: Template>(template: T) -> Html<String> {
    match template.render() {
        Ok(html) => Html(html),
        Err(e) => {
            tracing::error!("template render failed: {}", e);
            Html(
                "<p class=\"text-red-400 p-4\">Failed to render page. Please try again.</p>"
                    .to_string(),
            )
        }
    }
}

/// Home page with the big search box.
pub async fn home(State(_state): State<GuiState>) -> impl IntoResponse {
    render(templates::HomeTemplate {
        query: String::new(),
        seed: String::new(),
    })
}

/// About page.
pub async fn about(State(_state): State<GuiState>) -> impl IntoResponse {
    render(templates::AboutTemplate)
}

/// Search results page.
///
/// If fresh cached results exist, renders them immediately. Otherwise renders the
/// page shell with an SSE connection that will stream research progress and the
/// final result list.
pub async fn search(
    State(_state): State<GuiState>,
    Query(params): Query<SearchForm>,
) -> impl IntoResponse {
    let query = params.q.trim().to_string();
    let max_pages = clamp_max_pages(params.max_pages);
    let seed_input = params.seed.as_ref().map(|s| s.trim().to_string());
    let seed = seed_input.clone().unwrap_or_default();
    let categories: Vec<String> = params
        .categories
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if query.is_empty() {
        return render(templates::HomeTemplate { query, seed });
    }

    let seed_opt = seed_input.filter(|s| !s.is_empty());
    let cache_key = (
        query.clone(),
        max_pages,
        seed_opt.clone(),
        categories.clone(),
    );

    let cached = QUERY_CACHE
        .get(&cache_key)
        .filter(|c| c.cached_at.elapsed() < CACHE_TTL && params.force == 0)
        .map(|c| c.results.clone());
    let active_type = params.r#type.as_deref().unwrap_or("all").to_string();
    let is_all_type = active_type == "all" || active_type.is_empty();
    let categories_str = categories.join(",");

    match cached {
        Some(results) => {
            let results = filter_by_type(results, &active_type);
            render(templates::SearchTemplate {
                query,
                max_pages,
                seed,
                categories: categories.clone(),
                categories_str: categories_str.clone(),
                active_type,
                is_all_type,
                cached: true,
                results,
            })
        }
        None => render(templates::SearchTemplate {
            query,
            max_pages,
            seed,
            categories,
            categories_str,
            active_type,
            is_all_type,
            cached: false,
            results: Vec::new(),
        }),
    }
}

/// Autocomplete suggestions based on the current query.
///
/// Uses the query log service for next-word prediction (prefix completion
/// from historical queries). Falls back to BM25 title search when the
/// query log is not available.
pub async fn suggest(
    State(state): State<GuiState>,
    Query(params): Query<SuggestParams>,
) -> impl IntoResponse {
    let query = params.q.trim();
    if query.is_empty() || query.len() > 200 {
        return render(templates::SuggestionsTemplate {
            suggestions: Vec::new(),
        });
    }

    let suggestions = if let Some(qlog) = state.query_log.as_ref() {
        match qlog.get_suggestions(query, 8).await {
            Ok(suggestions) => suggestions.into_iter().map(|s| s.text).collect(),
            Err(e) => {
                tracing::warn!("query log suggestions failed: {}", e);
                Vec::new()
            }
        }
    } else {
        let indexer = state.indexer.read().await;
        match indexer.search_bm25(query, 5).await {
            Ok(results) => results.into_iter().map(|r| r.title).collect(),
            Err(e) => {
                tracing::warn!("suggestion search failed: {}", e);
                Vec::new()
            }
        }
    };

    render(templates::SuggestionsTemplate { suggestions })
}

/// Get the top categories available for filtering. Returns a fragment
/// that HTMX can swap into the categories sidebar. Accepts an optional
/// `q` parameter to preserve the current query when clicking a category.
pub async fn categories(
    State(state): State<GuiState>,
    Query(params): Query<SuggestParams>,
) -> impl IntoResponse {
    let categories = if let Some(cat) = state.categories.as_ref() {
        match cat.top_categories(30).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("categories failed: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };

    render(templates::CategoriesTemplate {
        categories,
        current_query: params.q.trim().to_string(),
    })
}

/// Record a click-through and redirect to the target URL.
pub async fn visit(Query(params): Query<VisitParams>) -> Result<Redirect, StatusCode> {
    let target = params.url.trim();
    if target.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }

    // Validate that the URL is HTTP(S) to avoid open redirects.
    match url::Url::parse(target) {
        Ok(u) if matches!(u.scheme(), "http" | "https") => {
            tracing::info!(
                visit.url = target,
                visit.query = params.q,
                visit.rank = params.rank,
                "gui click-through"
            );
            Ok(Redirect::temporary(target))
        }
        _ => Err(StatusCode::BAD_REQUEST),
    }
}

/// Server-Sent Events endpoint that performs the live research crawl and streams
/// progress / final results back to the browser.
pub async fn research_stream(
    State(state): State<GuiState>,
    Query(params): Query<SearchForm>,
) -> Sse<ReceiverStream<Result<Event, std::convert::Infallible>>> {
    let query = params.q.trim().to_string();
    let max_pages = clamp_max_pages(params.max_pages);
    let seed = params.seed.as_ref().and_then(|s| {
        let s = s.trim();
        if s.is_empty() {
            None
        } else {
            Some(s.to_string())
        }
    });
    let categories: Vec<String> = params
        .categories
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|c| c.trim().to_string())
                .filter(|c| !c.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let cache_key = (query.clone(), max_pages, seed.clone(), categories.clone());
    let active_type = params.r#type.as_deref().unwrap_or("").to_string();

    let (tx, rx) = mpsc::channel::<Result<Event, std::convert::Infallible>>(32);

    // If cached, stream the cached results immediately and close.
    if let Some(cached) = QUERY_CACHE
        .get(&cache_key)
        .filter(|c| c.cached_at.elapsed() < CACHE_TTL && params.force == 0)
    {
        let results = filter_by_type(cached.results.clone(), &active_type);
        let query2 = query.clone();
        tokio::spawn(async move {
            let _ = send_cached(&tx, &query2, &results).await;
        });
        return Sse::new(ReceiverStream::new(rx));
    }

    tokio::spawn(async move {
        let start = Instant::now();
        if let Err(e) = run_research_job(
            state.clone(),
            query.clone(),
            max_pages,
            seed,
            categories,
            active_type,
            tx.clone(),
        )
        .await
        {
            let _ = tx
                .send(Ok(Event::default()
                    .event("error")
                    .data(format!("Research failed: {}", e))))
                .await;
        }
        let _ = tx
            .send(Ok(Event::default().event("close").data("done")))
            .await;

        // Log the query for future autocomplete suggestions.
        if let Some(qlog) = state.query_log.as_ref() {
            let latency_ms = start.elapsed().as_millis() as u64;
            let session_id = "gui"; // Single-session for now; cookie-based later
            if let Err(e) = qlog.log_query(&query, session_id, 0, latency_ms).await {
                tracing::warn!("failed to log query: {}", e);
            }
        }
    });

    Sse::new(ReceiverStream::new(rx))
}

async fn send_cached(
    tx: &mpsc::Sender<Result<Event, std::convert::Infallible>>,
    query: &str,
    results: &[SearchResult],
) -> Result<(), mpsc::error::SendError<Result<Event, std::convert::Infallible>>> {
    tx.send(Ok(Event::default()
        .event("progress")
        .data(r#"{"stage":"cached","crawled":0,"total":0}"#)))
        .await?;

    let html = match (templates::ResultListTemplate { query, results }.render()) {
        Ok(h) => h,
        Err(e) => {
            tracing::error!("failed to render cached results: {}", e);
            return tx
                .send(Ok(Event::default()
                    .event("error")
                    .data("Failed to render results")))
                .await;
        }
    };

    tx.send(Ok(Event::default().event("result").data(html)))
        .await?;
    Ok(())
}

async fn run_research_job(
    state: GuiState,
    query: String,
    max_pages: u32,
    seed: Option<String>,
    categories: Vec<String>,
    active_type: String,
    tx: mpsc::Sender<Result<Event, std::convert::Infallible>>,
) -> Result<(), String> {
    let cache_seed = seed.clone();
    let options = ResearchOptions {
        query: query.clone(),
        seed,
        seeds: None,
        max_pages,
        delay_ms: 1000,
        follow_external: true,
        min_depth: 0,
        max_depth: 5,
        auto_depth: true,
        topics: None,
        proxies: None,
        domain_filter: categories.clone(),
        dynamic: false,
        deep: false,
    };

    let tx_progress = tx.clone();
    let progress = move |p: ResearchProgress| {
        let tx = tx_progress.clone();
        let data = serde_json::json!({
            "stage": p.stage,
            "crawled": p.crawled,
            "total": p.total,
            "current_url": p.current_url,
        });
        let event = Event::default().event("progress").data(data.to_string());
        let _ = tx.try_send(Ok(event));
    };

    let (contents, _research_graph) = execute_research(
        state.indexer.clone(),
        state.graph_store.clone(),
        None,
        options,
        progress,
    )
    .await
    .map_err(|e| e.to_string())?;

    let results = rank_contents(&query, contents).await?;

    // Cache the ranked results (unfiltered so subsequent type filters hit cache).
    QUERY_CACHE.insert(
        (
            query.clone(),
            max_pages,
            cache_seed.clone(),
            categories.clone(),
        ),
        CachedResults {
            results: results.clone(),
            cached_at: Instant::now(),
        },
    );

    let filtered = filter_by_type(results, &active_type);

    let html = (templates::ResultListTemplate {
        query: &query,
        results: &filtered,
    })
    .render()
    .map_err(|e| format!("render results: {}", e))?;

    tx.send(Ok(Event::default().event("result").data(html)))
        .await
        .map_err(|_| "client disconnected".to_string())?;

    Ok(())
}

/// Index freshly crawled content, search it, and rank the results.
async fn rank_contents(
    query: &str,
    contents: Vec<StructuredContent>,
) -> Result<Vec<SearchResult>, String> {
    if contents.is_empty() {
        return Ok(Vec::new());
    }

    let embedder = match FastembedEmbedder::new() {
        Ok(e) => Some(Arc::new(e) as Arc<dyn crate::engine::embedder::Embedder + Send + Sync>),
        Err(e) => {
            tracing::warn!("failed to load embedder for GUI ranking: {}", e);
            None
        }
    };

    let engine = match embedder {
        Some(e) => InMemorySearchEngine::with_embedder(e),
        None => InMemorySearchEngine::new(),
    };
    let indexer = Arc::new(engine);

    indexer
        .index_batch(&contents)
        .await
        .map_err(|e| format!("index batch: {}", e))?;

    let mut results = indexer
        .search_bm25(query, 10)
        .await
        .map_err(|e| format!("bm25 search: {}", e))?;

    let vector_scores: Option<HashMap<String, f64>> = match indexer.search_vector(query, 10).await {
        Ok(scores) => Some(scores),
        Err(e) => {
            tracing::warn!("vector search failed: {}", e);
            None
        }
    };

    let request = SearchRequest {
        query: query.to_string(),
        depth: SearchDepth::Standard,
        limit: 10,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: Some(ContentType::Any),
        include_content: false,
        include_graph: false,
        include_keywords: false,
        include_metrics: false,
        hybrid: vector_scores.is_some(),
    };

    let ranker = Ranker::new();
    results = ranker.rank(results, &request, None, vector_scores.as_ref());
    attach_content(&mut results, &contents);

    Ok(results)
}

/// Filter search results by content type label.
/// If `active_type` is empty or "all", returns all results unchanged.
fn filter_by_type(results: Vec<SearchResult>, active_type: &str) -> Vec<SearchResult> {
    if active_type.is_empty() || active_type == "all" {
        return results;
    }
    results
        .into_iter()
        .filter(|r| r.content_type == active_type)
        .collect()
}
