use std::collections::HashMap;

use anyhow::Result;

use crate::schema::content::StructuredContent;
use crate::schema::response::{IndexFreshness, SearchMetadata, SearchResult};

use super::search_engine::SearchEngine;

const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Build search metadata from a search engine.
pub async fn build_metadata(
    engine: &(dyn SearchEngine + Send + Sync),
    signals: Vec<String>,
) -> Result<SearchMetadata> {
    let count = engine.doc_count().await?;
    Ok(SearchMetadata {
        index_version: "1".to_string(),
        index_size: count,
        engine_version: ENGINE_VERSION.to_string(),
        searched_at: chrono::Utc::now(),
        signals_used: signals,
        index_freshness: IndexFreshness {
            oldest_page: None,
            newest_page: None,
            avg_age_days: 0.0,
        },
    })
}

/// Attach full content blocks to search results when the caller has the
/// raw structured content available (e.g. after a fresh crawl).
/// Content is truncated to `MAX_CONTENT_CHARS` per result to keep responses
/// within context-window limits.
pub fn attach_content(results: &mut [SearchResult], contents: &[StructuredContent]) {
    const MAX_CONTENT_CHARS: usize = 64 * 1024;
    let mut by_url: HashMap<&str, &StructuredContent> = HashMap::with_capacity(contents.len());
    for c in contents {
        by_url.insert(c.url.as_str(), c);
    }
    for r in results {
        if let Some(c) = by_url.get(r.url.as_str()) {
            let mut block = c.to_content_block();
            if block.text.chars().count() > MAX_CONTENT_CHARS {
                let truncated: String = block.text.chars().take(MAX_CONTENT_CHARS).collect();
                block.text = truncated;
                block.markdown = block.markdown.map(|m| {
                    if m.chars().count() > MAX_CONTENT_CHARS {
                        m.chars().take(MAX_CONTENT_CHARS).collect()
                    } else {
                        m
                    }
                });
            }
            r.content = Some(block);
            r.modified_at = c.modified_at;
            r.author = c.author.clone();
            r.site_name = c.site_name.clone();
            if !c.keywords.is_empty() {
                r.keywords = Some(c.keywords.clone());
            }
            r.favicon = c.favicon.clone();
            r.language = c.language.clone();
            r.content_type = c.content_type.clone();
            let avg_words = if c.sentence_count > 0 {
                c.word_count as f64 / c.sentence_count as f64
            } else {
                0.0
            };
            // Gunning fog index: 0.4 * ((words / sentences) + 100 * (complex_words / words))
            // When syllable data is not pre-computed, estimate complex words via Flesch-Kincaid grade:
            // FK = 0.39 * (words/sentences) + 11.8 * (syllables/words) - 15.59
            let fog = if c.sentence_count > 0 && c.word_count > 0 {
                let estimated_complex_pct = ((c.grade_level - (0.39 * avg_words) + 15.59).max(0.0) / 11.8) * 0.3;
                (0.4 * (avg_words + (estimated_complex_pct * 100.0).clamp(0.0, 100.0))).clamp(0.0, 30.0)
            } else {
                c.grade_level.clamp(0.0, 30.0)
            };
            r.metrics = Some(crate::schema::response::ContentMetrics {
                reading_ease: c.reading_ease,
                grade_level: c.grade_level,
                fog_index: (fog * 10.0).round() / 10.0,
                sentence_count: c.sentence_count,
                avg_words_per_sentence: (avg_words * 10.0).round() / 10.0,
                language: c.language.clone(),
                language_confidence: c.language_confidence,
                has_structured_data: !c.json_ld.is_empty() || c.schema_type.is_some(),
                schema_type: c.schema_type.clone(),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::search_engine::InMemorySearchEngine;
    use crate::schema::content::StructuredContent;
    use chrono::{TimeZone, Utc};
    use std::sync::Arc;

    fn sample(url: &str, title: &str, body: &str) -> StructuredContent {
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
            reading_time_seconds: 30,
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
            entities: crate::schema::content::Entities::default(),
        }
    }

    #[tokio::test]
    async fn test_indexer_search_roundtrip() {
        let engine = Arc::new(InMemorySearchEngine::new());

        let docs = vec![
            sample(
                "https://rust-lang.org",
                "Rust Language",
                "Rust is a systems language for reliability and performance.",
            ),
            sample(
                "https://go.dev",
                "Go Language",
                "Go is a simple language for building fast concurrent software.",
            ),
            sample(
                "https://typescriptlang.org",
                "TypeScript",
                "TypeScript adds static types to JavaScript for large scale apps.",
            ),
        ];

        let count = engine.index_batch(&docs).await.unwrap();
        assert_eq!(count, 3);

        let results = engine.search_bm25("systems language", 10).await.unwrap();
        assert!(!results.is_empty());
        assert!(results[0].title.contains("Rust"));

        let count = engine.doc_count().await.unwrap();
        assert_eq!(count, 3);
    }

    #[tokio::test]
    async fn test_indexer_vector_roundtrip() {
        use crate::engine::embedder::DummyEmbedder;
        let engine = Arc::new(InMemorySearchEngine::with_embedder(Arc::new(DummyEmbedder)));

        let docs = vec![
            sample(
                "https://rust-lang.org",
                "Rust Language",
                "Rust is a systems language for reliability and performance.",
            ),
            sample(
                "https://go.dev",
                "Go Language",
                "Go is a simple language for building fast concurrent software.",
            ),
        ];
        engine.index_batch(&docs).await.unwrap();
        let hits = engine.search_vector("systems language", 5).await.unwrap();
        assert!(hits.contains_key("https://rust-lang.org"));
    }
}
