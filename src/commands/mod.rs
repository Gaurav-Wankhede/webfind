pub fn index_path() -> std::path::PathBuf {
    webfind::config::data_dir().join("index_data")
}

/// Build the configured `CrawlGraphStore` backend.
///
/// All backends return a usable `CrawlGraphStore`. The SurrealDB backend has
/// been fully replaced by Turso; the remaining options are `Turso` (embedded
/// file) and `Memory` (transient, for tests). When `encryption_key` is set, the
/// Turso backend opens an encrypted database (SQLCipher AES-256-CBC) so the
/// graph data is encrypted at rest.
pub async fn build_graph_store(
    kind: &webfind::cli::GraphStoreArg,
    turso_path: &str,
    encryption_key: Option<&str>,
) -> anyhow::Result<std::sync::Arc<dyn webfind::engine::crawl_graph::CrawlGraphStore + Send + Sync>>
{
    use anyhow::Context;
    use std::sync::Arc;
    use webfind::cli::GraphStoreArg;
    use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph};
    use webfind::storage::turso_store::TursoStore;

    match kind {
        GraphStoreArg::Turso => {
            let store = match encryption_key {
                Some(key) => TursoStore::new_with_encryption(turso_path, key)
                    .await
                    .context("open encrypted Turso graph store")?,
                None => TursoStore::new(turso_path)
                    .await
                    .context("open Turso graph store")?,
            };
            Ok(Arc::new(store) as Arc<dyn CrawlGraphStore + Send + Sync>)
        }
        GraphStoreArg::Memory => {
            Ok(Arc::new(InMemoryCrawlGraph::new()) as Arc<dyn CrawlGraphStore + Send + Sync>)
        }
    }
}

pub mod crawl;
pub mod fetch;
pub mod graph;
pub mod index;
pub mod migrate;
pub mod proxy_pool;
pub mod research;
pub mod search;
pub mod serve;
pub mod status;
