use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use webfind::cli::GraphStoreArg;
use webfind::config::WebfindConfig;
use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::graph_summary::build_graph_summary;
use webfind::engine::indexer::{attach_content, build_metadata};
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::report::format_response;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::SearchResponse;
use webfind::storage::turso_store::TursoStore;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    cfg: &WebfindConfig,
    seed: Option<String>,
    query: String,
    max_pages: u32,
    delay: u32,
    _respect_robots: bool,
    proxies: Option<String>,
    hybrid: bool,
    limit: u32,
    include_graph: bool,
    include_content: bool,
    follow_external: bool,
    min_depth: u32,
    max_depth: u32,
    auto_depth: bool,
    topics: Option<String>,
    seeds: Option<String>,
    dynamic: bool,
    deep: bool,
    output: Option<std::path::PathBuf>,
    graph_store_arg: Option<GraphStoreArg>,
    turso_path: Option<String>,
) -> anyhow::Result<()> {
    // Determine the graph-store backend (default: Turso embedded DB) so every
    // crawled record is persisted into graph memory — the durable awareness
    // store agents search across. Pure CLI: no temp JSON, records live in the DB.
    let kind = webfind::config::resolve_graph_store(cfg, graph_store_arg);
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());

    // Build the embedder (KG + vector persistence). Degrade to a deterministic
    // dummy when the ONNX model is unavailable so research still persists.
    let embedder: Arc<dyn Embedder> = match FastembedEmbedder::new() {
        Ok(e) => Arc::new(e),
        Err(e) => {
            tracing::warn!("fastembed model unavailable, using dummy embedder: {}", e);
            Arc::new(webfind::engine::embedder::DummyEmbedder)
        }
    };

    // Open the graph store; Turso persists url_nodes + link_edges + page_content.
    let graph: Arc<dyn CrawlGraphStore + Send + Sync> = if kind == GraphStoreArg::Turso {
        let encryption_key = cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref());
        let store = match encryption_key {
            Some(key) => TursoStore::new_with_encryption(&turso_path, key)
                .await
                .context("open encrypted Turso graph store")?,
            None => TursoStore::new(&turso_path)
                .await
                .context("open Turso graph store")?,
        };
        Arc::new(store) as Arc<dyn CrawlGraphStore + Send + Sync>
    } else {
        Arc::new(InMemoryCrawlGraph::new()) as Arc<dyn CrawlGraphStore + Send + Sync>
    };

    eprintln!(
        "Graph store: {} ({})",
        kind.as_str(),
        if kind == GraphStoreArg::Turso {
            &turso_path
        } else {
            "in-memory"
        }
    );

    // A temporary in-memory indexer for seed auto-discovery (the durable index
    // is the graph store; results are also indexed below for BM25 ranking).
    let indexer: Arc<tokio::sync::RwLock<Arc<dyn SearchEngine + Send + Sync>>> =
        Arc::new(tokio::sync::RwLock::new(Arc::new(
            InMemorySearchEngine::with_embedder(embedder.clone()),
        )));

    // Run the full research crawl, persisting every record into the graph store
    // via the background worker. Reuses the same pipeline as the HTTP API so
    // CLI research and API research share behavior.
    let options = webfind::engine::research_service::ResearchOptions {
        query: query.clone(),
        seed,
        seeds,
        max_pages,
        delay_ms: delay,
        follow_external,
        min_depth,
        max_depth,
        auto_depth,
        topics,
        proxies,
        domain_filter: Vec::new(),
        dynamic,
        deep,
    };

    let start = Instant::now();
    let progress = |p: webfind::engine::research_service::ResearchProgress| {
        eprintln!(
            "  [{}] crawled {}/{} {}",
            p.stage,
            p.crawled,
            p.total,
            p.current_url.unwrap_or_default()
        );
    };

    let (contents, research_graph) = match webfind::engine::research_service::execute_research(
        indexer.clone(),
        Some(graph.clone()),
        Some(embedder.clone()),
        options,
        progress,
    )
    .await
    {
        Ok(c) => c,
        Err(webfind::engine::research_service::ResearchError::NoSeeds) => {
            anyhow::bail!(
                "No seeds discovered for the query. Provide a seed URL or rephrase the query."
            );
        }
        Err(e) => anyhow::bail!("research failed: {}", e),
    };

    if contents.is_empty() {
        eprintln!("No content crawled for query: {}", query);
        return Ok(());
    }

    eprintln!(
        "Crawl + persist complete in {:.2}s ({} pages persisted to graph store)",
        start.elapsed().as_secs_f64(),
        contents.len()
    );

    // Index the newly crawled pages for BM25 (+ vector) ranking.
    {
        let mut indexer = indexer.write().await;
        let engine = if hybrid {
            InMemorySearchEngine::with_embedder(embedder)
        } else {
            InMemorySearchEngine::new()
        };
        *indexer = Arc::new(engine);
        indexer.index_batch(&contents).await?;
    }

    let indexer_guard = indexer.read().await;
    let mut results = indexer_guard.search_bm25(&query, limit as usize).await?;

    let request = SearchRequest {
        query: query.clone(),
        depth: SearchDepth::Standard,
        limit,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: None,
        include_content,
        include_graph,
        include_keywords: false,
        include_metrics: false,
        hybrid,
    };

    let vector_scores: Option<HashMap<String, f64>> = if hybrid {
        Some(indexer_guard.search_vector(&query, limit as usize).await?)
    } else {
        None
    };

    let ranker = Ranker::new();
    results = ranker.rank(results, &request, None, vector_scores.as_ref());

    if include_content {
        attach_content(&mut results, &contents);
    }

    let graph_summary = if include_graph {
        build_graph_summary(research_graph.as_ref(), &results).await
    } else {
        None
    };

    let mut signals = vec!["bm25".to_string()];
    if hybrid {
        signals.push("vector".to_string());
    }
    let meta = build_metadata(&**indexer_guard, signals).await?;

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query,
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

    // Write to a file when requested (agents read it natively with their
    // file-read tool) or to stdout otherwise.
    if let Some(path) = output {
        std::fs::write(&path, &body)
            .with_context(|| format!("write research output to {}", path.display()))?;
        eprintln!("Research result written to {}", path.display());
    } else {
        print!("{}", body);
    }
    Ok(())
}
