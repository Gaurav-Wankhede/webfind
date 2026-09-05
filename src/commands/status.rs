pub async fn run() -> anyhow::Result<()> {
    println!("webfind v{}", env!("CARGO_PKG_VERSION"));
    println!("Indexing: in-memory (no persistent index file)");
    println!("Status:   ready — index is populated at runtime by crawl/research/MCP calls");
    Ok(())
}
