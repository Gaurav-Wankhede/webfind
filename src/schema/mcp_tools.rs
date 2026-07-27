use serde::{Deserialize, Serialize};

/// MCP Tool: web_search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchTool {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub depth: Option<String>,
    pub language: Option<String>,
    pub domains: Option<Vec<String>>,
    pub date_after: Option<String>,
    pub date_before: Option<String>,
}

/// MCP Tool: web_fetch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebFetchTool {
    pub url: String,
    #[serde(default = "default_true")]
    pub extract_content: bool,
    #[serde(default)]
    pub extract_keywords: bool,
    #[serde(default)]
    pub extract_links: bool,
}

/// MCP Tool: hybrid_search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchTool {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: u32,
    pub bm25_weight: Option<f64>,
    pub vector_weight: Option<f64>,
    pub graph_weight: Option<f64>,
}

/// MCP Tool: graph_traverse
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphTraverseTool {
    pub url: String,
    #[serde(default = "default_depth")]
    pub depth: u32,
    pub direction: Option<String>,
}

/// MCP Tool: crawl_status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlStatusTool {
    pub include_stats: bool,
}

fn default_limit() -> u32 {
    10
}

fn default_true() -> bool {
    true
}

fn default_depth() -> u32 {
    1
}
