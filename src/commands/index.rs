use std::sync::Arc;

use webfind::cli::IndexAction;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};

pub async fn run(action: IndexAction) -> anyhow::Result<()> {
    match action {
        IndexAction::Import { dataset, limit } => {
            println!("Importing from Common Crawl: {} (limit={})", dataset, limit);
            anyhow::bail!("Import not yet implemented — coming in Phase 10");
        }
        IndexAction::Stats => {
            let path = crate::commands::index_path();
            if !path.exists() {
                println!("No index found at {}", path.display());
                println!("Run 'webfind crawl' or 'webfind index import' to create one.");
                return Ok(());
            }
            let indexer = Arc::new(InMemorySearchEngine::new());
            let count = indexer.doc_count().await?;
            println!("Index: {}", path.display());
            println!("Documents: {}", count);
            println!("Engine: v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        IndexAction::Optimize => {
            let path = crate::commands::index_path();
            if !path.exists() {
                println!("No index found at {}", path.display());
                return Ok(());
            }
            let indexer = Arc::new(InMemorySearchEngine::new());
            indexer.optimize().await?;
            println!("Index optimized.");
            Ok(())
        }
    }
}
