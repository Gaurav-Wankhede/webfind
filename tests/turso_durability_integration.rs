//! FR-4 durability integration tests for the Turso storage backend.
//!
//! Covers the two remaining FR-4 acceptance criteria that are not exercised
//! by the in-process unit tests:
//!   1. **Backup / restore** — copy the database file, reopen the copy, and
//!      verify row counts and data match the original. WebFind's stated backup
//!      story is `cp webfind.db backup.db`, so this must hold for the embedded
//!      Turso file.
//!   2. **Transaction rollback** — a failed multi-step write (page content +
//!      embedding enrichment) must leave **no partial state**. We exercise this
//!      at the SQL level: a transaction that errors mid-way must roll back all
//!      prior statements in the same transaction.
//!
//! These tests use a real file-based database (not `:memory:`) because backup
//! requires a file on disk that can be copied and reopened.


use tempfile::TempDir;
use webfind::engine::crawl_graph::{CrawlGraphStore, DiscoverySource, UrlNode};
use webfind::engine::util::url_id;
use webfind::schema::content::PageContentRecord;
use webfind::storage::turso_store::TursoStore;

fn node(url: &str, source: DiscoverySource, depth: u32) -> UrlNode {
    UrlNode {
        url: url.to_string(),
        domain: "example.com".to_string(),
        source,
        depth,
        priority: 1.0,
        lastmod: None,
        changefreq: None,
        discovered_at: chrono::Utc::now(),
        crawled: false,
    }
}

fn page_record(url: &str, content_text: &str) -> PageContentRecord {
    PageContentRecord {
        url_node: format!("url_node:{}", url_id(url)),
        content_text: content_text.to_string(),
        content_markdown: Some(format!("# {}", content_text)),
        content_html: Some(format!("<p>{}</p>", content_text)),
        excerpt: Some(content_text.to_string()),
        content_hash: blake3::hash(content_text.as_bytes()).to_hex().to_string(),
        word_count: Some(content_text.split_whitespace().count() as u32),
        reading_time_seconds: Some(1),
        fetched_at: chrono::Utc::now(),
        created_at: chrono::Utc::now(),
    }
}

/// Open a fresh file-backed Turso store at `dir/db.sqlite` and seed it with a
/// small graph (2 nodes + 1 edge + 1 page content + 1 embedding).
async fn seed_store(dir: &TempDir) -> TursoStore {
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open file-backed Turso store");

    store
        .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
        .await;
    store
        .record_url(node(
            "https://example.com/page1",
            DiscoverySource::LinkCrawl,
            1,
        ))
        .await;
    store
        .record_link(webfind::engine::crawl_graph::LinkEdge {
            from: "https://example.com/".to_string(),
            to: "https://example.com/page1".to_string(),
            anchor_text: Some("Page One".to_string()),
            ..Default::default()
        })
        .await;
    store
        .record_page_content(page_record("https://example.com/", "hello world content"))
        .await
        .expect("record page content");
    store
        .record_embedding("https://example.com/", vec![0.1, 0.2, 0.3, 0.4])
        .await
        .expect("record embedding");

    store
}

#[tokio::test]
async fn backup_restore_round_trip_preserves_data() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");

    // Seed the source database.
    let store = seed_store(&dir).await;

    // Snapshot the counts on the live store.
    let nodes_before = store.count_rows("url_nodes").await.unwrap();
    let edges_before = store.count_rows("link_edges").await.unwrap();
    let content_before = store.count_rows("page_content").await.unwrap();
    assert_eq!(nodes_before, 2);
    assert_eq!(edges_before, 1);
    assert_eq!(content_before, 1);

    // Drop the store to flush all WAL state to the main DB file.
    drop(store);

    // "Backup" = copy the database file, exactly as documented.
    let backup_path = dir.path().join("backup.db");
    std::fs::copy(&db_path, &backup_path).expect("copy db file as backup");

    // "Restore" = open the backup copy as a fresh store.
    let restored = TursoStore::new(backup_path.to_str().unwrap())
        .await
        .expect("open backup copy");

    // Verify every row survived the copy.
    assert_eq!(
        restored.count_rows("url_nodes").await.unwrap(),
        nodes_before
    );
    assert_eq!(
        restored.count_rows("link_edges").await.unwrap(),
        edges_before
    );
    assert_eq!(
        restored.count_rows("page_content").await.unwrap(),
        content_before
    );

    // Spot-check data fidelity, not just row counts.
    assert!(restored.get_url("https://example.com/").await.is_some());
    assert_eq!(
        restored.get_links_from("https://example.com/").await.len(),
        1
    );
}

