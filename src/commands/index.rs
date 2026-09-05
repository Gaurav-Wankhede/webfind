use webfind::cli::IndexAction;

pub async fn run(action: IndexAction) -> anyhow::Result<()> {
    match action {
        IndexAction::Import { dataset, limit } => {
            println!("Importing from Common Crawl: {} (limit={})", dataset, limit);
            anyhow::bail!("Import not yet implemented — coming in Phase 10");
        }
        IndexAction::Stats => {
            println!("Indexing is in-memory: no persistent index file exists.");
            println!(
                "The search index is populated at runtime by 'webfind crawl', 'webfind research', or MCP research calls."
            );
            println!("Engine: v{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        IndexAction::Optimize => {
            println!("In-memory index needs no optimization.");
            Ok(())
        }
        IndexAction::Domains => {
            println!("{}", webfind::engine::seed_catalog::catalog_summary());
            Ok(())
        }
    }
}
