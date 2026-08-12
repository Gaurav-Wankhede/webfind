//! Graph migration from a legacy SurrealDB JSON export to a fresh Turso database (FR-7).
//!
//! The migration tool reads a JSON export of `url_nodes` + `link_edges` (+ optional
//! `page_content` and `crawl_jobs`) and writes a fresh Turso/libSQL database. On the
//! way in it:
//!
//! - Re-derives every URL ID from the URL itself via the shared BLAKE3 [`url_id`]
//!   utility, discarding whatever IDs the source stored (SHA-256 or otherwise).
//! - Validates that all embeddings share one dimension before importing; an export
//!   with mixed dimensions is rejected up-front rather than producing a corrupt DB.
//! - Rebuilds the FTS index and recomputes PageRank so the destination is fully
//!   queryable immediately after migration.
//!
//! The source schema is intentionally lenient — fields that are commonly null
//! (lastmod, changefreq, anchor_text) default cleanly so partial exports still
//! migrate.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashSet;
use std::path::Path;

use crate::engine::crawl_graph::{CrawlGraphStore, DiscoverySource, UrlNode};
use crate::engine::util::url_id;
use crate::schema::content::PageContentRecord;
use crate::storage::turso_store::TursoStore;

/// Expected version of the JSON export schema. Rejects unknown versions so a
/// future schema bump is never silently mis-parsed.
const EXPORT_VERSION: u32 = 1;

/// Summary returned to the caller after a successful migration.
#[derive(Debug, Clone)]
pub struct MigrationReport {
    /// Number of url_nodes imported.
    pub url_nodes_imported: usize,
    /// Number of link_edges imported.
    pub link_edges_imported: usize,
    /// Number of page_content records imported.
    pub page_content_imported: usize,
    /// Number of crawl_jobs imported.
    pub crawl_jobs_imported: usize,
    /// Common embedding dimension across all embeddings, if any were present.
    pub embedding_dimension: Option<usize>,
}

/// Top-level JSON export envelope.
#[derive(Debug, Deserialize)]
pub struct MigrationExport {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub exported_at: Option<String>,
    #[serde(default)]
    pub url_nodes: Vec<ExportUrlNode>,
    #[serde(default)]
    pub link_edges: Vec<ExportLinkEdge>,
    #[serde(default)]
    pub page_content: Vec<ExportPageContent>,
    #[serde(default)]
    pub crawl_jobs: Vec<ExportCrawlJob>,
}

/// A URL node in the export. The `id` field, if present, is ignored — the
/// destination always re-derives the canonical BLAKE3 id from `url`.
#[derive(Debug, Deserialize)]
pub struct ExportUrlNode {
    pub url: String,
    #[serde(default)]
    pub domain: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub depth: Option<u32>,
    #[serde(default)]
    pub priority: Option<f32>,
    #[serde(default)]
    pub lastmod: Option<String>,
    #[serde(default)]
    pub changefreq: Option<String>,
    #[serde(default)]
    pub discovered_at: Option<String>,
    #[serde(default)]
    pub crawled: Option<bool>,
    #[serde(default)]
    pub excerpt: Option<String>,
    /// Embedding vector as a flat list of f32, or null if absent.
    #[serde(default)]
    pub embedding: Option<Vec<f32>>,
}

/// A directed link edge in the export. Endpoint IDs, if present, are ignored —
/// source/target ids are re-derived from the `from`/`to` URLs.
#[derive(Debug, Deserialize)]
pub struct ExportLinkEdge {
    pub from: String,
    pub to: String,
    #[serde(default)]
    pub anchor_text: Option<String>,
}