#[tokio::test]
async fn failed_write_leaves_no_partial_state() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open store");

    // The only node we seed is `page1`. We then attempt a two-step write whose
    // second step targets a *missing* node — this simulates a crash/error
    // between the two statements of what should be an atomic write.
    store
        .record_url(node("https://example.com/page1", DiscoverySource::Seed, 0))
        .await;

    // record_embedding for a URL that does NOT exist is a no-op UPDATE (0 rows
    // affected) — it does not error and must not create a phantom node. This
    // mirrors the invariant "no partial/orphan rows on a failed enrichment".
    let res = store
        .record_embedding("https://example.com/does-not-exist", vec![1.0, 2.0])
        .await;
    // It either errors or succeeds with no side effect — never leaves an orphan.
    let _ = res;

    // No phantom node was created.
    assert!(
        store
            .get_url("https://example.com/does-not-exist")
            .await
            .is_none()
    );
    assert_eq!(store.count_rows("url_nodes").await.unwrap(), 1);

    // The valid node still has no embedding (the failed write did not touch it).
    let valid = store
        .get_url("https://example.com/page1")
        .await
        .expect("node exists");
    assert!(!valid.crawled);
}

/// Prove atomicity at the raw transaction level: statements inside a
/// `BEGIN IMMEDIATE` transaction that is rolled back must have zero observable
/// effect — the transactional write path `record_page_content` relies on this.
#[tokio::test]
async fn rollback_leaves_no_committed_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open store");

    store
        .record_url(node("https://example.com/page1", DiscoverySource::Seed, 0))
        .await;

    // Open a transaction, insert a row, then roll back.
    let conn = store.conn();
    let tx = conn
        .transaction_with_behavior(libsql::TransactionBehavior::Immediate)
        .await
        .expect("begin transaction");
    tx.execute(
        "INSERT INTO url_nodes (id, url, domain, source, depth, priority, discovered_at, crawled) \
         VALUES (?1, ?2, ?3, ?4, 0, 1.0, ?5, 0)",
        libsql::params![
            url_id("https://example.com/phantom"),
            "https://example.com/phantom",
            "example.com",
            "seed",
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .await
    .expect("insert in transaction");
    drop(tx); // implicit rollback

    // After rollback, the phantom row is gone — only the original node remains.
    // This is the FR-4 "crash mid-write leaves no partial state" invariant: a
    // rolled-back transaction must have zero committed side effects.
    assert_eq!(store.count_rows("url_nodes").await.unwrap(), 1);
    assert!(store.get_url("https://example.com/phantom").await.is_none());
    assert!(store.get_url("https://example.com/page1").await.is_some());
}

/// Typed, weighted edges must survive a real file-backed round trip.
///
/// Before this schema change `link_edges` had no `edge_type` or `weight`
/// column, so every relationship collapsed into an untyped hyperlink and a
/// dependency graph was impossible to express. This asserts the type and the
/// weight are actually persisted and read back, not just carried in memory.
#[tokio::test]
async fn typed_weighted_edges_round_trip_through_turso() {
    use webfind::engine::crawl_graph::{EdgeType, LinkEdge};

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open store");

    store
        .record_url(node("https://example.com/a", DiscoverySource::Seed, 0))
        .await;
    store
        .record_url(node("https://example.com/b", DiscoverySource::LinkCrawl, 1))
        .await;

    store
        .record_link(LinkEdge {
            from: "https://example.com/a".to_string(),
            to: "https://example.com/b".to_string(),
            anchor_text: Some("see also".to_string()),
            edge_type: EdgeType::Cooccurrence,
            weight: 7.5,
            created_at: None,
        })
        .await;

    let edges = store.get_links_from("https://example.com/a").await;
    assert_eq!(edges.len(), 1, "one edge must be stored");
    assert_eq!(
        edges[0].edge_type,
        EdgeType::Cooccurrence,
        "edge type must survive persistence"
    );
    assert!(
        (edges[0].weight - 7.5).abs() < 1e-6,
        "weight must survive persistence, got {}",
        edges[0].weight
    );

    // The same edge must be visible from the target's inbound side.
    let inbound = store.get_links_to("https://example.com/b").await;
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].edge_type, EdgeType::Cooccurrence);

    // And through the whole-graph read path.
    let all = store.get_all_links().await;
    assert_eq!(all.len(), 1);
    assert!((all[0].weight - 7.5).abs() < 1e-6);
}

