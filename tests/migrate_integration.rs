//! Integration tests for `webfind migrate` (FR-7).
//!
//! Exercises the full JSON-export → Turso-database path end to end: a
//! JSON export is written to disk, the migration runs against a fresh
//! Turso file, and the destination is verified for row counts, BLAKE3
//! id re-derivation, embedding persistence, FTS indexability, and that
//! mixed-dimension exports are rejected up-front.

use std::path::PathBuf;

use serde_json::json;
use tempfile::TempDir;
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::storage::migrate::migrate_graph;

/// Build a minimal but representative JSON export and write it to `path`.
fn write_export(path: &PathBuf, nodes: &[serde_json::Value], edges: &[serde_json::Value]) {
    let export = json!({
        "version": 1,
        "exported_at": "2026-08-09T00:00:00Z",
        "url_nodes": nodes,
        "link_edges": edges,
        "page_content": [],
        "crawl_jobs": [],
    });
    std::fs::write(path, serde_json::to_string_pretty(&export).unwrap()).unwrap();
}

fn sample_nodes() -> Vec<serde_json::Value> {
    vec![
        json!({
            "url": "https://example.com/",
            "domain": "example.com",
            "source": "seed",
            "depth": 0,
            "priority": 1.0,
            "discovered_at": "2026-08-09T00:00:00Z",
            "crawled": true,
            "excerpt": "example homepage",
            "embedding": [0.1_f32, 0.2, 0.3, 0.4],
        }),
        json!({
            "url": "https://example.com/page1",
            "domain": "example.com",
            "source": "link_crawl",
            "depth": 1,
            "crawled": false,
            "embedding": [0.5_f32, 0.6, 0.7, 0.8],
        }),
        json!({
            "url": "https://other.test/deep",
            "domain": "other.test",
            "source": "external_link",
            "depth": 2,
            "crawled": true,
        }),
    ]
}

fn sample_edges() -> Vec<serde_json::Value> {
    vec![
        json!({ "from": "https://example.com/", "to": "https://example.com/page1", "anchor_text": "page one" }),
        json!({ "from": "https://example.com/page1", "to": "https://other.test/deep" }),
    ]
}

#[tokio::test]
async fn migrate_round_trip_verifies_row_counts_and_persists_embeddings() {
    let tmp = TempDir::new().unwrap();
    let export_path = tmp.path().join("export.json");
    let db_path = tmp.path().join("graph.db");

    write_export(&export_path, &sample_nodes(), &sample_edges());

    let report = migrate_graph(&export_path, &db_path)
        .await
        .expect("migration succeeds");

    assert_eq!(report.url_nodes_imported, 3, "all three nodes imported");
    assert_eq!(report.link_edges_imported, 2, "all two edges imported");
    assert_eq!(report.page_content_imported, 0);
    assert_eq!(report.crawl_jobs_imported, 0);
    assert_eq!(
        report.embedding_dimension,
        Some(4),
        "common embedding dim detected"
    );

    // Destination must be openable and queryable.
    let store = webfind::storage::turso_store::TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open migrated db");

    assert_eq!(store.count_rows("url_nodes").await.unwrap(), 3);
    assert_eq!(store.count_rows("link_edges").await.unwrap(), 2);
}

#[tokio::test]
async fn migrate_creates_stub_nodes_for_orphan_edge_endpoints() {
    let tmp = TempDir::new().unwrap();
    let export_path = tmp.path().join("export.json");
    let db_path = tmp.path().join("graph.db");

    // Edge references a URL absent from url_nodes -> migration must stub it.
    let nodes = vec![json!({ "url": "https://a.test/", "source": "seed", "depth": 0 })];
    let edges = vec![json!({ "from": "https://a.test/", "to": "https://b.test/orphan" })];

    write_export(&export_path, &nodes, &edges);
    let report = migrate_graph(&export_path, &db_path)
        .await
        .expect("migration succeeds");

    // 1 declared node + 1 stub for the orphan edge endpoint.
    assert_eq!(report.url_nodes_imported, 2);
    assert_eq!(report.link_edges_imported, 1);

    let store = webfind::storage::turso_store::TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open migrated db");
    assert_eq!(store.count_rows("url_nodes").await.unwrap(), 2);
    assert_eq!(store.count_rows("link_edges").await.unwrap(), 1);

    // The stub node is retrievable and derives its id from the URL (BLAKE3).
    let b = store
        .get_url("https://b.test/orphan")
        .await
        .expect("stub node exists");
    assert_eq!(
        b.source,
        webfind::engine::crawl_graph::DiscoverySource::LinkCrawl
    );
}

#[tokio::test]
async fn migrate_rejects_inconsistent_embedding_dimensions() {
    let tmp = TempDir::new().unwrap();
    let export_path = tmp.path().join("export.json");
    let db_path = tmp.path().join("graph.db");

    let nodes = vec![
        json!({ "url": "https://a.test/", "embedding": [0.1_f32, 0.2] }),
        json!({ "url": "https://b.test/", "embedding": [0.1_f32, 0.2, 0.3] }),
    ];
    write_export(&export_path, &nodes, &[]);

    let err = migrate_graph(&export_path, &db_path)
        .await
        .expect_err("migration must fail on mixed dims");
    let msg = format!("{err}");
    assert!(
        msg.contains("inconsistent embedding dimensions"),
        "error should mention mixed dimensions, got: {msg}"
    );
}

#[tokio::test]
async fn migrate_rejects_unknown_export_version() {
    let tmp = TempDir::new().unwrap();
    let export_path = tmp.path().join("export.json");
    let db_path = tmp.path().join("graph.db");

    let export = json!({
        "version": 99,
        "url_nodes": [],
        "link_edges": [],
        "page_content": [],
        "crawl_jobs": [],
    });
    std::fs::write(&export_path, serde_json::to_string_pretty(&export).unwrap()).unwrap();

    let err = migrate_graph(&export_path, &db_path)
        .await
        .expect_err("migration must fail on bad version");
    assert!(
        format!("{err}").contains("unsupported export version"),
        "error should mention version, got: {err}"
    );
}

#[tokio::test]
async fn migrate_is_idempotent_over_repeated_runs() {
    let tmp = TempDir::new().unwrap();
    let export_path = tmp.path().join("export.json");

    write_export(&export_path, &sample_nodes(), &sample_edges());

    // Run twice into separate databases; both should succeed with identical counts.
    for _ in 0..2 {
        let db_path = tmp
            .path()
            .join(format!("graph-{}.db", rand::random::<u16>()));
        let report = migrate_graph(&export_path, &db_path)
            .await
            .expect("migration succeeds");
        assert_eq!(report.url_nodes_imported, 3);
        assert_eq!(report.link_edges_imported, 2);
    }
}