/// A page-content record in the export.
#[derive(Debug, Deserialize)]
pub struct ExportPageContent {
    /// URL of the owning node. Used to derive `url_node_id`.
    pub url: String,
    #[serde(default)]
    pub content_text: Option<String>,
    #[serde(default)]
    pub content_markdown: Option<String>,
    #[serde(default)]
    pub content_html: Option<String>,
    #[serde(default)]
    pub excerpt: Option<String>,
    #[serde(default)]
    pub content_hash: Option<String>,
    #[serde(default)]
    pub word_count: Option<u32>,
    #[serde(default)]
    pub reading_time_seconds: Option<u32>,
    #[serde(default)]
    pub fetched_at: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

/// A crawl job in the export.
#[derive(Debug, Deserialize)]
pub struct ExportCrawlJob {
    pub url: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub attempts: Option<i32>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
}

/// Run the migration from a JSON export file to a Turso database file.
///
/// `from_path` points to the JSON export; `to_path` is the destination libSQL
/// file. If `to_path` already exists it is **overwritten** after a successful
/// import. Returns a [`MigrationReport`] on success.
pub async fn migrate_graph(
    from_path: impl AsRef<Path>,
    to_path: impl AsRef<Path>,
) -> Result<MigrationReport> {
    let from_path = from_path.as_ref();
    let to_path = to_path.as_ref();

    // 1. Load + parse the export.
    let export = load_export(from_path).await?;

    // 2. Validate the schema version.
    if export.version != EXPORT_VERSION {
        bail!(
            "unsupported export version {found}; expected {expected}",
            found = export.version,
            expected = EXPORT_VERSION
        );
    }

    // 3. Validate embedding dimensions are consistent.
    let dim = validate_embedding_dimensions(&export.url_nodes)?;

    // 4. Open (create) the destination Turso database.
    let store = TursoStore::new(to_path.to_str().unwrap_or("webfind.db"))
        .await
        .with_context(|| {
            format!(
                "create destination Turso database at `{}`",
                to_path.display()
            )
        })?;

    // 5. Import url_nodes (re-deriving BLAKE3 ids from URLs) + link_edges in a
    //    single transaction. Batching cuts commit overhead from O(rows) to
    //    O(1), which is what makes the FR-7 "<60s / 100K docs" budget reachable.
    let mut url_set: HashSet<String> = HashSet::with_capacity(export.url_nodes.len());

    // Build the node batch (with optional excerpt + embedding).
    let mut node_batch: Vec<(UrlNode, Option<String>, Option<Vec<f32>>)> =
        Vec::with_capacity(export.url_nodes.len());
    for node in &export.url_nodes {
        let domain = node
            .domain
            .clone()
            .or_else(|| crate::engine::util::extract_domain(&node.url))
            .unwrap_or_default();
        node_batch.push((
            UrlNode {
                url: node.url.clone(),
                domain,
                source: parse_source(node.source.as_deref()),
                depth: node.depth.unwrap_or(0),
                priority: node.priority.unwrap_or(1.0),
                lastmod: node.lastmod.as_deref().map(|s| parse_ts(Some(s))),
                changefreq: node.changefreq.clone(),
                discovered_at: parse_ts(node.discovered_at.as_deref()),
                crawled: node.crawled.unwrap_or(false),
            },
            node.excerpt.clone(),
            node.embedding.clone(),
        ));
        url_set.insert(node.url.clone());
    }

    // Stub nodes for edge endpoints absent from url_nodes, so edges never dangle.
    let mut edge_endpoints: HashSet<String> = HashSet::new();
    for edge in &export.link_edges {
        edge_endpoints.insert(edge.from.clone());
        edge_endpoints.insert(edge.to.clone());
    }
    for url in edge_endpoints.difference(&url_set) {
        let domain = crate::engine::util::extract_domain(url).unwrap_or_default();
        node_batch.push((
            UrlNode {
                url: url.clone(),
                domain,
                source: DiscoverySource::LinkCrawl,
                depth: 0,
                priority: 1.0,
                lastmod: None,
                changefreq: None,
                discovered_at: Utc::now(),
                crawled: false,
            },
            None,
            None,
        ));
    }

    // Edge batch.
    let mut edge_batch: Vec<(String, String, Option<String>)> =
        Vec::with_capacity(export.link_edges.len());
    for edge in &export.link_edges {
        edge_batch.push((edge.from.clone(), edge.to.clone(), edge.anchor_text.clone()));
    }

    let (imported_nodes, imported_edges) = store
        .migrate_batch(&node_batch, &edge_batch)
        .await
        .context("batch-import url_nodes + link_edges")?;
    assert_eq!(imported_edges, edge_batch.len());

    // 7. Import page_content (re-deriving url_node_id from URL).
    let mut imported_content = 0usize;
    for content in &export.page_content {
        import_page_content(&store, content)
            .await
            .with_context(|| format!("import page content for `{}`", content.url))?;
        imported_content += 1;
    }

    // 8. Import crawl_jobs.
    let mut imported_jobs = 0usize;
    for job in &export.crawl_jobs {
        import_crawl_job(&store, job)
            .await
            .with_context(|| format!("import crawl job `{}`", job.url))?;
        imported_jobs += 1;
    }

    // 9. Rebuild FTS index and recompute PageRank.
    store
        .rebuild_fts()
        .await
        .context("rebuild FTS index after migration")?;
    store
        .compute_pagerank(20, 0.85)
        .await
        .context("compute PageRank after migration")?;

    // 10. Verify row counts match the export.
    verify_row_counts(
        &store,
        imported_nodes,
        imported_edges,
        imported_content,
        imported_jobs,
    )
    .await?;

    let report = MigrationReport {
        url_nodes_imported: imported_nodes,
        link_edges_imported: imported_edges,
        page_content_imported: imported_content,
        crawl_jobs_imported: imported_jobs,
        embedding_dimension: dim,
    };

    tracing::info!(
        url_nodes = report.url_nodes_imported,
        link_edges = report.link_edges_imported,
        page_content = report.page_content_imported,
        crawl_jobs = report.crawl_jobs_imported,
        embedding_dim = ?report.embedding_dimension,
        "migration complete"
    );
    Ok(report)
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Read and parse a JSON export from disk.
async fn load_export(path: &Path) -> Result<MigrationExport> {
    let text = tokio::fs::read_to_string(path)
        .await
        .with_context(|| format!("read export file `{}`", path.display()))?;
    serde_json::from_str::<MigrationExport>(&text)
        .with_context(|| format!("parse export JSON from `{}`", path.display()))
}

/// Validate that all non-empty embeddings share a single dimension.
///
/// Returns the common dimension, or `None` if no embeddings are present.
fn validate_embedding_dimensions(nodes: &[ExportUrlNode]) -> Result<Option<usize>> {
    let mut expected: Option<usize> = None;
    for node in nodes {
        let Some(vec) = &node.embedding else { continue };
        if vec.is_empty() {
            continue;
        }
        match expected {
            None => expected = Some(vec.len()),
            Some(dim) if dim != vec.len() => bail!(
                "inconsistent embedding dimensions in export: \
                 expected {dim} but `{}` has {}",
                node.url,
                vec.len()
            ),
            _ => {}
        }
    }
    Ok(expected)
}

/// Parse an ISO-8601 timestamp string, defaulting to now on missing/invalid.
fn parse_ts(value: Option<&str>) -> DateTime<Utc> {
    match value {
        Some(s) => DateTime::parse_from_rfc3339(s)
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|_| Utc::now()),
        None => Utc::now(),
    }
}

/// Map a source string to a `DiscoverySource`, defaulting to Seed.
fn parse_source(value: Option<&str>) -> DiscoverySource {
    match value.unwrap_or("seed") {
        "sitemap" => DiscoverySource::Sitemap,
        "link_crawl" => DiscoverySource::LinkCrawl,
        "external_link" => DiscoverySource::ExternalLink,
        _ => DiscoverySource::Seed,
    }
}

/// Import a page_content record, deriving url_node_id from the URL.
async fn import_page_content(store: &TursoStore, content: &ExportPageContent) -> Result<()> {
    let record = PageContentRecord {
        url_node: format!("url_node:{}", url_id(&content.url)),
        content_text: content.content_text.clone().unwrap_or_default(),
        content_markdown: content.content_markdown.clone(),
        content_html: content.content_html.clone(),
        excerpt: content.excerpt.clone(),
        content_hash: content
            .content_hash
            .clone()
            .unwrap_or_else(|| format!("hash-{}", url_id(&content.url))),
        word_count: content.word_count,
        reading_time_seconds: content.reading_time_seconds,
        fetched_at: parse_ts(content.fetched_at.as_deref()),
        created_at: parse_ts(content.created_at.as_deref()),
    };
    store.record_page_content(record).await?;
    Ok(())
}

/// Import a crawl job.
async fn import_crawl_job(store: &TursoStore, job: &ExportCrawlJob) -> Result<()> {
    let id = store.enqueue_crawl_job(&job.url).await?;
    // Preserve the source status rather than leaving the job in 'pending'.
    if let Some(status) = job.status.as_deref() {
        store
            .mark_crawl_job_status(&id, status, job.error.as_deref())
            .await?;
    }
    Ok(())
}

/// Verify the destination row counts match what we imported.
async fn verify_row_counts(
    store: &TursoStore,
    expected_nodes: usize,
    expected_edges: usize,
    expected_content: usize,
    expected_jobs: usize,
) -> Result<()> {
    let actual_nodes = store.count_rows("url_nodes").await?;
    let actual_edges = store.count_rows("link_edges").await?;
    let actual_content = store.count_rows("page_content").await?;
    let actual_jobs = store.count_rows("crawl_jobs").await?;

    if actual_nodes != expected_nodes {
        bail!("url_nodes row-count mismatch: expected {expected_nodes}, found {actual_nodes}");
    }
    if actual_edges != expected_edges {
        bail!("link_edges row-count mismatch: expected {expected_edges}, found {actual_edges}");
    }
    if actual_content != expected_content {
        bail!(
            "page_content row-count mismatch: expected {expected_content}, found {actual_content}"
        );
    }
    if actual_jobs != expected_jobs {
        bail!("crawl_jobs row-count mismatch: expected {expected_jobs}, found {actual_jobs}");
    }
    tracing::info!(
        actual_nodes,
        actual_edges,
        actual_content,
        actual_jobs,
        "row-count verification passed"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_embedding_dimensions_allows_uniform() {
        let nodes = vec![
            ExportUrlNode {
                url: "https://a".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: Some(vec![1.0, 2.0, 3.0]),
            },
            ExportUrlNode {
                url: "https://b".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: Some(vec![4.0, 5.0, 6.0]),
            },
        ];
        assert_eq!(validate_embedding_dimensions(&nodes).unwrap(), Some(3));
    }

    #[test]
    fn validate_embedding_dimensions_rejects_mixed() {
        let nodes = vec![
            ExportUrlNode {
                url: "https://a".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: Some(vec![1.0, 2.0]),
            },
            ExportUrlNode {
                url: "https://b".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: Some(vec![1.0, 2.0, 3.0]),
            },
        ];
        assert!(validate_embedding_dimensions(&nodes).is_err());
    }

    #[test]
    fn validate_embedding_dimensions_handles_empty_and_null() {
        let nodes = vec![
            ExportUrlNode {
                url: "https://a".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: None,
            },
            ExportUrlNode {
                url: "https://b".into(),
                domain: None,
                source: None,
                depth: None,
                priority: None,
                lastmod: None,
                changefreq: None,
                discovered_at: None,
                crawled: None,
                excerpt: None,
                embedding: Some(vec![]),
            },
        ];
        assert_eq!(validate_embedding_dimensions(&nodes).unwrap(), None);
    }
}
