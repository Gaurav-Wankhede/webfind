// webfind-models: DistilledDocument and StructuralAssets for Stage 2 distillation.
// Preserves critical non-prose context (diagrams, tables, equations, code contracts, callouts).

use chrono::{DateTime, Utc};
pub use chrono::Utc as PayloadUtc;
use serde::{Deserialize, Serialize};

/// Visual or architectural diagram asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramAsset {
    /// Format identifier: "mermaid", "svg", "plantuml", "dot", etc.
    pub format: String,
    /// Raw diagram code or SVG markup.
    pub raw: String,
    /// Optional explanatory caption or title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Tabular data asset preserving Markdown or CSV representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableAsset {
    /// Header columns.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub headers: Vec<String>,
    /// Table rows where each inner Vec represents a cell sequence.
    pub rows: Vec<Vec<String>>,
    /// Pre-rendered Markdown representation for seamless LLM context injection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Optional table caption or title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<String>,
}

/// Mathematical formula asset preserving LaTeX notation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MathAsset {
    /// LaTeX expression string.
    pub latex: String,
    /// Whether this is a block display equation ($$) or inline ($).
    pub is_block: bool,
    /// Optional formula identifier or tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

/// Code contract or snippet asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAsset {
    /// Programming language identifier (e.g. "rust", "typescript", "python").
    pub language: String,
    /// Complete code block or API contract.
    pub code: String,
    /// Optional file path or signature anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

/// Security notice, warning, or callout container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalloutAsset {
    /// Severity or type: "danger", "warning", "note", "tip", "caution".
    pub severity: String,
    /// Callout body text.
    pub message: String,
}

/// Six Pillars Structural Assets container.
/// Isolates non-prose context so token compression does not lose architectural information.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StructuralAssets {
    /// Architectural diagrams (Mermaid, SVG).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub diagrams: Vec<DiagramAsset>,
    /// Structured comparison and data tables.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub tables: Vec<TableAsset>,
    /// Mathematical equations and algorithms (LaTeX).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub equations: Vec<MathAsset>,
    /// API contracts and code snippets.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub code_contracts: Vec<CodeAsset>,
    /// Security warnings, deprecation notices, and highlighted callouts.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub callouts: Vec<CalloutAsset>,
}

impl StructuralAssets {
    /// Returns true if no structural assets are present.
    #[inline]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.diagrams.is_empty()
            && self.tables.is_empty()
            && self.equations.is_empty()
            && self.code_contracts.is_empty()
            && self.callouts.is_empty()
    }

    /// Returns total count of all structural assets contained.
    #[inline]
    #[must_use]
    pub fn total_count(&self) -> usize {
        self.diagrams.len()
            + self.tables.len()
            + self.equations.len()
            + self.code_contracts.len()
            + self.callouts.len()
    }
}

/// Document source provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocumentProvenance {
    /// Canonical source URL.
    pub canonical_url: String,
    /// Document or page title.
    pub title: String,
    /// Ingestion timestamp (UTC).
    pub crawled_at: DateTime<Utc>,
    /// Content hash (SHA-256) for deduplication and provenance integrity.
    pub content_hash: String,
}

/// Compression statistics for token accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionMetrics {
    /// Estimated or exact token count of raw source document before distillation.
    pub raw_token_count: usize,
    /// Estimated or exact token count of distilled payload.
    pub distilled_token_count: usize,
    /// Compression ratio percentage (e.g. 98 for 98% reduction).
    pub reduction_percent: u8,
}

/// High-density distilled document output produced by Stage 2 Metadata Distiller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistilledDocument {
    /// Document provenance and canonical identity.
    pub provenance: DocumentProvenance,
    /// Terse executive takeaway (<150 tokens) distilling core conclusions.
    pub core_takeaways: String,
    /// Preserved structural assets (diagrams, tables, math, code, callouts).
    #[serde(skip_serializing_if = "StructuralAssets::is_empty", default)]
    pub structural_assets: StructuralAssets,
    /// High-saliency extracted text spans or section digests.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub key_insights: Vec<String>,
    /// Compression and token economy metrics.
    pub metrics: CompressionMetrics,
}
