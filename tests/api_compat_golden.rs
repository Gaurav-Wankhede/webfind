//! API compatibility golden tests.
//!
//! These lock the **public JSON contract** of the search API — the exact field
//! names, nesting, and types that CLI/REST/MCP consumers depend on. Any change
//! to the response schema (adding/renaming/removing a field) is a breaking
//! change for API consumers, so it must be a deliberate, reviewed decision.
//!
//! Each test renders a fully-populated `SearchResponse` and compares the
//! serialized output to a stored snapshot. Run `cargo insta review` (or set
//! `INSTA_UPDATE=always`) to accept intentional schema changes.

use chrono::{TimeZone, Utc};
use serde_json::json;
use webfind::schema::request::SearchDepth;
use webfind::schema::response::{
    ContentBlock, ContentMetrics, GraphSummary, IndexFreshness, Keyword, ScoreBreakdown,
    SearchMetadata, SearchResponse, SearchResult,
};

fn fixed_ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 12, 12, 0, 0).unwrap()
}

/// Build a fully-populated `SearchResponse` with deterministic values so the
/// golden snapshots are stable across runs.
fn sample_response() -> SearchResponse {
    let result = SearchResult {
        rank: 1,
        url: "https://rust-lang.org".to_string(),
        title: "The Rust Programming Language".to_string(),
        snippet: "Rust is a systems language focused on safety, speed, and concurrency."
            .to_string(),
        domain: "rust-lang.org".to_string(),
        published_at: Some(fixed_ts()),
        modified_at: None,
        crawled_at: fixed_ts(),
        author: Some("Core Team".to_string()),
        site_name: Some("Rust".to_string()),
        score: 12.5,
        scores: ScoreBreakdown {
            bm25: Some(10.0),
            vector: Some(2.5),
            graph: Some(1.0),
            freshness: Some(0.5),
            quality: Some(0.0),
            ax_score: None,
            final_score: 12.5,
        },
        content: Some(ContentBlock {
            text: "Rust is a systems language focused on safety, speed, and concurrency."
                .to_string(),
            excerpt: "Rust is a systems language".to_string(),
            word_count: 10,
            reading_time_seconds: 3,
            html: Some("<p>Rust</p>".to_string()),
            markdown: Some("# Rust".to_string()),
        }),
        keywords: Some(vec![Keyword {
            text: "rust".to_string(),
            tfidf_score: 0.8,
            rank: 1,
        }]),
        metrics: Some(ContentMetrics {
            reading_ease: 65.0,
            grade_level: 8.0,
            fog_index: 10.0,
            sentence_count: 1,
            avg_words_per_sentence: 10.0,
            language: "en".to_string(),
            language_confidence: 0.99,
            has_structured_data: false,
            schema_type: None,
        }),
        favicon: Some("https://rust-lang.org/favicon.ico".to_string()),
        thumbnail: None,
        llms_txt: None,
        ai_catalog: None,
        openapi_spec: None,
        mcp_server: None,
        language: "en".to_string(),
        content_type: "text/html".to_string(),
    };

    SearchResponse {
        request_id: "golden-test-1".to_string(),
        query: "rust systems language".to_string(),
        depth: SearchDepth::Standard,
        total_results: 1,
        returned: 1,
        latency_ms: 42,
        results: vec![result],
        suggestions: vec!["rust ownership".to_string()],
        related: vec!["go concurrency".to_string()],
        graph: Some(GraphSummary {
            inbound_links: vec![],
            outbound_links: vec![],
            related_domains: vec![],
            domain_authority: vec![],
        }),
        metadata: SearchMetadata {
            index_version: "1".to_string(),
            index_size: 128,
            engine_version: "0.1.0".to_string(),
            searched_at: fixed_ts(),
            signals_used: vec!["bm25".to_string(), "vector".to_string()],
            index_freshness: IndexFreshness {
                oldest_page: Some(fixed_ts()),
                newest_page: Some(fixed_ts()),
                avg_age_days: 1.5,
            },
        },
    }
}

/// The full `SearchResponse` serialized to JSON is the REST/MCP wire contract.
/// Locks field names, nesting, and presence of optional fields.
#[test]
fn search_response_json_contract_is_stable() {
    let response = sample_response();
    let value = serde_json::to_value(&response).unwrap();
    insta::assert_json_snapshot!("search_response_contract", value);
}

/// The LLM-friendly view (`to_llm_value`) is what MCP agents receive. It must
/// remain minimal and stable — a change here changes what the agent sees.
#[test]
fn llm_view_contract_is_stable() {
    let response = sample_response();
    insta::assert_json_snapshot!("llm_view_contract", response.to_llm_value());
}

/// Deterministic formatting contract: the same response must always produce the
/// same JSON string (field ordering is stable because serde_json preserves
/// struct declaration order for structs).
#[test]
fn json_is_deterministic() {
    let a = serde_json::to_string(&sample_response()).unwrap();
    let b = serde_json::to_string(&sample_response()).unwrap();
    assert_eq!(a, b, "serialization must be deterministic across runs");

    // Spot-check a few top-level keys exist with correct types.
    let value: serde_json::Value = serde_json::from_str(&a).unwrap();
    assert_eq!(value["query"], json!("rust systems language"));
    assert_eq!(value["total_results"], json!(1));
    assert!(value["results"][0]["scores"]["bm25"].is_number());
    assert!(value["metadata"]["signals_used"].is_array());
}
