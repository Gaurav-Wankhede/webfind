use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;

use webfind::cli::GraphStoreArg;
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::engine::embedder::{DummyEmbedder, Embedder, FastembedEmbedder};
use webfind::engine::graph_summary::build_graph_summary;
use webfind::engine::indexer::build_metadata;
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::report::format_response;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::{
    ContentBlock, IndexFreshness, ScoreBreakdown, SearchMetadata, SearchResponse, SearchResult,
};
use webfind::storage::turso_store::TursoStore;

/// Search the embedded Turso graph store with hybrid (BM25 + vector + graph)
/// RRF fusion. Used when `--graph-store turso` is selected; requires no
/// `index_data` directory.
#[allow(clippy::too_many_arguments)]
async fn turso_search(
    cfg: &webfind::config::WebfindConfig,
    query: &str,
    depth: webfind::cli::DepthArg,
    limit: u32,
    output: webfind::cli::OutputArg,
    include_content: bool,
    turso_path: Option<String>,
    hybrid: bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
    let store = TursoStore::new(&turso_path)
        .await
        .with_context(|| format!("open Turso graph store at `{turso_path}`"))?;
    // Ensure the derived indexes are current so search returns full signals.
    store.rebuild_fts().await.context("rebuild FTS index")?;
    store
        .compute_pagerank(20, 0.85)
        .await
        .context("compute PageRank")?;

    // Embed the query for the vector signal (fall back to the deterministic
    // dummy embedder when the ONNX model is unavailable).
    let embedding: Option<Vec<f32>> = if hybrid {
        match FastembedEmbedder::new() {
            Ok(e) => e.embed(&[query]).ok().and_then(|v| v.into_iter().next()),
            Err(e) => {
                tracing::warn!("fastembed unavailable, using dummy embedder: {e}");
                DummyEmbedder
                    .embed(&[query])
                    .ok()
                    .and_then(|v| v.into_iter().next())
            }
        }
    } else {
        None
    };

    let hits = store
        .search(query, embedding.as_deref(), limit.max(1) as usize)
        .await
        .context("hybrid search in Turso store")?;

    let mut signals: Vec<String> = Vec::new();
    for h in &hits {
        for s in &h.signals {
            if !signals.contains(s) {
                signals.push(s.clone());
            }
        }
    }
    if signals.is_empty() {
        signals.push("bm25".to_string());
    }

    let results: Vec<SearchResult> = hits
        .into_iter()
        .enumerate()
        .map(|(i, h)| {
            let domain = url::Url::parse(&h.url)
                .map(|u| u.host_str().unwrap_or("").to_string())
                .unwrap_or_default();
            let has_vec = h.signals.contains(&"vector".to_string());
            let has_graph = h.signals.contains(&"graph".to_string());
            SearchResult {
                rank: (i + 1) as u32,
                url: h.url.clone(),
                title: h.title,
                snippet: h.excerpt.clone(),
                domain,
                published_at: None,
                modified_at: None,
                crawled_at: Utc::now(),
                author: None,
                site_name: None,
                score: h.score,
                scores: ScoreBreakdown {
                    bm25: 0.0,
                    vector: has_vec.then_some(h.score),
                    graph: has_graph.then_some(h.score),
                    freshness: None,
                    quality: None,
                    final_score: h.score,
                },
                content: include_content.then(|| ContentBlock {
                    text: h.excerpt.clone(),
                    excerpt: h.excerpt.clone(),
                    word_count: 0,
                    reading_time_seconds: 0,
                    html: None,
                    markdown: None,
                }),
                keywords: None,
                metrics: None,
                favicon: None,
                thumbnail: None,
                language: "en".to_string(),
                content_type: "text/html".to_string(),
            }
        })
        .collect();

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query: query.to_string(),
        depth: SearchDepth::from(depth),
        total_results: results.len() as u64,
        returned: results.len() as u32,
        latency_ms: start.elapsed().as_millis() as u64,
        results,
        suggestions: vec![],
        related: vec![],
        graph: None,
        metadata: SearchMetadata {
            index_version: "turso".to_string(),
            index_size: store.get_urls().await.len() as u64,
            engine_version: "webfind-turso".to_string(),
            searched_at: Utc::now(),
            signals_used: signals,
            index_freshness: IndexFreshness {
                oldest_page: None,
                newest_page: None,
                avg_age_days: 0.0,
            },
        },
    };

    let formatted = format_response(&response, &OutputFormat::from(output));
    print!("{}", formatted);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    query: String,
    depth: webfind::cli::DepthArg,
    limit: u32,
    output: webfind::cli::OutputArg,
    _language: Option<String>,
    _domains: Option<String>,
    include_content: bool,
    include_graph: bool,
    include_keywords: bool,
    include_metrics: bool,
    graph_store: Option<GraphStoreArg>,
    turso_path: Option<String>,
    hybrid: bool,
) -> anyhow::Result<()> {
    let graph_store = webfind::config::resolve_graph_store(cfg, graph_store);

    // Turso-backed hybrid search: search the embedded graph store directly
    // instead of the Tantivy index. No `index_data` directory is required.
    if graph_store == GraphStoreArg::Turso {
        return turso_search(
            cfg,
            &query,
            depth,
            limit,
            output,
            include_content,
            turso_path,
            hybrid,
        )
        .await;
    }

    let path = crate::commands::index_path();

    if !path.exists() {
        eprintln!(
            "No index found at {}. Run 'webfind crawl' or 'webfind index import' first.",
            path.display()
        );
        std::process::exit(1);
    }

    let embedder: Option<Arc<dyn Embedder>> = if hybrid {
        Some(Arc::new(
            webfind::engine::embedder::FastembedEmbedder::new()
                .context("load embedding model for hybrid search")?,
        ))
    } else {
        None
    };
    let indexer: Arc<dyn SearchEngine + Send + Sync> = Arc::new(
        embedder
            .as_ref()
            .map(|e| InMemorySearchEngine::with_embedder(e.clone()))
            .unwrap_or_else(InMemorySearchEngine::new),
    );
    let ranker = Ranker::new();

    let depth_enum = SearchDepth::from(depth);

    let start = Instant::now();
    let mut results = indexer.search_bm25(&query, limit as usize).await?;
    let search_ms = start.elapsed().as_millis() as u64;

    let request = SearchRequest {
        query: query.clone(),
        depth: depth_enum.clone(),
        limit,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: None,
        include_content,
        include_graph,
        include_keywords,
        include_metrics,
        hybrid,
    };

    let vector_scores: Option<HashMap<String, f64>> = if hybrid {
        Some(indexer.search_vector(&query, limit as usize).await?)
    } else {
        None
    };

    let data_dir = webfind::config::data_dir();
    let cache = webfind::engine::pagerank_cache::PageRankCache::new(&data_dir);
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
    let graph_store_arc: Arc<dyn webfind::engine::crawl_graph::CrawlGraphStore + Send + Sync> =
        crate::commands::build_graph_store(
            &graph_store,
            &turso_path,
            cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref()),
        )
        .await?;
    let graph_scores: Option<HashMap<String, f64>> = Some(
        cache
            .get_or_compute(graph_store_arc.as_ref(), 20, 0.85)
            .await?,
    );
    results = ranker.rank(
        results,
        &request,
        graph_scores.as_ref(),
        vector_scores.as_ref(),
    );

    let graph_summary = if include_graph {
        build_graph_summary(graph_store_arc.as_ref(), &results).await
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
    let total = results.len() as u64;
    let meta = build_metadata(&*indexer, signals).await?;

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query: query.clone(),
        depth: depth_enum,
        total_results: total,
        returned: results.len() as u32,
        latency_ms: search_ms,
        results,
        suggestions: vec![],
        related: vec![],
        graph: graph_summary,
        metadata: meta,
    };

    let formatted = format_response(&response, &OutputFormat::from(output));
    print!("{}", formatted);

    Ok(())
}
