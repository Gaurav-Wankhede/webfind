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
    /// Deterministic regex-extracted entities from the clean text, available to
    /// the LLM without calling an extraction model.
    pub entities: Entities,
}

/// Structured entities extracted from page text via deterministic regex.
/// Populated in the fetch pipeline before content reaches the LLM so models
/// receive clean, machine-checkable facts instead of raw page text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entities {
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub addresses: Vec<String>,
    pub urls: Vec<String>,
    pub prices: Vec<String>,
    pub dates: Vec<String>,
    pub ip_addresses: Vec<String>,
    pub social_handles: Vec<String>,
}

impl Entities {
    /// True when no entities were extracted.
    pub fn is_empty(&self) -> bool {
        self.emails.is_empty()
            && self.phones.is_empty()
            && self.addresses.is_empty()
            && self.urls.is_empty()
            && self.prices.is_empty()
            && self.dates.is_empty()
            && self.ip_addresses.is_empty()
            && self.social_handles.is_empty()
    }
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

/// Persisted full-page content stored in the knowledge graph.
/// Mirrors the `page_content` SurrealDB table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageContentRecord {
    pub url_node: String,
    pub content_text: String,
    pub content_markdown: Option<String>,
    pub content_html: Option<String>,
    pub excerpt: Option<String>,
    pub content_hash: String,
    pub word_count: Option<u32>,
    pub reading_time_seconds: Option<u32>,
    pub fetched_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

impl PageContentRecord {
    /// Build from a StructuredContent blob and a url_node record ID.
    pub fn from_content(content: &StructuredContent, url_node_id: impl Into<String>) -> Self {
        Self {
            url_node: url_node_id.into(),
            content_text: content.content_text.clone(),
            content_markdown: Some(content.content_markdown.clone()),
            content_html: Some(content.content_html.clone()),
            excerpt: Some(content.excerpt.clone()),
            content_hash: format!("{:016x}", {
                use std::collections::hash_map::DefaultHasher;
                use std::hash::{Hash, Hasher};
                let mut hasher = DefaultHasher::new();
                content.content_text.hash(&mut hasher);
                hasher.finish()
            }),
            word_count: Some(content.word_count),
            reading_time_seconds: Some(content.reading_time_seconds),
            fetched_at: content.fetched_at,
            created_at: Utc::now(),
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
