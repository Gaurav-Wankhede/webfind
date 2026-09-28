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
    pub content_type: String,
    pub content_type_header: String,
    pub is_paywalled: bool,
    pub is_valid_content: bool,
    /// Deterministic regex-extracted entities from the clean text, available to
    /// the LLM without calling an extraction model.
    pub entities: Entities,
}

impl StructuredContent {
    /// Convert to a token-optimized compact representation.
    /// Omits heavy duplicate fields (`content_html`, `content_text`, `normalized_text`)
    /// while preserving markdown, clean metadata, and extracted technical entities.
    pub fn to_compact(&self) -> CompactStructuredContent {
        CompactStructuredContent {
            url: self.url.clone(),
            final_url: self.final_url.clone(),
            status_code: self.status_code,
            title: self.title.clone(),
            description: self.description.clone(),
            author: self.author.clone(),
            site_name: self.site_name.clone(),
            language: self.language.clone(),
            published_at: self.published_at,
            content_markdown: self.content_markdown.clone(),
            excerpt: self.excerpt.clone(),
            word_count: self.word_count,
            reading_time_seconds: self.reading_time_seconds,
            reading_ease: self.reading_ease,
            grade_level: self.grade_level,
            keywords: self.keywords.clone(),
            entities: self.entities.clone(),
            internal_links: self.internal_links.clone(),
            external_links: self.external_links.clone(),
            favicon: self.favicon.clone(),
            is_paywalled: self.is_paywalled,
            ssl_valid: self.ssl_valid,
            fetch_duration_ms: self.fetch_duration_ms,
        }
    }
}

/// Token-optimized structured content representation for LLMs and agent harnesses.
/// Strips redundant raw HTML and plain text duplicate representations (~65% token savings)
/// while retaining Markdown body and extracted semantic entities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactStructuredContent {
    pub url: String,
    pub final_url: String,
    pub status_code: u16,
    pub title: String,
    pub description: Option<String>,
    pub author: Option<String>,
    pub site_name: Option<String>,
    pub language: String,
    pub published_at: Option<DateTime<Utc>>,
    pub content_markdown: String,
    pub excerpt: String,
    pub word_count: u32,
    pub reading_time_seconds: u32,
    pub reading_ease: f64,
    pub grade_level: f64,
    pub keywords: Vec<Keyword>,
    pub entities: Entities,
    pub internal_links: Vec<String>,
    pub external_links: Vec<String>,
    pub favicon: Option<String>,
    pub is_paywalled: bool,
    pub ssl_valid: bool,
    pub fetch_duration_ms: u64,
}

/// Structured entities extracted from page content via deterministic AST & regex.
/// Populated in the fetch pipeline before content reaches the LLM so models
/// receive clean, machine-checkable facts instead of raw page text.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Entities {
    // Technical / Document Structure
    pub code_blocks: Vec<CodeBlockEntity>,
    pub tables: Vec<TableEntity>,
    pub headings: Vec<HeadingEntity>,
    pub diagrams: Vec<DiagramEntity>,
    pub examples: Vec<ExampleEntity>,
    pub faqs: Vec<FaqEntity>,
    pub steps: Vec<StepEntity>,
    pub maths: Vec<MathEntity>,
    pub callouts: Vec<CalloutEntity>,
    pub package_versions: Vec<String>,
    pub licenses: Vec<String>,
    pub repository_urls: Vec<String>,

    // Contact & Commerce
    pub emails: Vec<String>,
    pub phones: Vec<String>,
    pub addresses: Vec<String>,
    pub urls: Vec<String>,
    pub prices: Vec<String>,
    pub dates: Vec<String>,
    pub ip_addresses: Vec<String>,
    pub social_handles: Vec<String>,
}

/// Extracted fenced or syntax code block from page content.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CodeBlockEntity {
    pub language: Option<String>,
    pub code: String,
    pub line_count: usize,
}

/// Extracted tabular data with header and row structure.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableEntity {
    pub caption: Option<String>,
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

/// Extracted heading element representing document hierarchy.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct HeadingEntity {
    pub level: u8,
    pub text: String,
    pub anchor: Option<String>,
}

/// Diagram: a mermaid/dot/graphviz code block reclassified, or an image
/// whose alt text or URL signals diagram/chart/graph content.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiagramEntity {
    pub kind: String,
    pub caption: Option<String>,
    pub source: String,
}

/// Usage example: a code block joined to its nearest preceding heading.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExampleEntity {
    pub context_heading: Option<String>,
    pub language: Option<String>,
    pub code: String,
}

/// FAQ pair: a question heading with its following paragraph.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FaqEntity {
    pub question: String,
    pub answer: String,
}

/// Procedural step from an ordered list under a how-to heading.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct StepEntity {
    pub position: u32,
    pub context_heading: Option<String>,
    pub title: String,
}

/// Math formula extracted from TeX delimiters.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MathEntity {
    pub tex: String,
    pub display: bool,
}

/// Docs callout (note/warning/tip) mapped from blockquotes and admonitions.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CalloutEntity {
    pub kind: String,
    pub text: String,
}

impl Entities {
    /// True when no entities were extracted.
    pub fn is_empty(&self) -> bool {
        self.code_blocks.is_empty()
            && self.tables.is_empty()
            && self.headings.is_empty()
            && self.diagrams.is_empty()
            && self.examples.is_empty()
            && self.faqs.is_empty()
            && self.steps.is_empty()
            && self.maths.is_empty()
            && self.callouts.is_empty()
            && self.package_versions.is_empty()
            && self.licenses.is_empty()
            && self.repository_urls.is_empty()
            && self.emails.is_empty()
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
