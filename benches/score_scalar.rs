use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use tokio::runtime::Runtime;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::schema::content::StructuredContent;

fn create_test_content(n: usize) -> Vec<StructuredContent> {
    (0..n)
        .map(|i| {
            StructuredContent {
                url: format!("https://example.com/page{}", i),
                final_url: format!("https://example.com/page{}", i),
                status_code: 200,
                title: format!("Test Page {}", i),
                description: Some(format!("Description for page {}", i)),
                canonical_url: Some(format!("https://example.com/page{}", i)),
                language: "en".to_string(),
                language_confidence: 0.99,
                published_at: Some(chrono::Utc::now()),
                modified_at: Some(chrono::Utc::now()),
                author: Some("Test Author".to_string()),
                site_name: Some("Example Site".to_string()),
                content_text: format!("This is the content of page {}. It contains some words for testing BM25 scoring. The quick brown fox jumps over the lazy dog. Rust is a systems programming language. Search engines use BM25 for ranking.", i),
                content_html: format!("<html><body><p>Content for page {}</p></body></html>", i),
                content_markdown: format!("# Page {i}\n\nContent for page {i}"),
                excerpt: format!("Excerpt for page {}", i),
                word_count: 100,
                char_count: 800,
                sentence_count: 8,
                reading_time_seconds: 30,
                reading_ease: 70.0,
                grade_level: 8.0,
                keywords: vec![],
                open_graph: None,
                twitter_card: None,
                json_ld: vec![],
                schema_type: None,
                images: vec![],
                internal_links: vec![],
                external_links: vec![],
                favicon: None,
                rss_url: None,
                normalized_text: format!("normalized content for page {}", i),
                fetched_at: chrono::Utc::now(),
                fetch_duration_ms: 100,
                html_size_bytes: 2000,
                encoding: Some("utf-8".to_string()),
                ssl_valid: true,
                redirect_count: 0,
                content_type: "text/html".to_string(),
                content_type_header: "text/html; charset=utf-8".to_string(),
                is_paywalled: false,
                is_valid_content: true,
                entities: webfind::schema::content::Entities::default(),
            }
        })
        .collect()
}

fn bench_bm25_scoring(c: &mut Criterion) {
    let mut group = c.benchmark_group("bm25_scoring");
    let rt = Runtime::new().unwrap();

    for size in [100, 1000, 10000].iter() {
        let contents = create_test_content(*size);
        let engine = InMemorySearchEngine::new();

        // Index the content
        for content in &contents {
            rt.block_on(engine.index_one(content)).unwrap();
        }

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &engine, |b, engine| {
            b.iter(|| {
                let query = "rust programming language";
                let results = rt.block_on(engine.search_bm25(query, 10)).unwrap();
                black_box(results);
            });
        });
    }

    group.finish();
}

fn bench_bm25_scoring_with_different_queries(c: &mut Criterion) {
    let mut group = c.benchmark_group("bm25_query_variations");
    let rt = Runtime::new().unwrap();

    let contents = create_test_content(1000);
    let engine = InMemorySearchEngine::new();
    for content in &contents {
        rt.block_on(engine.index_one(content)).unwrap();
    }

    let queries = [
        "rust",
        "programming language",
        "quick brown fox",
        "search engine ranking",
        "systems programming",
        "nonexistent query term xyz",
    ];

    for query in queries.iter() {
        group.bench_with_input(BenchmarkId::from_parameter(query), query, |b, query| {
            b.iter(|| {
                let results = rt
                    .block_on(engine.search_bm25(black_box(*query), 10))
                    .unwrap();
                black_box(results);
            });
        });
    }

    group.finish();
}

fn bench_vector_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("vector_search");
    let rt = Runtime::new().unwrap();

    for size in [100, 1000, 5000].iter() {
        let contents = create_test_content(*size);

        let engine = InMemorySearchEngine::new();
        for content in &contents {
            rt.block_on(engine.index_one(content)).unwrap();
        }

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &engine, |b, engine| {
            b.iter(|| {
                let results = rt.block_on(engine.search_vector("test query", 10)).unwrap();
                black_box(results);
            });
        });
    }

    group.finish();
}

fn bench_index_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_batch");
    let rt = Runtime::new().unwrap();

    for size in [100, 1000, 10000].iter() {
        let contents = create_test_content(*size);
        let engine = InMemorySearchEngine::new();

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(size), &engine, |b, engine| {
            b.iter(|| {
                let count = rt.block_on(engine.index_batch(&contents)).unwrap();
                black_box(count);
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_bm25_scoring,
    bench_bm25_scoring_with_different_queries,
    bench_vector_search,
    bench_index_batch
);
criterion_main!(benches);
