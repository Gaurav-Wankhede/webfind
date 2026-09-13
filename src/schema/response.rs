use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::request::SearchDepth;

/// Top-level search response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResponse {
    pub request_id: String,
    pub query: String,
    pub depth: SearchDepth,
    pub total_results: u64,
    pub returned: u32,
    pub latency_ms: u64,
    pub results: Vec<SearchResult>,
    pub suggestions: Vec<String>,
    pub related: Vec<String>,
    pub graph: Option<GraphSummary>,
    pub metadata: SearchMetadata,
}

/// Individual search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub rank: u32,
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub domain: String,
    pub published_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub crawled_at: DateTime<Utc>,
    pub author: Option<String>,
    pub site_name: Option<String>,
    pub score: f64,
    pub scores: ScoreBreakdown,
    pub content: Option<ContentBlock>,
    pub keywords: Option<Vec<Keyword>>,
    pub metrics: Option<ContentMetrics>,
    pub favicon: Option<String>,
    pub thumbnail: Option<String>,
    pub language: String,
    pub content_type: String,
}

/// Score breakdown by ranking signal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub bm25: Option<f64>,
    pub vector: Option<f64>,
    pub graph: Option<f64>,
    pub freshness: Option<f64>,
    pub quality: Option<f64>,
    pub final_score: f64,
}

/// Content block with normalized text
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentBlock {
    pub text: String,
    pub excerpt: String,
    pub word_count: u32,
    pub reading_time_seconds: u32,
    pub html: Option<String>,
    pub markdown: Option<String>,
}

/// Extracted keyword
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keyword {
    pub text: String,
    pub tfidf_score: f64,
    pub rank: u32,
}

/// Content quality metrics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContentMetrics {
    pub reading_ease: f64,
    pub grade_level: f64,
    pub fog_index: f64,
    pub sentence_count: u32,
    pub avg_words_per_sentence: f64,
    pub language: String,
    pub language_confidence: f64,
    pub has_structured_data: bool,
    pub schema_type: Option<String>,
}

/// Graph summary for search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSummary {
    pub inbound_links: Vec<GraphEdge>,
    pub outbound_links: Vec<GraphEdge>,
    pub related_domains: Vec<DomainNode>,
    pub domain_authority: Vec<DomainAuthority>,
}

/// Graph edge (link relationship)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source_url: String,
    pub target_url: String,
    pub anchor_text: Option<String>,
    pub crawled_at: DateTime<Utc>,
}

/// Domain node in graph
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainNode {
    pub domain: String,
    pub page_count: u32,
    pub avg_authority: f64,
}

/// Domain authority score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainAuthority {
    pub domain: String,
    pub authority_score: f64,
    pub inbound_links: u32,
    pub outbound_links: u32,
}

/// Search metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMetadata {
    pub index_version: String,
    pub index_size: u64,
    pub engine_version: String,
    pub searched_at: DateTime<Utc>,
    pub signals_used: Vec<String>,
    pub index_freshness: IndexFreshness,
}

/// Index freshness info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexFreshness {
    pub oldest_page: Option<DateTime<Utc>>,
    pub newest_page: Option<DateTime<Utc>>,
    pub avg_age_days: f64,
}

impl SearchResponse {
    /// Render a concise, LLM-friendly view of the response.
    ///
    /// Strips internal score noise and keeps only the information a model needs
    /// to answer a question: title, URL, dates, author, site, excerpt, and —
    /// when requested — the full normalized content block.
    pub fn to_llm_value(&self) -> serde_json::Value {
        let results: Vec<serde_json::Value> = self
            .results
            .iter()
            .map(|r| {
                let mut obj = json!({
                    "rank": r.rank,
                    "title": r.title,
                    "url": r.url,
                    "domain": r.domain,
                    "excerpt": r.snippet,
                    "published_at": r.published_at,
                    "modified_at": r.modified_at,
                    "author": r.author,
                    "site_name": r.site_name,
                    "language": r.language,
                });
                if let Some(ref content) = r.content {
                    obj["word_count"] = json!(content.word_count);
                    obj["reading_time_seconds"] = json!(content.reading_time_seconds);
                    obj["content"] = json!({
                        "text": content.text,
                        "excerpt": content.excerpt,
                        "markdown": content.markdown,
                        "word_count": content.word_count,
                        "reading_time_seconds": content.reading_time_seconds,
                    });
                }
                obj
            })
            .collect();

        let mut value = json!({
            "query": self.query,
            "total_results": self.total_results,
            "returned": self.returned,
            "results": results,
        });

        if self.graph.is_some() {
            value["graph"] = json!(self.graph);
        }

        value["metadata"] = json!({
            "index_size": self.metadata.index_size,
            "engine_version": self.metadata.engine_version,
            "searched_at": self.metadata.searched_at,
            "signals_used": self.metadata.signals_used,
        });

        value
    }
}
