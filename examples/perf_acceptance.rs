//! PRD acceptance performance benchmarks.
//!
//! Measures the three FR-1..FR-3 acceptance latency/throughput criteria against
//! a realistic synthetic dataset and reports pass/fail:
//!
//!   - FR-2 hybrid search    p99 < 30ms  (100K docs)
//!   - FR-3 graph traversal  p99 < 20ms  (100K edges)
//!   - FR-7 migration        total < 60s (100K docs)
//!
//! Run: `cargo run --release --example perf_acceptance [--nodes N]`
//!
//! Exit code 0 if all criteria pass, 1 if any fail, 2 on setup error.

use std::time::Instant;

use webfind::engine::crawl_graph::{CrawlGraphStore, DiscoverySource, LinkEdge, UrlNode};
use webfind::engine::util::url_id;
use webfind::schema::content::PageContentRecord;
use webfind::storage::turso_store::TursoStore;

const DEFAULT_NODES: usize = 100_000;
const HYBRID_P99_BUDGET_MS: f64 = 30.0;
const TRAVERSAL_P99_BUDGET_MS: f64 = 20.0;
const MIGRATION_BUDGET_S: f64 = 60.0;

/// How many operations to run when sampling the p99 tail.
const SAMPLE_RUNS: usize = 200;

/// Compute the 99th-percentile of a slice of durations (ms).
fn p99_ms(durations: &mut [f64]) -> f64 {
    durations.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let idx = (durations.len() as f64 * 0.99).ceil() as usize;
    let idx = idx.clamp(1, durations.len()) - 1;
    durations[idx]
}

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

/// Generate `n` synthetic URLs and seed a file-backed Turso store with a chain
/// graph (each node links to the next) plus page content and embeddings so BM25
/// and vector search both have signals.
async fn seed_store(path: &str, n: usize) -> TursoStore {
    let store = TursoStore::new(path).await.expect("open store");

    let words = [
        "rust",
        "tokio",
        "async",
        "systems",
        "language",
        "web",
        "server",
        "performance",
        "concurrency",
        "safety",
        "memory",
        "ownership",
        "borrow",
        "crate",
        "module",
    ];

    // Nodes + content + embeddings.
    for i in 0..n {
        let url = format!("https://example.com/page{i}");
        let topic = words[i % words.len()];
        let body = format!(
            "Page {i} about {topic}. The {topic} ecosystem powers high-performance \
             concurrent systems software. This is synthetic content for the acceptance \
             benchmark measuring hybrid search latency under load."
        );
        store.record_url(node(&url, DiscoverySource::Seed, 0)).await;
        store
            .record_page_content(PageContentRecord {
                url_node: format!("url_node:{}", url_id(&url)),
                content_text: body.clone(),
                content_markdown: Some(body.clone()),
                content_html: Some(format!("<p>{body}</p>")),
                excerpt: Some(body[..body.len().min(80)].to_string()),
                content_hash: blake3::hash(body.as_bytes()).to_hex().to_string(),
                word_count: Some(body.split_whitespace().count() as u32),
                reading_time_seconds: Some(1),
                fetched_at: chrono::Utc::now(),
                created_at: chrono::Utc::now(),
            })
            .await
            .expect("record page content");
        // Deterministic pseudo-embedding (8-dim, enough to exercise the path).
        store
            .record_embedding(&url, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8])
            .await
            .expect("record embedding");
    }

    // Chain of link edges: page_i -> page_{i+1} (n-1 edges).
    for i in 0..n.saturating_sub(1) {
        store
            .record_link(LinkEdge {
                from: format!("https://example.com/page{i}"),
                to: format!("https://example.com/page{}", i + 1),
                anchor_text: Some("next".to_string()),
            })
            .await;
    }

    store.rebuild_fts().await.expect("rebuild FTS");
    store
        .compute_pagerank(20, 0.85)
        .await
        .expect("compute pagerank");
    store
        .build_vector_index()
        .await
        .expect("build vector index");
    store
}

fn report(label: &str, budget_ms: f64, p99: f64, units: &str) -> bool {
    let pass = p99 <= budget_ms;
    println!(
        "{label:28} p99 = {p99:8.3} {units}  (budget {budget_ms:6.1} {units})  {}",
        if pass { "PASS" } else { "FAIL" }
    );
    pass
}