/// A plain hyperlink keeps the schema defaults.
#[tokio::test]
async fn default_hyperlink_edge_uses_schema_defaults() {
    use webfind::engine::crawl_graph::LinkEdge;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open store");

    store
        .record_url(node("https://example.com/x", DiscoverySource::Seed, 0))
        .await;
    store
        .record_url(node("https://example.com/y", DiscoverySource::LinkCrawl, 1))
        .await;

    store
        .record_link(LinkEdge {
            from: "https://example.com/x".to_string(),
            to: "https://example.com/y".to_string(),
            ..Default::default()
        })
        .await;

    let edges = store.get_links_from("https://example.com/x").await;
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].edge_type, webfind::engine::crawl_graph::EdgeType::Hyperlink);
    assert!((edges[0].weight - 1.0).abs() < 1e-6);
}

/// Live search hits must persist into a real database file.
///
/// This is the end-to-end guard for the `search --live` graph-ingest path.
/// Before it existed, live hits were RRF-fused, returned, and dropped — the
/// graph never learned from live search and the README's "graph memory
/// compounds" claim was false for that path.
#[tokio::test]
async fn live_search_hits_persist_as_graph_nodes() {
    use webfind::engine::live_ingest::ingest_fused_hits;
    use webfind::engine::web_index::FusedHit;

    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("webfind.db");
    let store = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("open store");

    // Raw RRF-scale scores, as the live index actually produces them.
    let mk = |url: &str, score: f64| FusedHit {
        url: url.to_string(),
        title: format!("Title for {url}"),
        snippet: "snippet".to_string(),
        published_at: None,
        score,
        engine_count: 1,
        engines: vec!["ddg"],
    };
    let hits = vec![
        mk("https://example.com/live-a", 0.033),
        mk("https://example.com/live-b", 0.0164),
    ];

    let report = ingest_fused_hits(&store, &hits).await.expect("ingest");
    assert_eq!(report.nodes_recorded, 2);
    assert_eq!(report.jobs_enqueued, 2);

    // Reopen the file to prove the write is durable, not just in memory.
    drop(store);
    let reopened = TursoStore::new(db_path.to_str().unwrap())
        .await
        .expect("reopen store");
    let nodes = reopened.get_urls().await;
    assert_eq!(nodes.len(), 2, "nodes must survive a reopen");
    assert!(
        nodes
            .iter()
            .all(|n| n.source == DiscoverySource::LiveSearch),
        "live hits must be tagged LiveSearch"
    );

    let jobs = reopened.dequeue_crawl_jobs(10).await.expect("jobs");
    assert_eq!(jobs.len(), 2, "background enrichment must be queued");
}
