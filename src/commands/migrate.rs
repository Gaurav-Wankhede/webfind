//! `webfind migrate` — migrate a legacy SurrealDB JSON export into a fresh
//! Turso database (FR-7).
//!
//! Reads a JSON export of `url_nodes` + `link_edges` (+ optional `page_content`
//! and `crawl_jobs`) and writes a Turso/libSQL database. Re-derives every URL ID
//! via the shared BLAKE3 `url_id()` utility, validates embedding dimensions, and
//! rebuilds the FTS index + PageRank so the destination is queryable immediately.

use std::path::PathBuf;

use anyhow::Context;
use webfind::storage::migrate::migrate_graph;

/// Run the `migrate` subcommand.
///
/// `from` is the path to the JSON export. `to` is the destination Turso file;
/// it is created if absent and overwritten if present.
pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    from: PathBuf,
    to: PathBuf,
) -> anyhow::Result<()> {
    // Allow --to to be relative to the configured data dir when it is not an
    // absolute path — matches the ergonomics of other Turso-backed commands.
    let to = resolve_dest(cfg, to);
    let to_str = to
        .to_str()
        .with_context(|| format!("destination path is not valid UTF-8: {}", to.display()))?;

    if !from.exists() {
        anyhow::bail!(
            "export file not found: {} — run `surrealdb export` or supply a valid path",
            from.display()
        );
    }

    println!(
        "Migrating\n  from: {}\n  to:   {}",
        from.display(),
        to.display()
    );

    let report = migrate_graph(&from, to_str)
        .await
        .context("migration failed")?;

    println!("\nMigration complete:");
    println!("  url_nodes:     {}", report.url_nodes_imported);
    println!("  link_edges:    {}", report.link_edges_imported);
    println!("  page_content:  {}", report.page_content_imported);
    println!("  crawl_jobs:    {}", report.crawl_jobs_imported);
    if let Some(dim) = report.embedding_dimension {
        println!("  embedding_dim: {} (validated)", dim);
    } else {
        println!("  embedding_dim: none");
    }
    Ok(())
}

/// Resolve the destination path: absolute paths are used as-is; otherwise the
/// path is joined to the configured data directory.
fn resolve_dest(_cfg: &webfind::config::WebfindConfig, to: PathBuf) -> PathBuf {
    if to.is_absolute() {
        return to;
    }
    webfind::config::data_dir().join(to)
}
