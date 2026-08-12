use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::api::ApiState;
use crate::engine::categories::CategoryService;
use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::query_log::QueryLogService;
use crate::engine::search_engine::SearchEngine;

/// Shared state for the GUI server.
#[derive(Clone)]
pub struct GuiState {
    pub indexer: Arc<RwLock<Arc<dyn SearchEngine + Send + Sync>>>,
    pub graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
    pub data_dir: PathBuf,
    pub query_log: Option<Arc<QueryLogService>>,
    pub categories: Option<Arc<CategoryService>>,
}

impl GuiState {
    pub fn new(
        indexer: Arc<dyn SearchEngine + Send + Sync>,
        graph_store: Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            indexer: Arc::new(RwLock::new(indexer)),
            graph_store,
            data_dir,
            query_log: None,
            categories: None,
        }
    }

    pub fn from_api_state(state: Arc<ApiState>) -> Self {
        Self {
            indexer: state.indexer(),
            graph_store: state.graph_store(),
            data_dir: state.data_dir().clone(),
            query_log: state.query_log(),
            categories: state.categories(),
        }
    }
}
