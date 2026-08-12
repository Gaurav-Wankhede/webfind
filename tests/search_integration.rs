use std::sync::Arc;

use chrono::{TimeZone, Utc};
use webfind::engine::indexer::{attach_content, build_metadata};
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::report::format_response;
use webfind::schema::content::StructuredContent;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::SearchResponse;

fn make_content(url: &str, title: &str, body: &str) -> StructuredContent {
    StructuredContent {
        url: url.to_string(),
        final_url: url.to_string(),
        status_code: 200,
        title: title.to_string(),
        description: None,
        canonical_url: None,
        language: "en".to_string(),
        language_confidence: 0.95,
        published_at: Some(Utc.with_ymd_and_hms(2025, 6, 15, 10, 0, 0).unwrap()),
        modified_at: None,
        author: None,
        site_name: None,
        content_text: body.to_string(),
        content_html: format!("<p>{}</p>", body),
        content_markdown: body.to_string(),
        excerpt: body.chars().take(200).collect(),
        word_count: body.split_whitespace().count() as u32,
        char_count: body.len() as u32,
        sentence_count: 1,
        reading_time_seconds: (body.split_whitespace().count() as u32 * 3) / 10,
        reading_ease: 65.0,
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
        normalized_text: body.to_string(),
        fetched_at: Utc::now(),
        fetch_duration_ms: 150,
        html_size_bytes: 1024,
        encoding: None,
        ssl_valid: true,
        redirect_count: 0,
        is_paywalled: false,
        is_valid_content: true,
        content_type: "text/html".to_string(),
        content_type_header: "text/html".to_string(),
        entities: webfind::schema::content::Entities::default(),
    }
}

#[tokio::test]
async fn test_full_search_pipeline() {
    let indexer = InMemorySearchEngine::new();

    let docs = vec![
        make_content(
            "https://rust-lang.org",
            "The Rust Programming Language",
            "Rust is a systems language focused on safety, speed, and concurrency. It powers browsers, operating systems, and command-line tools.",
        ),
        make_content(
            "https://go.dev",
            "Go Programming Language",
            "Go is an open-source language that makes it easy to build reliable and efficient software. It features garbage collection and concurrency.",
        ),
        make_content(
            "https://python.org",
            "Python Language",
            "Python is a high-level programming language known for its simplicity and readability. It is widely used in data science and machine learning.",
        ),
        make_content(
            "https://docs.rust-lang.org/book",
            "The Rust Book",
            "This book covers Rust fundamentals including ownership, borrowing, lifetimes, and the type system. Essential reading for new Rust developers.",
        ),
    ];

    let count = indexer.index_batch(&docs).await.unwrap();
    assert_eq!(count, 4);

    // Test BM25 search
    let results = indexer
        .search_bm25("rust systems language", 10)
        .await
        .unwrap();
    assert!(!results.is_empty());
    assert!(
        results[0].title.contains("Rust"),
        "top result should be Rust"
    );

    // Test ranker
    let ranker = Ranker::new();
    let request = SearchRequest {
        query: "rust systems language".to_string(),
        depth: SearchDepth::Shallow,
        limit: 10,
        output: OutputFormat::Json,
        language: None,
        date_range: None,
        domains: None,
        content_type: None,
        include_content: false,
        include_graph: false,
        include_keywords: false,
        include_metrics: false,
        hybrid: false,
    };
    let ranked = ranker.rank(results, &request, None, None);
    assert_eq!(ranked.len(), 4);

    // Test report formatting
    let meta = build_metadata(&indexer, vec!["bm25".to_string()])
        .await
        .unwrap();
    let response = SearchResponse {
        request_id: "test-integration".to_string(),
        query: "rust systems language".to_string(),
        depth: SearchDepth::Shallow,
        total_results: ranked.len() as u64,
        returned: ranked.len() as u32,
        latency_ms: 1,
        results: ranked,
        suggestions: vec!["rust ownership".to_string()],
        related: vec!["go concurrency".to_string()],
        graph: None,
        metadata: meta,
    };

    // All three formats should produce valid output
    let json = format_response(&response, &OutputFormat::Json);
    assert!(json.contains("rust-lang.org"), "JSON should contain URL");

    let report = format_response(&response, &OutputFormat::Report);
    assert!(report.contains("WEBFIND SEARCH RESULTS"), "Report header");
    assert!(
        report.contains("Suggestions"),
        "Report should have suggestions"
    );

    let md = format_response(&response, &OutputFormat::Markdown);
    assert!(md.contains("# WebFind Search"), "Markdown header");
    assert!(
        md.contains("[The Rust Programming Language]"),
        "Markdown link"
    );

    println!("\n=== REPORT ===\n{}", report);
    println!("=== MARKDOWN ===\n{}", md);
}
