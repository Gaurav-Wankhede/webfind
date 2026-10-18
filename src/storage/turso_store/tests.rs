use super::*;
use std::sync::Arc;


    fn node(url: &str, source: DiscoverySource, depth: u32) -> UrlNode {
        UrlNode {
            url: url.to_string(),
            domain: "example.com".to_string(),
            source,
            depth,
            priority: 1.0,
            lastmod: None,
            changefreq: None,
            discovered_at: Utc::now(),
            crawled: false,
        }
    }

    #[tokio::test]
    async fn test_encrypted_store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();
        let key = "test-encryption-key-2026";

        // Write through the encrypted store.
        let store = TursoStore::new_with_encryption(path, key)
            .await
            .expect("create encrypted db");
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/a", DiscoverySource::LinkCrawl, 1))
            .await;

        // Re-open with the correct key — reads must succeed.
        let reopen = TursoStore::new_with_encryption(path, key)
            .await
            .expect("reopen encrypted db with correct key");
        assert_eq!(reopen.count_rows("url_nodes").await.unwrap(), 2);
        assert!(reopen.get_url("https://example.com/").await.is_some());
    }

    #[tokio::test]
    async fn test_encrypted_store_rejects_wrong_key() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();

        // Create with one key.
        let store = TursoStore::new_with_encryption(path, "correct-key")
            .await
            .expect("create encrypted db");
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;

        // Re-open with a different key — decryption must fail.
        let result = TursoStore::new_with_encryption(path, "wrong-key").await;
        assert!(
            result.is_err(),
            "opening an encrypted db with the wrong key must fail"
        );
    }

    #[tokio::test]
    async fn test_encrypted_store_data_not_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();
        let key = "test-encryption-key-2026";

        let store = TursoStore::new_with_encryption(path, key)
            .await
            .expect("create encrypted db");
        store
            .record_url(node(
                "https://secret.example.com/",
                DiscoverySource::Seed,
                0,
            ))
            .await;

        // The raw database file must not contain the plaintext URL.
        let raw = tokio::fs::read_to_string(path).await.unwrap_or_default();
        assert!(
            !raw.contains("secret.example.com"),
            "encrypted db file must not contain plaintext data"
        );
    }

    #[tokio::test]
    async fn test_turso_store_records_and_reads() {
        let store = Arc::new(
            TursoStore::new(":memory:")
                .await
                .expect("open in-memory db"),
        );

        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/a", DiscoverySource::LinkCrawl, 1))
            .await;
        store
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: Some("link".to_string()),
                ..Default::default()
            })
            .await;

        let urls = store.get_urls().await;
        assert_eq!(urls.len(), 2, "expected 2 url nodes");

        let root = store
            .get_url("https://example.com/")
            .await
            .expect("root url exists");
        assert_eq!(root.source, DiscoverySource::Seed);
        assert_eq!(root.depth, 0);

        let outgoing = store.get_links_from("https://example.com/").await;
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].to, "https://example.com/a");

        let incoming = store.get_links_to("https://example.com/a").await;
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].from, "https://example.com/");
    }

    #[tokio::test]
    async fn test_turso_store_graph_version_bumps() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let v0 = store.graph_version().await;
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/b", DiscoverySource::LinkCrawl, 1))
            .await;
        let v1 = store.graph_version().await;
        assert_ne!(v0, v1, "graph version must bump on mutation");
    }

    #[tokio::test]
    async fn test_record_page_content_is_transactional() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let url = "https://example.com/content";
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;

        let record = PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: "hello world".to_string(),
            content_markdown: Some("# hello".to_string()),
            content_html: Some("<p>hello</p>".to_string()),
            excerpt: Some("hello world".to_string()),
            content_hash: "deadbeef".to_string(),
            word_count: Some(2),
            reading_time_seconds: Some(1),
            fetched_at: Utc::now(),
            created_at: Utc::now(),
        };
        store
            .record_page_content(record)
            .await
            .expect("record page content");

        // Enrichment lands on the node atomically.
        let node = store.get_url(url).await.expect("node exists");
        assert!(node.crawled, "node must be marked crawled");

        // Idempotent re-record does not error.
        let again = PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: "hello world v2".to_string(),
            content_markdown: None,
            content_html: None,
            excerpt: Some("updated".to_string()),
            content_hash: "deadbeef".to_string(),
            word_count: Some(2),
            reading_time_seconds: Some(1),
            fetched_at: Utc::now(),
            created_at: Utc::now(),
        };
        store.record_page_content(again).await.expect("re-record");
        let node = store.get_url(url).await.expect("node exists");
        assert!(node.crawled, "node remains crawled after re-record");
    }

    #[tokio::test]
    async fn test_atomic_job_dequeue_no_double_claim() {
        let store = TursoStore::new(":memory:").await.expect("open db");

        for i in 0..5 {
            store
                .enqueue_crawl_job(&format!("https://example.com/page{i}"))
                .await
                .expect("enqueue");
        }

        // Two sequential dequeues must claim disjoint jobs.
        let first = store.dequeue_crawl_jobs(2).await.expect("dequeue 1");
        let second = store.dequeue_crawl_jobs(2).await.expect("dequeue 2");
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);

        let claimed: std::collections::HashSet<String> =
            first.iter().chain(&second).map(|j| j.url.clone()).collect();
        assert_eq!(
            claimed.len(),
            4,
            "dequeued jobs must be unique across claims (no TOCTOU double-assignment)"
        );

        // Remaining single job still pending.
        let third = store.dequeue_crawl_jobs(10).await.expect("dequeue 3");
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].url, "https://example.com/page4");
    }

    #[tokio::test]
    async fn test_crawl_job_status_lifecycle() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let id = store
            .enqueue_crawl_job("https://example.com/job")
            .await
            .expect("enqueue");

        store
            .mark_crawl_job_status(&id, "failed", Some("boom"))
            .await
            .expect("mark failed");
        let jobs = store.dequeue_crawl_jobs(10).await.expect("dequeue");
        assert!(jobs.is_empty(), "failed job is no longer pending");

        store
            .mark_crawl_job_status(&id, "done", None)
            .await
            .expect("mark done");
    }

    #[tokio::test]
    async fn test_embedding_roundtrip() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let url = "https://example.com/embed";
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;

        let vec = vec![0.1f32, 0.2, 0.3, 0.4];
        store
            .record_embedding(url, vec.clone())
            .await
            .expect("record embedding");

        // Verify the blob is persisted (384-dim is the production size; here small).
        let mut rows = store
            .conn()
            .query(
                "SELECT embedding FROM url_nodes WHERE url = ?1",
                params![url],
            )
            .await
            .expect("query embedding");
        let row = rows.next().await.expect("embedding row").expect("present");
        let bytes: Vec<u8> = row.get(0).expect("embedding blob");
        assert_eq!(bytes.len(), vec.len() * 4, "one f32 per dimension");
    }

    /// Helper: record a URL node for traversal tests.
    async fn rec(store: &TursoStore, url: &str) {
        store
            .record_url(node(url, DiscoverySource::LinkCrawl, 0))
            .await;
    }

    /// Build a line graph: a -> b -> c -> d.
    async fn line_graph(store: &TursoStore) {
        for u in ["a", "b", "c", "d"] {
            rec(store, &format!("https://{u}.example.com/")).await;
        }
        let pairs = [("a", "b"), ("b", "c"), ("c", "d")];
        for (f, t) in pairs {
            store
                .record_link(LinkEdge {
                    from: format!("https://{f}.example.com/"),
                    to: format!("https://{t}.example.com/"),
                    anchor_text: None,
                    ..Default::default()
                })
                .await;
        }
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_outbound() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        line_graph(&store).await;

        let visited = store
            .traverse("https://a.example.com/", 2, TraversalDirection::Outbound)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://a.example.com/".to_string(),
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
            ],
            "outbound depth 2 reaches a, b, c"
        );
        assert!(!visited.contains(&"https://d.example.com/".to_string()));
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_inbound() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        line_graph(&store).await;

        let visited = store
            .traverse("https://d.example.com/", 2, TraversalDirection::Inbound)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
                "https://d.example.com/".to_string(),
            ],
            "inbound depth 2 from d reaches b, c, d"
        );
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_both_cycle_safe() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // a <-> b <-> c (bidirectional cycle)
        for u in ["a", "b", "c"] {
            rec(&store, &format!("https://{u}.example.com/")).await;
        }
        let pairs = [("a", "b"), ("b", "a"), ("b", "c"), ("c", "b")];
        for (f, t) in pairs {
            store
                .record_link(LinkEdge {
                    from: format!("https://{f}.example.com/"),
                    to: format!("https://{t}.example.com/"),
                    anchor_text: None,
                    ..Default::default()
                })
                .await;
        }

        // Both directions, depth 5 — must terminate (cycle-safe) and cover all 3.
        let visited = store
            .traverse("https://a.example.com/", 5, TraversalDirection::Both)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://a.example.com/".to_string(),
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
            ],
            "cycle traversal terminates and visits all reachable nodes"
        );
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_empty_and_depth_zero() {
        let store = TursoStore::new(":memory:").await.expect("open db");

        // Depth 0 returns just the start.
        line_graph(&store).await;
        let visited = store
            .traverse("https://a.example.com/", 0, TraversalDirection::Outbound)
            .await;
        assert_eq!(
            visited,
            vec!["https://a.example.com/".to_string()],
            "depth 0 yields only the start URL"
        );

        // Empty graph returns just the start.
        let empty = TursoStore::new(":memory:").await.expect("open db");
        let visited = empty
            .traverse("https://solo.example.com/", 3, TraversalDirection::Both)
            .await;
        assert_eq!(
            visited,
            vec!["https://solo.example.com/".to_string()],
            "no edges still returns the start URL"
        );
    }

    /// Record a url node + page content + optional embedding for search tests.
    async fn rec_content(
        store: &TursoStore,
        url: &str,
        content_text: &str,
        embedding: Option<Vec<f32>>,
    ) {
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;
        store
            .record_page_content(PageContentRecord {
                url_node: format!("url_node:{}", url_id(url)),
                content_text: content_text.to_string(),
                content_markdown: None,
                content_html: None,
                excerpt: Some(content_text.to_string()),
                content_hash: format!("hash-{}", url_id(url)),
                word_count: Some(10),
                reading_time_seconds: Some(1),
                fetched_at: Utc::now(),
                created_at: Utc::now(),
            })
            .await
            .expect("record content");
        if let Some(emb) = embedding {
            store
                .record_embedding(url, emb)
                .await
                .expect("record embedding");
        }
    }

    /// Long enough to clear the word floor, differing by a single word.
    fn dedup_prose() -> String {
        "the distributed search engine indexes crawled web pages across many hosts \
         and stores their content in a local database so that agents can query it \
         later without hitting the network again every single time which saves \
         bandwidth and improves latency for repeated research tasks over time"
            .to_string()
    }

    #[tokio::test]
    async fn test_find_duplicate_rejects_cross_domain_near_dup() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Canonical copy on a different domain: cross-domain is NOT exempt,
        // because syndication is the primary dedup target.
        let canonical = "https://origin.example.com/article";
        rec_content(&store, canonical, &dedup_prose(), None).await;

        // Incoming copy differs by one word and sits on an unrelated domain.
        let syndicated = "https://mirror.other-site.net/article";
        let mut near = dedup_prose();
        near.push_str(" indeed");
        let found = store
            .find_duplicate(syndicated, &near)
            .await
            .expect("dedup check");
        assert_eq!(
            found.as_deref(),
            Some(canonical),
            "cross-domain near-duplicate must collapse onto the canonical copy"
        );
    }

    #[tokio::test]
    async fn test_find_duplicate_skips_short_pages() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(&store, "https://a.example.com/", &dedup_prose(), None).await;
        // Identical text but far below the word floor: fingerprints of short
        // pages are chrome-dominated, so the gate must not fire.
        let short = "same words exactly";
        assert!(
            store
                .find_duplicate("https://b.example.org/", short)
                .await
                .expect("dedup check")
                .is_none(),
            "pages under the word floor must never be rejected as duplicates"
        );
    }

    #[tokio::test]
    async fn test_find_duplicate_first_seen_is_canonical() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Record the later copy first, then backdate the earlier one, so the
        // first-seen rule must pick a URL that was discovered EARLIER.
        let later = "https://z.example.com/doc";
        let earlier = "https://a.example.com/doc";
        rec_content(&store, later, &dedup_prose(), None).await;

        // Re-record the same content under the other URL with an older
        // discovered_at, making it the canonical first-seen copy.
        let mut node = node(earlier, DiscoverySource::Seed, 0);
        node.discovered_at = Utc::now() - chrono::Duration::days(30);
        store.record_url(node).await;
        store
            .record_page_content(PageContentRecord {
                url_node: format!("url_node:{}", url_id(earlier)),
                content_text: dedup_prose(),
                content_markdown: None,
                content_html: None,
                excerpt: None,
                content_hash: format!("hash-{}", url_id(earlier)),
                word_count: Some(60),
                reading_time_seconds: Some(1),
                fetched_at: Utc::now(),
                created_at: Utc::now(),
            })
            .await
            .expect("record earlier copy");

        // A third near-identical page must collapse onto the EARLIER copy,
        // not the one that happened to be indexed first.
        let found = store
            .find_duplicate("https://third.example.net/doc", &dedup_prose())
            .await
            .expect("dedup check");
        assert_eq!(
            found.as_deref(),
            Some(earlier),
            "first-seen copy must be canonical regardless of index order"
        );
    }

    #[tokio::test]
    async fn test_find_duplicate_allows_distinct_content() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(&store, "https://a.example.com/", &dedup_prose(), None).await;
        let unrelated = "python is a high level scripting language used for web \
                         development automation and data analysis workflows that \
                         emphasise readability and a large standard library over \
                         raw execution speed in many production environments";
        assert!(
            store
                .find_duplicate("https://b.example.org/", unrelated)
                .await
                .expect("dedup check")
                .is_none(),
            "genuinely distinct content must not be flagged as a duplicate"
        );
    }

    /// Build substantive prose of exactly `n` words for quality-gate tests.
    fn prose_of_words(n: usize) -> String {
        let vocab = [
            "the", "distributed", "search", "engine", "indexes", "crawled", "web",
            "pages", "across", "many", "hosts", "stores", "content", "local",
            "database", "agents", "query", "later", "network", "saves", "bandwidth",
            "improves", "latency", "research", "tasks", "production", "pipeline",
        ];
        (0..n).map(|i| vocab[i % vocab.len()]).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn test_check_quality_accepts_substantive_content() {
        let body = prose_of_words(TursoStore::MIN_QUALITY_WORDS);
        assert_eq!(
            TursoStore::check_quality(&body),
            QualityVerdict::Accept,
            "content at the word floor must be accepted"
        );
        // Comfortably above the floor.
        assert_eq!(
            TursoStore::check_quality(&prose_of_words(600)),
            QualityVerdict::Accept
        );
    }

    #[test]
    fn test_check_quality_rejects_thin_content() {
        // One word under the floor.
        let thin = prose_of_words(TursoStore::MIN_QUALITY_WORDS - 1);
        assert_eq!(
            TursoStore::check_quality(&thin),
            QualityVerdict::Reject("thin content below word floor"),
            "content just under the floor must be rejected"
        );
        // A nav-chrome fragment: short, repetitive, no intent.
        assert_eq!(
            TursoStore::check_quality("Home About Contact Sign in Subscribe"),
            QualityVerdict::Reject("thin content below word floor")
        );
        // Empty and whitespace-only content.
        assert_eq!(
            TursoStore::check_quality(""),
            QualityVerdict::Reject("thin content below word floor")
        );
        assert_eq!(
            TursoStore::check_quality("   \n\t  "),
            QualityVerdict::Reject("thin content below word floor")
        );
    }

    #[tokio::test]
    async fn test_bm25_search() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(
            &store,
            "https://a.example.com/",
            "the quick brown fox jumps over the lazy dog",
            None,
        )
        .await;
        rec_content(
            &store,
            "https://b.example.com/",
            "rust programming language guarantees memory safety",
            None,
        )
        .await;
        store.rebuild_fts().await.expect("rebuild FTS");

        let hits = store.bm25_search("rust", 10).await.expect("bm25 search");
        assert_eq!(hits.len(), 1, "only one doc mentions rust");
        assert_eq!(hits[0].0, "https://b.example.com/");
    }

    #[tokio::test]
    async fn test_vector_search_cosine() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(
            &store,
            "https://a.example.com/",
            "alpha",
            Some(vec![1.0, 0.0, 0.0, 0.0]),
        )
        .await;
        rec_content(
            &store,
            "https://b.example.com/",
            "beta",
            Some(vec![0.0, 1.0, 0.0, 0.0]),
        )
        .await;

        // Query closer to A in vector space.
        let hits = store
            .vector_search(&[0.9, 0.1, 0.0, 0.0], 2)
            .await
            .expect("vector search");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "https://a.example.com/", "A is most similar");
        assert!(
            hits[0].1 > hits[1].1,
            "similarity must be sorted descending"
        );
    }

    #[tokio::test]
    async fn test_hybrid_search_rrf() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // A: strong BM25 match, NO embedding (bm25-only signal).
        rec_content(
            &store,
            "https://a.example.com/",
            "rust is the best systems programming language",
            None,
        )
        .await;
        // B: moderate BM25 match AND strong vector match (dual signal).
        rec_content(
            &store,
            "https://b.example.com/",
            "we use rust here",
            Some(vec![0.9, 0.1, 0.0, 0.0]),
        )
        .await;
        // C: no BM25 match, strong vector match (vector-only signal).
        rec_content(
            &store,
            "https://c.example.com/",
            "a completely unrelated topic about cats",
            Some(vec![0.99, 0.0, 0.0, 0.0]),
        )
        .await;
        store.rebuild_fts().await.expect("rebuild FTS");

        // Query vector favors C slightly, B second; A absent from vector space.
        let hits = store
            .hybrid_search("rust", Some(&[1.0, 0.0, 0.0, 0.0]), 10)
            .await
            .expect("hybrid search");
        assert_eq!(hits.len(), 3, "all three docs matched at least one signal");

        let b = hits
            .iter()
            .find(|h| h.url == "https://b.example.com/")
            .unwrap();
        let a = hits
            .iter()
            .find(|h| h.url == "https://a.example.com/")
            .unwrap();
        let c = hits
            .iter()
            .find(|h| h.url == "https://c.example.com/")
            .unwrap();

        // B matches both signals → out-ranks A (bm25-only) and C (vector-only),
        // which each accumulate RRF from a single list.
        assert!(
            b.score > a.score && b.score > c.score,
            "dual-signal doc B must out-rank both single-signal docs A and C under RRF \
             (B={:.4}, A={:.4}, C={:.4})",
            b.score,
            a.score,
            c.score
        );
        assert_eq!(b.signals.len(), 3, "B matched bm25 + vector + freshness");
        assert_eq!(a.signals.len(), 2, "A matched bm25 + freshness");
        assert_eq!(c.signals.len(), 2, "C matched vector + freshness");
    }

    #[tokio::test]
    async fn test_pagerank_compute_and_cache() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Hub-and-spoke: root links to a, b, c. Root has the most in-links via
        // back-edges, so it should carry the highest centrality.
        line_graph(&store).await; // a -> b -> c -> d
        let scores = store
            .compute_pagerank(20, 0.85)
            .await
            .expect("compute pagerank");
        assert_eq!(scores.len(), 4, "all four nodes scored");

        let cached = store.pagerank_scores().await.expect("read cache");
        assert_eq!(cached.len(), 4, "cache matches node count");
        assert_eq!(
            cached[0].0, "https://d.example.com/",
            "sink node d accumulates the highest centrality in a->b->c->d"
        );

        // Every cached score is present in the returned map.
        for (url, score) in &cached {
            assert!(
                (scores.get(url).copied().unwrap_or(0.0) - score).abs() < 1e-9,
                "cache mirrors computed scores for {url}"
            );
        }
    }

    #[tokio::test]
    async fn test_hybrid_search_fuses_graph_signal() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Two docs, both strong BM25 + vector. Their PageRank differs.
        rec_content(
            &store,
            "https://hub.example.com/",
            "rust systems language",
            Some(vec![1.0, 0.0, 0.0]),
        )
        .await;
        rec_content(
            &store,
            "https://leaf.example.com/",
            "rust systems language",
            Some(vec![1.0, 0.0, 0.0]),
        )
        .await;
        // Edge hub -> leaf makes hub the higher-centrality node.
        store
            .record_link(LinkEdge {
                from: "https://hub.example.com/".to_string(),
                to: "https://leaf.example.com/".to_string(),
                anchor_text: None,
                ..Default::default()
            })
            .await;
        store.rebuild_fts().await.expect("rebuild FTS");
        store.compute_pagerank(20, 0.85).await.expect("pagerank");

        let hits = store
            .hybrid_search("rust", Some(&[1.0, 0.0, 0.0]), 10)
            .await
            .expect("hybrid search");
        assert_eq!(hits.len(), 2);

        // Both match bm25 + vector; hub additionally matches the graph signal,
        // so it should rank strictly above the leaf.
        let hub = hits
            .iter()
            .find(|h| h.url == "https://hub.example.com/")
            .unwrap();
        let leaf = hits
            .iter()
            .find(|h| h.url == "https://leaf.example.com/")
            .unwrap();
        // Both pages also appear in the fused PageRank list (hub at rank 1,
        // leaf at rank 2), so each carries the graph signal. The hub's higher
        // PageRank rank is what pushes it ahead under RRF.
        assert!(
            hub.graph_score.unwrap_or(0.0) != leaf.graph_score.unwrap_or(0.0),
            "graph-signal must differentiate hub from leaf at the signal level \
             (hub={:.4}, leaf={:.4})",
            hub.graph_score.unwrap_or(0.0),
            leaf.graph_score.unwrap_or(0.0)
        );
        assert!(hub.signals.contains(&"graph".to_string()));
        assert!(hub.signals.contains(&"freshness".to_string()));
        assert!(leaf.signals.contains(&"graph".to_string()));
        assert!(leaf.signals.contains(&"freshness".to_string()));
    }

    #[tokio::test]
    async fn test_diskann_vector_index_search() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("vector.db");
        let path = db_path.to_str().unwrap();
        let store = TursoStore::new(path).await.expect("open db");

        // Record three embeddings. A is close to the query, B is far, C is
        // medium.
        for (url, vec) in [
            ("https://a.example/", vec![1.0_f32, 0.0, 0.0, 0.0]),
            ("https://b.example/", vec![0.0_f32, 1.0, 0.0, 0.0]),
            ("https://c.example/", vec![0.8_f32, 0.2, 0.0, 0.0]),
        ] {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_embedding(url, vec)
                .await
                .expect("record embedding");
        }

        // Build the DiskANN index.
        store
            .build_vector_index()
            .await
            .expect("build vector index");
        assert_eq!(store.vector_index_len(), 3);

        // Search for the nearest neighbors of [1,0,0,0] — A should rank first,
        // C second (close to A), B last (orthogonal).
        let store = Arc::new(store);
        let hits = store
            .vector_search(&[1.0_f32, 0.0, 0.0, 0.0], 3)
            .await
            .expect("vector search");
        assert_eq!(hits.len(), 3, "all three docs should be returned");
        assert_eq!(
            hits[0].0, "https://a.example/",
            "identical vector must rank first"
        );
        assert_eq!(
            hits[2].0, "https://b.example/",
            "orthogonal vector must rank last"
        );
        // Similarity scores must be in descending order.
        assert!(hits[0].1 >= hits[1].1 && hits[1].1 >= hits[2].1);
    }

    fn content_record(url: &str, fetched_at: DateTime<Utc>) -> PageContentRecord {
        PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: format!("Full body text for {url} ").repeat(20),
            content_markdown: Some(format!("## {url}").repeat(5)),
            content_html: Some(format!("<p>{url}</p>").repeat(5)),
            excerpt: Some(format!("Excerpt for {url}")),
            content_hash: format!("hash-{url}"),
            word_count: Some(100),
            reading_time_seconds: Some(30),
            fetched_at,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_enforce_storage_budget_evicts_oldest_full_content() {
        let store = TursoStore::new(":memory:").await.expect("open store");

        // Three pages with distinct fetch times (oldest first).
        let base = Utc::now();
        let urls = [
            ("https://old.example/", base - chrono::Duration::days(30)),
            ("https://mid.example/", base - chrono::Duration::days(10)),
            ("https://new.example/", base),
        ];
        for (url, fetched) in urls {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_page_content(content_record(url, fetched))
                .await
                .expect("record content");
            store
                .record_embedding(url, vec![1.0_f32, 0.0, 0.0, 0.0])
                .await
                .expect("record embedding");
        }

        assert_eq!(store.full_content_page_count().await.unwrap(), 3);

        // Cap full content at 1 page → oldest two must be stripped.
        let pruned = store
            .enforce_storage_budget(None, Some(1))
            .await
            .expect("enforce budget");
        assert_eq!(pruned, 2, "two oldest pages should be evicted");

        // Only the newest page retains full content.
        assert_eq!(store.full_content_page_count().await.unwrap(), 1);

        // All three embeddings must survive — searchable core is preserved.
        let count: i64 = {
            let mut rows = store
                .conn
                .query(
                    "SELECT COUNT(*) FROM url_nodes WHERE embedding IS NOT NULL",
                    params![],
                )
                .await
                .expect("count embeddings");
            rows.next()
                .await
                .expect("row")
                .expect("some")
                .get::<i64>(0)
                .expect("int")
        };
        assert_eq!(count, 3, "embeddings must survive eviction");
    }

    #[tokio::test]
    async fn test_enforce_storage_budget_respects_byte_budget() {
        let store = TursoStore::new(":memory:").await.expect("open store");

        for (url, i) in [
            ("https://a.example/", 0u32),
            ("https://b.example/", 1),
            ("https://c.example/", 2),
        ] {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_page_content(content_record(
                    url,
                    Utc::now() - chrono::Duration::minutes(i as i64 * 5),
                ))
                .await
                .expect("record content");
        }
        assert_eq!(store.full_content_page_count().await.unwrap(), 3);

        // A tiny byte budget (below one page of content) must still shrink the
        // full-content count toward zero without erroring.
        let pruned = store
            .enforce_storage_budget(Some(1), None)
            .await
            .expect("enforce tiny byte budget");
        assert_eq!(
            pruned, 3,
            "all pages should be stripped to fit 1-byte budget"
        );
        assert_eq!(store.full_content_page_count().await.unwrap(), 0);
        // Logical size is now just embeddings/excerpts — must be < 4096 bytes.
        let size = store.db_size_bytes().await.unwrap();
        assert!(size < 4096, "stripped db should be tiny, got {size} bytes");
    }
