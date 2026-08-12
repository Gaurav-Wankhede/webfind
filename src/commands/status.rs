use std::sync::Arc;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};

pub async fn run() -> anyhow::Result<()> {
    use crate::commands::index_path;
    let path = index_path();
    println!("webfind v{}", env!("CARGO_PKG_VERSION"));

    if path.exists() {
        let indexer = InMemorySearchEngine::new();
        match indexer.doc_count().await {
            Ok(count) => {
                println!("Index:    {}", path.display());
                println!("Documents: {}", count);
                println!("Status:   ready");
            }
            Err(e) => {
                println!("Index:    {} (error: {})", path.display(), e);
                println!("Status:   index corrupt or missing");
            }
        }
    } else {
        println!("Index:    not found");
        println!("Status:   no index — run 'webfind crawl' to create one");
    }
    Ok(())
}
