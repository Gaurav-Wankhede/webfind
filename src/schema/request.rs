use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Main search request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    pub depth: SearchDepth,
    pub limit: u32,
    pub output: OutputFormat,
    pub language: Option<String>,
    pub date_range: Option<DateRange>,
    pub domains: Option<DomainFilter>,
    pub content_type: Option<ContentType>,
    pub include_content: bool,
    pub include_graph: bool,
    pub include_keywords: bool,
    pub include_metrics: bool,
    /// Enable BM25 + vector hybrid re-ranking.
    #[serde(default)]
    pub hybrid: bool,
}

/// Search depth level — determines which ranking signals are used
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchDepth {
    /// BM25 only (~1ms)
    Shallow,
    /// BM25 + vector similarity (~10ms)
    #[default]
    Standard,
    /// BM25 + vector + graph + freshness + quality (~50ms)
    Deep,
    /// All signals + full metadata (~200ms)
    Comprehensive,
}

/// Date range filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
}

/// Domain filter
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DomainFilter {
    pub include: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

/// Content type classification
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Article,
    Tutorial,
    Documentation,
    News,
    Forum,
    #[default]
    Any,
}

/// Output format
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    Json,
    #[default]
    Report,
    Markdown,
}