#[tokio::main]
async fn main() {
    let n = std::env::args()
        .nth(1)
        .and_then(|a| a.parse::<usize>().ok())
        .unwrap_or(DEFAULT_NODES);

    println!("=== WebFind PRD acceptance performance benchmark ===");
    println!("Dataset: {n} synthetic docs\n");

    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("perf.db");

    let t0 = Instant::now();
    let store = seed_store(db_path.to_str().unwrap(), n).await;
    println!("Seeded {n} docs in {:.1}s", t0.elapsed().as_secs_f64());
    assert_eq!(
        store.count_rows("url_nodes").await.unwrap(),
        n,
        "seed must produce exactly {n} nodes"
    );

    let mut all_pass = true;

    // ── FR-2: Hybrid search p99 < 30ms ─────────────────────────────────────
    let mut hybrid_lat: Vec<f64> = Vec::with_capacity(SAMPLE_RUNS);
    for i in 0..SAMPLE_RUNS {
        let query = format!("{} concurrency", ["rust", "async", "systems"][i % 3]);
        let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
        let start = Instant::now();
        store
            .hybrid_search(&query, Some(&embedding), 10)
            .await
            .expect("hybrid search");
        hybrid_lat.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let hybrid_p99 = p99_ms(&mut hybrid_lat);
    all_pass &= report("hybrid_search", HYBRID_P99_BUDGET_MS, hybrid_p99, "ms");

    // Component breakdown: which signal dominates the hybrid budget?
    let query = "rust concurrency";
    let embedding = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8];
    let t = Instant::now();
    store.bm25_search(query, 10).await.expect("bm25");
    let bm25_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    store.vector_search(&embedding, 10).await.expect("vector");
    let vec_ms = t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let pr = store.pagerank_scores().await.expect("pagerank");
    let pr_ms = t.elapsed().as_secs_f64() * 1000.0;
    println!(
        "  breakdown: bm25={bm25_ms:.2}ms  vector={vec_ms:.2}ms  pagerank_read={pr_ms:.2}ms ({count} rows)",
        count = pr.len()
    );

    // ── FR-3: Graph traversal p99 < 20ms ───────────────────────────────────
    let mut trav_lat: Vec<f64> = Vec::with_capacity(SAMPLE_RUNS);
    for i in 0..SAMPLE_RUNS {
        let start_url = format!("https://example.com/page{}", i * 17 % n);
        let start = Instant::now();
        let _ = store
            .traverse(
                &start_url,
                3,
                webfind::engine::crawl_graph::TraversalDirection::Both,
            )
            .await;
        trav_lat.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let trav_p99 = p99_ms(&mut trav_lat);
    all_pass &= report("graph_traversal", TRAVERSAL_P99_BUDGET_MS, trav_p99, "ms");

    // ── FR-7: Migration wall-clock < 60s ───────────────────────────────────
    // Build a JSON export (url_nodes + link_edges) then time migrate_graph.
    let export_path = dir.path().join("export.json");
    let mut nodes = Vec::with_capacity(n);
    let mut edges = Vec::with_capacity(n.saturating_sub(1));
    for i in 0..n {
        nodes.push(serde_json::json!({
            "url": format!("https://example.com/page{i}"),
            "domain": "example.com",
            "source": "seed",
            "depth": 0,
            "priority": 1.0,
            "discovered_at": "2026-08-09T00:00:00Z",
            "crawled": true,
        }));
        if i + 1 < n {
            edges.push(serde_json::json!({
                "from": format!("https://example.com/page{i}"),
                "to": format!("https://example.com/page{}", i + 1),
                "anchor_text": "next",
            }));
        }
    }
    let export = serde_json::json!({
        "version": 1,
        "exported_at": "2026-08-09T00:00:00Z",
        "url_nodes": nodes,
        "link_edges": edges,
        "page_content": [],
        "crawl_jobs": [],
    });
    std::fs::write(&export_path, serde_json::to_string(&export).unwrap()).unwrap();

    let mig_dest = dir.path().join("migrated.db");
    let mig_start = Instant::now();
    let report = webfind::storage::migrate::migrate_graph(&export_path, &mig_dest)
        .await
        .expect("migrate");
    let mig_secs = mig_start.elapsed().as_secs_f64();
    let mig_pass = mig_secs <= MIGRATION_BUDGET_S;
    all_pass &= mig_pass;
    println!(
        "migration                     total = {mig_secs:8.3} s   (budget {MIGRATION_BUDGET_S:6.1} s)  {}",
        if mig_pass { "PASS" } else { "FAIL" }
    );
    println!(
        "  -> {url} nodes, {edges} edges imported",
        url = report.url_nodes_imported,
        edges = report.link_edges_imported
    );

    println!(
        "\n=== RESULT: {} ===",
        if all_pass { "ALL PASS" } else { "FAILURE(S)" }
    );
    std::process::exit(if all_pass { 0 } else { 1 });
}
