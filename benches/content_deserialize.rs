use chrono::Utc;
use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use serde_json;
use webfind::schema::content::StructuredContent;

fn create_test_json(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| {
            let content = StructuredContent {
                url: format!("https://example.com/page{}", i),
                final_url: format!("https://example.com/page{}", i),
                status_code: 200,
                title: format!("Test Page {}", i),
                description: Some(format!("Description for page {}", i)),
                canonical_url: Some(format!("https://example.com/page{}", i)),
                language: "en".to_string(),
                language_confidence: 0.99,
                published_at: Some(Utc::now()),
                modified_at: Some(Utc::now()),
                author: Some("Test Author".to_string()),
                site_name: Some("Example Site".to_string()),
                content_text: format!("This is the content of page {i}. It contains some words for testing JSON deserialization. The quick brown fox jumps over the lazy dog. Rust is a systems programming language. Search engines use BM25 for ranking. This content is long enough to test realistic deserialization performance."),
                content_html: format!("<html><body><p>Content for page {i}</p></body></html>"),
                content_markdown: format!("# Page {i}\n\nContent for page {i}"),
                excerpt: format!("Excerpt for page {i}"),
                word_count: 150,
                char_count: 1000,
                sentence_count: 10,
                reading_time_seconds: 45,
                reading_ease: 65.0,
                grade_level: 8.5,
                keywords: vec![],
                open_graph: None,
                twitter_card: None,
                json_ld: vec![],
                schema_type: None,
                images: vec![],
                internal_links: vec![
                    format!("https://example.com/link{}", i),
                    format!("https://example.com/link{}", i + 1),
                    format!("https://example.com/link{}", i + 2),
                ],
                external_links: vec![],
                favicon: None,
                rss_url: None,
                normalized_text: format!("normalized content for page {}", i),
                fetched_at: Utc::now(),
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
            };
            serde_json::to_string(&content).unwrap()
        })
        .collect()
}

fn bench_json_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_deserialize");

    for size in [100, 1000, 10000].iter() {
        let json_strings = create_test_json(*size);

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &json_strings,
            |b, json_strings| {
                b.iter(|| {
                    for json in json_strings {
                        let content: StructuredContent = serde_json::from_str(json).unwrap();
                        black_box(content);
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_json_serialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_serialize");

    for size in [100, 1000, 10000].iter() {
        let contents: Vec<StructuredContent> = (0..*size)
            .map(|i| StructuredContent {
                url: format!("https://example.com/page{}", i),
                final_url: format!("https://example.com/page{}", i),
                status_code: 200,
                title: format!("Test Page {}", i),
                description: Some(format!("Description for page {}", i)),
                canonical_url: Some(format!("https://example.com/page{}", i)),
                language: "en".to_string(),
                language_confidence: 0.99,
                published_at: Some(Utc::now()),
                modified_at: Some(Utc::now()),
                author: Some("Test Author".to_string()),
                site_name: Some("Example Site".to_string()),
                content_text: format!("Content for page {i}"),
                content_html: format!("<html><body><p>Content for page {i}</p></body></html>"),
                content_markdown: format!("# Page {i}\n\nContent for page {i}"),
                excerpt: format!("Excerpt for page {i}"),
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
                fetched_at: Utc::now(),
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
            })
            .collect();

        group.throughput(Throughput::Elements(*size as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &contents,
            |b, contents| {
                b.iter(|| {
                    for content in contents {
                        let json = serde_json::to_string(&content).unwrap();
                        black_box(json);
                    }
                });
            },
        );
    }

    group.finish();
}

fn bench_json_roundtrip(c: &mut Criterion) {
    let mut group = c.benchmark_group("json_roundtrip");

    let json_strings = create_test_json(1000);

    group.bench_function("deserialize_then_serialize", |b| {
        b.iter(|| {
            for json in &json_strings {
                let content: StructuredContent = serde_json::from_str(json).unwrap();
                let json_out = serde_json::to_string(&content).unwrap();
                black_box(json_out);
            }
        });
    });

    group.finish();
}

fn bench_large_content_deserialize(c: &mut Criterion) {
    let mut group = c.benchmark_group("large_content_deserialize");

    // Create content with large body (10KB)
    let large_content = "x".repeat(10000);
    let content = StructuredContent {
        url: "https://example.com/large".to_string(),
        final_url: "https://example.com/large".to_string(),
        status_code: 200,
        title: "Large Page".to_string(),
        description: Some("Large page description".to_string()),
        canonical_url: Some("https://example.com/large".to_string()),
        language: "en".to_string(),
        language_confidence: 0.99,
        published_at: Some(Utc::now()),
        modified_at: Some(Utc::now()),
        author: Some("Test Author".to_string()),
        site_name: Some("Example Site".to_string()),
        content_text: large_content.clone(),
        content_html: format!("<html><body><p>{}</p></body></html>", large_content),
        content_markdown: format!("# Large Page\n\n{}", large_content),
        excerpt: "Large page excerpt".to_string(),
        word_count: 2000,
        char_count: 10000,
        sentence_count: 100,
        reading_time_seconds: 600,
        reading_ease: 50.0,
        grade_level: 10.0,
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
        normalized_text: large_content.clone(),
        fetched_at: Utc::now(),
        fetch_duration_ms: 500,
        html_size_bytes: 15000,
        encoding: Some("utf-8".to_string()),
        ssl_valid: true,
        redirect_count: 0,
        content_type: "text/html".to_string(),
        content_type_header: "text/html; charset=utf-8".to_string(),
        is_paywalled: false,
        is_valid_content: true,
        entities: webfind::schema::content::Entities::default(),
    };
    let json = serde_json::to_string(&content).unwrap();

    group.bench_function("10kb_content", |b| {
        b.iter(|| {
            let content: StructuredContent = serde_json::from_str(&json).unwrap();
            black_box(content);
        });
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_json_deserialize,
    bench_json_serialize,
    bench_json_roundtrip,
    bench_large_content_deserialize
);
criterion_main!(benches);
