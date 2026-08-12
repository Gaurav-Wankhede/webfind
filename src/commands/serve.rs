use std::sync::Arc;

use anyhow::Context;

use webfind::cli::GraphStoreArg;
use webfind::engine::categories::CategoryService;
use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::fingerprint::FingerprintAuditLog;
use webfind::engine::query_log::QueryLogService;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::storage::turso_store::TursoStore;

#[allow(clippy::too_many_arguments)]
pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    port: u16,
    transport: webfind::cli::TransportArg,
    graph_store: Option<GraphStoreArg>,
    turso_path: Option<String>,
    hybrid: bool,
    rate_limit: Option<u32>,
    gui_port: u16,
) -> anyhow::Result<()> {
    let kind = webfind::config::resolve_graph_store(cfg, graph_store);
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());

    let embedder: Option<Arc<dyn Embedder>> = if hybrid {
        Some(Arc::new(
            FastembedEmbedder::new().context("load embedding model for hybrid search")?,
        ))
    } else {
        None
    };

    // Graph store + Turso-backed auxiliary services (query log, categories,
    // fingerprint audit) derive from the embedded database when selected.
    let mut graph_store: Arc<dyn CrawlGraphStore + Send + Sync> =
        Arc::new(InMemoryCrawlGraph::new()) as Arc<dyn CrawlGraphStore + Send + Sync>;
    let mut audit_store: Option<Arc<dyn FingerprintAuditLog + Send + Sync>> = None;
    let mut query_log: Option<Arc<QueryLogService>> = None;
    let mut categories: Option<Arc<CategoryService>> = None;
    // Concrete Turso store handle used to enforce the storage budget. Present
    // only when the Turso backend is selected (the storage-tiering feature is a
    // no-op for the in-memory backend).
    let mut storage_store: Option<Arc<webfind::storage::turso_store::TursoStore>> = None;

    if kind == GraphStoreArg::Turso {
        let encryption_key = cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref());
        let store = match encryption_key {
            Some(key) => TursoStore::new_with_encryption(&turso_path, key)
                .await
                .context("open encrypted Turso graph store")?,
            None => TursoStore::new(&turso_path)
                .await
                .context("open Turso graph store")?,
        };
        let store = Arc::new(store);
        graph_store = store.clone() as Arc<dyn CrawlGraphStore + Send + Sync>;
        audit_store = Some(store.clone() as Arc<dyn FingerprintAuditLog + Send + Sync>);
        query_log = Some(Arc::new(
            QueryLogService::new(store.conn(), turso_path.clone()).await,
        ));
        categories = Some(Arc::new(CategoryService::new(store.conn())));
        storage_store = Some(store);
    }

    let indexer: Arc<dyn SearchEngine + Send + Sync> = Arc::new(
        embedder
            .map(|e| InMemorySearchEngine::with_embedder(e))
            .unwrap_or_else(InMemorySearchEngine::new),
    );

    let rate_limit = webfind::config::resolve_rate_limit(cfg, rate_limit);

    // Spawn a periodic storage-budget enforcer. When a budget is configured,
    // this keeps the embedded DB under the disk cap by evicting full content of
    // the oldest pages (preserving embeddings/excerpts). Runs for the lifetime
    // of the server for both transports.
    let max_bytes = webfind::config::resolve_storage_max_bytes(cfg);
    let max_full_content = webfind::config::resolve_max_full_content_pages(cfg);
    let _storage_task = if let Some(store) = storage_store.clone() {
        if max_bytes.is_some() || max_full_content.is_some() {
            tracing::info!(
                "storage budget active: max_bytes={:?} max_full_content_pages={:?}",
                max_bytes,
                max_full_content
            );
            Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
                loop {
                    interval.tick().await;
                    match store
                        .enforce_storage_budget(max_bytes, max_full_content)
                        .await
                    {
                        Ok(pruned) if pruned > 0 => {
                            tracing::info!("storage budget: evicted full content for {pruned} pages");
                        }
                        Ok(_) => {}
                        Err(e) => tracing::warn!("storage budget enforcement failed: {e}"),
                    }
                }
            }))
        } else {
            None
        }
    } else {
        None
    };

    match transport {
        webfind::cli::TransportArg::Http => {
            println!("Starting HTTP search API on port {}", port);
            if rate_limit.is_some() {
                println!("Rate limiting enabled");
            }
            println!("Starting WebFind GUI on port {}", gui_port);
            if query_log.is_some() {
                println!("Query log & autocomplete enabled");
            }

            let state = Arc::new(
                webfind::api::ApiState::new(
                    indexer,
                    Some(graph_store),
                    audit_store,
                    webfind::config::data_dir(),
                )
                .with_query_log(query_log)
                .with_categories(categories),
            );

            let cfg_clone = cfg.clone();
            let api_handle = tokio::spawn(webfind::api::run_server(
                state.clone(),
                rate_limit,
                port,
                cfg_clone,
            ));
            let gui_handle = tokio::spawn(webfind::gui::run_server(state.clone(), gui_port));

            let (api_res, gui_res) = tokio::try_join!(api_handle, gui_handle)?;
            api_res?;
            gui_res?;
            Ok(())
        }
        webfind::cli::TransportArg::Stdio => {
            eprintln!("Starting MCP server on stdio");
            webfind::mcp::WebfindMcpServer::run_stdio(
                indexer,
                Some(graph_store),
                audit_store,
                webfind::config::data_dir(),
            )
            .await?;
            Ok(())
        }
    }
}
