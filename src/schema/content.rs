use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::response::{ContentBlock, Keyword};

/// Clean structured content extracted from a web page
/// Output of the Fetcher layer, input to the Indexer
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredContent {
    pub url: String,
    pub final_url: String,
    pub status_code: u16,
    pub title: String,
    pub description: Option<String>,
    pub canonical_url: Option<String>,
    pub language: String,
    pub language_confidence: f64,
    pub published_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub author: Option<String>,
    pub site_name: Option<String>,
    pub content_text: String,
    pub content_html: String,
    pub content_markdown: String,
    pub excerpt: String,
    pub word_count: u32,
    pub char_count: u32,
    pub sentence_count: u32,
    pub reading_time_seconds: u32,
    pub reading_ease: f64,
    pub grade_level: f64,
    pub keywords: Vec<Keyword>,
    pub open_graph: Option<OpenGraph>,
    pub twitter_card: Option<TwitterCard>,
    pub json_ld: Vec<serde_json::Value>,
    pub schema_type: Option<String>,
    pub images: Vec<ImageInfo>,
    pub internal_links: Vec<String>,
    pub external_links: Vec<String>,
    pub favicon: Option<String>,
    pub rss_url: Option<String>,
    pub normalized_text: String,
    pub fetched_at: DateTime<Utc>,
    pub fetch_duration_ms: u64,
    pub html_size_bytes: u64,
    pub encoding: Option<String>,
    pub ssl_valid: bool,
    pub redirect_count: u8,
    pub is_paywalled: bool,
    pub is_valid_content: bool,
}

impl StructuredContent {
    /// Build a full-text content block for search responses.
    pub fn to_content_block(&self) -> ContentBlock {
        ContentBlock {
            text: self.content_text.clone(),
            excerpt: self.excerpt.clone(),
            word_count: self.word_count,
            reading_time_seconds: self.reading_time_seconds,
            html: Some(self.content_html.clone()),
            markdown: Some(self.content_markdown.clone()),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenGraph {
    pub title: Option<String>,
    pub r#type: Option<String>,
    pub image: Option<String>,
    pub url: Option<String>,
    pub description: Option<String>,
    pub site_name: Option<String>,
    pub locale: Option<String>,
    pub article_author: Option<String>,
    pub article_published_time: Option<String>,
    pub article_modified_time: Option<String>,
    pub article_section: Option<String>,
    pub article_tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwitterCard {
    pub card: Option<String>,
    pub site: Option<String>,
    pub creator: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageInfo {
    pub url: String,
    pub alt: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
