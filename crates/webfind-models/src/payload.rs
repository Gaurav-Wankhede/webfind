// webfind-models: DistilledDocument and StructuralAssets for Stage 2 distillation.
// Preserves critical non-prose context (diagrams, tables, equations, code contracts, callouts).
// Optimized per RUST_SYSTEMS_OPTIMIZATION_HANDBOOK:
// 1. CompactStr (Small String Optimization, 0 heap mallocs for <=24 bytes)
// 2. SmallVec<[T; N]> (Stack-inlined collections for <=2-4 items)
// 3. Cache-line packed field ordering (8 -> 4 -> 2 -> 1)
// 4. DistillationTrainingPair providing symmetric raw-to-distilled supervision

use chrono::{DateTime, Utc};
pub use chrono::Utc as PayloadUtc;
pub use compact_str::CompactString as CompactStr;
use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

/// Visual or architectural diagram asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagramAsset {
    /// Format identifier: "mermaid", "svg", "plantuml", "dot", etc. (stack SSO <= 24 bytes).
    pub format: CompactStr,
    /// Raw diagram code or SVG markup.
    pub raw: String,
    /// Optional explanatory caption or title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<CompactStr>,
}

/// Tabular data asset preserving Markdown or CSV representations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableAsset {
    /// Header columns (stack-inlined if <= 8 columns).
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub headers: SmallVec<[CompactStr; 8]>,
    /// Table rows where each inner row is stack-inlined.
    pub rows: Vec<SmallVec<[CompactStr; 8]>>,
    /// Pre-rendered Markdown representation for seamless LLM context injection.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub markdown: Option<String>,
    /// Optional table caption or title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caption: Option<CompactStr>,
}

/// Mathematical formula asset preserving LaTeX notation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MathAsset {
    /// LaTeX expression string.
    pub latex: String,
    /// Optional formula identifier or tag.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<CompactStr>,
    /// Whether this is a block display equation ($$) or inline ($).
    pub is_block: bool,
}

/// Code contract or snippet asset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeAsset {
    /// Programming language identifier (e.g. "rust", "typescript", "python") (stack SSO).
    pub language: CompactStr,
    /// Complete code block or API contract.
    pub code: String,
    /// Optional file path or signature anchor.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<CompactStr>,
}

/// Security notice, warning, or callout container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalloutAsset {
    /// Severity: "danger", "warning", "note", "tip", "caution" (stack SSO).
    pub severity: CompactStr,
    /// Callout body text.
    pub message: String,
}

/// Six Pillars Structural Assets container.
/// Stack-inlined with SmallVec for typical documentation densities (<=2 diagrams, <=4 tables, <=4 callouts).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StructuralAssets {
    /// Architectural diagrams (Mermaid, SVG).
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub diagrams: SmallVec<[DiagramAsset; 2]>,
    /// Structured comparison and data tables.
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub tables: SmallVec<[TableAsset; 4]>,
    /// Mathematical equations and algorithms (LaTeX).
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub equations: SmallVec<[MathAsset; 4]>,
    /// API contracts and code snippets.
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub code_contracts: SmallVec<[CodeAsset; 4]>,
    /// Security warnings, deprecation notices, and highlighted callouts.
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub callouts: SmallVec<[CalloutAsset; 4]>,
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
    /// Content hash (SHA-256) for deduplication and provenance integrity (64 chars).
    pub content_hash: CompactStr,
    /// Ingestion timestamp (UTC).
    pub crawled_at: DateTime<Utc>,
}

/// Compression statistics for token accounting.
/// Cache-line packed: 8 bytes -> 8 bytes -> 1 byte (17 bytes total).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressionMetrics {
    /// Estimated token count of raw source document before distillation.
    pub raw_token_count: usize,
    /// Estimated token count of distilled payload.
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
    /// High-saliency extracted text spans or section digests (stack-inlined <= 6).
    #[serde(skip_serializing_if = "SmallVec::is_empty", default)]
    pub key_insights: SmallVec<[CompactStr; 6]>,
    /// Compression and token economy metrics.
    pub metrics: CompressionMetrics,
}

/// Complete symmetric training pair for Stage 2 Neural Distillation.
/// Maps uncompressed input document to ground-truth distilled structural payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DistillationTrainingPair {
    /// Raw uncompressed source text / Markdown / HTML.
    pub raw_content: String,
    /// Source origin domain (e.g. "docs.rs", "sqlite.org").
    pub domain: CompactStr,
    /// Raw token length of uncompressed document.
    pub raw_tokens: usize,
    /// Target ground-truth distilled document with preserved 6 structural pillars.
    pub target_distilled: DistilledDocument,
}
