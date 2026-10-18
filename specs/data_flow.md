# WebFind Complete Pipeline Data Flow Types

## Overview
This document specifies the exact Rust types for every stage of the WebFind pipeline, from QueryInput to ReportOutput.

---

## Stage 0: Input Types

```rust
// src/types/input.rs
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use crate::engine::source_tier::SourceTier;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryInput {
    pub query: String,
    pub options: QueryOptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryOptions {
    #[serde(default = "default_depth")]
    pub depth: SearchDepth,
    #[serde(default = "default_max_sources")]
    pub max_sources: usize,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
    #[serde(default = "default_true")]
    pub include_images: bool,
    #[serde(default = "default_languages")]
    pub languages: Vec<String>,
    #[serde(default)]
    pub source_tiers: Vec<SourceTier>,
    #[serde(default)]
    pub date_range: Option<DateRange>,
    #[serde(default = "default_output_format")]
    pub output_format: OutputFormat,
    #[serde(default = "default_true")]
    pub include_explanations: bool,
    #[serde(default = "default_max_explanation_length")]
    pub max_explanation_length: usize,
    #[serde(default = "default_citation_style")]
    pub citation_style: CitationStyle,
}

fn default_depth() -> SearchDepth { SearchDepth::Standard }
fn default_max_sources() -> usize { 20 }
fn default_max_results() -> usize { 50 }
fn default_true() -> bool { true }
fn default_languages() -> Vec<String> { vec!["en".to_string()] }
fn default_output_format() -> OutputFormat { OutputFormat::Markdown }
fn default_max_explanation_length() -> usize { 500 }
fn default_citation_style() -> CitationStyle { CitationStyle::Inline }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchDepth {
    Quick,
    Standard,
    Deep,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DateRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum OutputFormat {
    Markdown,
    Json,
    Html,
    Pdf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CitationStyle {
    Inline,
    Footnote,
    Bibliography,
}
```

---

## Stage 1: Query Routing Types

```rust
// src/types/routing.rs
use serde::{Deserialize, Serialize};
use crate::engine::source_tier::SourceTier;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRoute {
    pub intent: QueryIntent,
    pub domain: String,
    pub language: String,
    pub sources: Vec<String>,
    pub confidence: f32,
    pub strategy: SearchStrategy,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QueryIntent {
    Vulnerability,
    RustError,
    CodeExplanation,
    ApiDocs,
    Specification,
    FactLookup,
    Comparison,
    Tutorial,
    General,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchStrategy {
    Deterministic,
    Embedding,
    Language,
    Fallback,
}
```

---

## Stage 2: Retrieval Types

```rust
// src/types/retrieval.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use crate::engine::source_tier::SourceTier;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HybridSearchResult {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub snippet: String,
    
    // Individual scores
    pub bm25_score: f32,
    pub dense_score: f32,
    pub sparse_score: f32,
    
    // Fused scores
    pub rrf_score: f32,
    pub final_score: f32,
    
    // Rank positions
    pub bm25_rank: usize,
    pub dense_rank: usize,
    pub sparse_rank: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bm25SparseVector {
    pub token_weights: HashMap<u32, f32>,
}
```

---

## Stage 3: Extraction Types

```rust
// src/types/extraction.rs
use serde::{Deserialize, Serialize};
use crate::engine::source_tier::SourceTier;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ExampleType {
    Code,
    Math,
    Chemical,
    Table,
    Diagram,
    Text,
    Image,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedExample {
    pub example_type: ExampleType,
    pub content: String,
    pub language: Option<String>,
    pub latex: Option<String>,
    pub smiles: Option<String>,
    pub markdown: Option<String>,
    pub mermaid: Option<String>,
    pub image_url: Option<String>,
    pub alt_text: Option<String>,
    pub surrounding_context: Option<String>,
    pub source_url: String,
    pub source_tier: SourceTier,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchedDocument {
    pub url: String,
    pub title: String,
    pub content_markdown: String,
    pub content_text: String,
    pub domain: String,
    pub language: String,
    pub source_tier: SourceTier,
    pub fetched_at: chrono::DateTime<chrono::Utc>,
    pub examples: Vec<ExtractedExample>,
    pub images: Vec<ExtractedExample>, // Image type examples
}
```

---

## Stage 4: KAG Reasoning Types

```rust
// src/types/reasoning.rs
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningChain {
    pub hops: Vec<ReasoningHop>,
    pub final_entities: Vec<ResolvedEntity>,
    pub confidence: f32,
    pub logical_form: LogicalForm,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReasoningHop {
    pub from_entity: String,
    pub relation: String,
    pub to_entity: String,
    pub relation_type: RelationType,
    pub weight: f32,
    pub evidence_urls: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum RelationType {
    Mentions,
    RelatedTo,
    InstanceOf,
    SubclassOf,
    PartOf,
    Causes,
    DependsOn,
    Uses,
    Implements,
    Extends,
    References,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogicalForm {
    EntityLookup { name: String, entity_type: Option<String> },
    Relation { subject: String, predicate: String, object: String },
    And(Box<LogicalForm>, Box<LogicalForm>),
    Or(Box<LogicalForm>, Box<LogicalForm>),
    Not(Box<LogicalForm>),
    Filter(Box<LogicalForm>, String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedEntity {
    pub canonical_name: String,
    pub entity_type: String,
    pub description: String,
    pub confidence: f32,
    pub aliases: Vec<String>,
    pub source_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedEntity {
    pub name: String,
    pub entity_type: String,
    pub description: String,
    pub context: String,
    pub confidence: f32,
}
```

---

## Stage 5: Synthesis Types

```rust
// src/types/synthesis.rs
use serde::{Deserialize, Serialize};
use crate::engine::source_tier::SourceTier;
use crate::types::extraction::ExtractedExample;
use crate::types::extraction::ExampleType;
use crate::types::reasoning::{ReasoningChain, LogicalForm};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConceptExplanation {
    pub summary: String,
    pub steps: Vec<ExplanationStep>,
    pub key_concepts: Vec<SimpleConcept>,
    pub when_to_use: String,
    pub pitfalls: Vec<String>,
    pub analogy: Option<String>,
    pub confidence: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExplanationStep {
    pub number: usize,
    pub description: String,
    pub why: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimpleConcept {
    pub term: String,
    pub plain_english: String,
    pub example_usage: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleWithExplanation {
    pub example: ExtractedExample,
    pub explanation: Option<ConceptExplanation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub snippet: String,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceCluster {
    pub topic: String,
    pub representative_url: String,
    pub rank: f32,
    pub examples: Vec<ExampleWithExplanation>,
    pub citations: Vec<Citation>,
    pub related_entities: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Citation {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub snippet: String,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePack {
    pub query: String,
    pub route: QueryRoute,  // from routing.rs
    pub clusters: Vec<EvidenceCluster>,
    pub citation_graph: CitationGraph,
    pub reasoning_chain: Option<ReasoningChain>,
    pub metadata: EvidenceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceMetadata {
    pub sources_searched: usize,
    pub total_results: usize,
    pub search_time_ms: u64,
    pub fetch_time_ms: u64,
    pub kag_time_ms: u64,
    pub synthesis_time_ms: u64,
    pub models_used: Vec<String>,
    pub cache_hits: usize,
    pub cache_misses: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationGraph {
    pub nodes: Vec<CitationNode>,
    pub edges: Vec<CitationEdge>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationNode {
    pub id: String,
    pub url: String,
    pub title: String,
    pub source_tier: SourceTier,
    pub cluster_ids: Vec<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CitationEdge {
    pub from: String,
    pub to: String,
    pub relationship: CitationRelationship,
    pub weight: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum CitationRelationship {
    Supports,
    Contradicts,
    Extends,
    References,
    Duplicate,
}
```

---

## Stage 6: Output Types

```rust
// src/types/output.rs
use serde::{Deserialize, Serialize};
use crate::types::synthesis::{EvidencePack, EvidenceCluster, CitationGraph, Citation, EvidenceMetadata};
use crate::types::reasoning::ReasoningChain;
use crate::types::routing::QueryRoute;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportOutput {
    pub query: String,
    pub executive_summary: String,
    pub sections: Vec<ReportSection>,
    pub citations: CategorizedCitations,
    pub metadata: EvidenceMetadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSection {
    pub title: String,
    pub narrative: String,
    pub examples_with_explanations: Vec<ExampleWithExplanation>,
    pub citations: Vec<Citation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleWithExplanation {
    pub example: ExtractedExample,  // from extraction.rs
    pub explanation: Option<ConceptExplanation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CategorizedCitations {
    pub primary: Vec<Citation>,
    pub raw: Vec<Citation>,
    pub reference: Vec<Citation>,
    pub encyclopedia: Vec<Citation>,
    pub community: Vec<Citation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OutputFormat {
    Markdown(String),
    Json(serde_json::Value),
    Html(String),
    Pdf(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorOutput {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
    pub retry_after_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ErrorCode {
    QueryTooShort,
    QueryTooLong,
    InvalidOptions,
    NoResultsFound,
    FetchTimeout,
    FetchFailed,
    EmbeddingFailed,
    ModelUnavailable,
    DatabaseError,
    RateLimited,
    InternalError,
}
```

---

## Stage 7: Health & Monitoring Types

```rust
// src/types/health.rs
use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthOutput {
    pub status: HealthStatus,
    pub version: String,
    pub uptime_secs: u64,
    pub databases: DatabaseHealth,
    pub models: ModelHealth,
    pub cache: CacheStats,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabaseHealth {
    pub turso: ComponentHealth,
    pub qdrant: ComponentHealth,
    pub falkordb: ComponentHealth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentHealth {
    pub status: HealthStatus,
    pub latency_ms: u64,
    pub last_check: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHealth {
    pub embedding: ComponentHealth,
    pub explanation: ComponentHealth,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    pub query_cache_hits: u64,
    pub query_cache_misses: u64,
    pub embedding_cache_size: usize,
}
```

---

## Stage 8: Configuration Types

```rust
// src/types/config.rs
use serde::{Deserialize, Serialize};
use crate::types::retrieval::HybridRetrievalConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    pub hybrid_config: HybridRetrievalConfig,
    pub max_concurrent_fetches: usize,
    pub fetch_timeout_secs: u64,
    pub enable_kag: bool,
    pub enable_explanations: bool,
    pub max_examples_per_type: usize,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            hybrid_config: HybridRetrievalConfig::default(),
            max_concurrent_fetches: 10,
            fetch_timeout_secs: 30,
            enable_kag: true,
            enable_explanations: true,
            max_examples_per_type: 5,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchConfig {
    pub timeout_secs: u64,
    pub max_concurrent: usize,
    pub extract_examples: bool,
}
```

---

## Type Aliases & Re-exports

```rust
// src/types/mod.rs
pub mod input;
pub mod routing;
pub mod retrieval;
pub mod extraction;
pub mod reasoning;
pub mod synthesis;
pub mod output;
pub mod health;
pub mod config;

pub use input::{QueryInput, QueryOptions, SearchDepth, DateRange, OutputFormat, CitationStyle};
pub use routing::{QueryRoute, QueryIntent, SearchStrategy};
pub use retrieval::{HybridSearchResult, Bm25SparseVector, HybridRetrievalConfig};
pub use extraction::{ExtractedExample, ExampleType, FetchedDocument};
pub use reasoning::{ReasoningChain, ReasoningHop, RelationType, LogicalForm, ResolvedEntity, ExtractedEntity};
pub use synthesis::{ConceptExplanation, ExplanationStep, SimpleConcept, ExampleWithExplanation, Citation, EvidenceCluster, Citation, EvidencePack, CitationGraph, CitationNode, CitationEdge, CitationRelationship, EvidenceMetadata};
pub use output::{ReportOutput, ReportSection, ExampleWithExplanation, CategorizedCitations, OutputFormat, ErrorOutput, ErrorCode};
pub use health::{HealthOutput, HealthStatus, DatabaseHealth, ComponentHealth, ModelHealth, CacheStats};
pub use config::{PipelineConfig, FetchConfig, HybridRetrievalConfig};
```

---

## JSON Schema Examples

### QueryInput Example
```json
{
  "query": "How does Rust async/await work?",
  "options": {
    "depth": "standard",
    "max_sources": 20,
    "max_results": 50,
    "include_images": true,
    "languages": ["en"],
    "source_tiers": ["primary_official", "raw_source", "reference"],
    "output_format": "markdown",
    "include_explanations": true,
    "max_explanation_length": 300,
    "citation_style": "inline"
  }
}
```

### ReportOutput Example (JSON)
```json
{
  "query": "How does Rust async/await work?",
  "executive_summary": "Rust's async system is built around the Future trait...",
  "sections": [
    {
      "title": "The Future Trait",
      "narrative": "The Future trait is the foundation of all async code in Rust...",
      "examples_with_explanations": [
        {
          "example": {
            "type": "code",
            "content": "trait Future { type Output; fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output>; }",
            "language": "rust",
            "source_url": "https://doc.rust-lang.org/std/future/trait.Future.html"
          },
          "explanation": {
            "summary": "Future is a trait representing an asynchronous computation...",
            "steps": [
              { "number": 1, "description": "Define the output type", "why": "What the future ultimately returns" },
              { "number": 2, "description": "Implement poll()", "why": "The executor calls this to make progress" }
            ],
            "key_concepts": [
              { "term": "Pin", "plain_english": "Prevents moving the future in memory", "example_usage": "Pin<&mut Self>" }
            ],
            "when_to_use": "Use async when doing I/O or waiting",
            "pitfalls": ["Never block inside poll()", "Don't forget to wake()"],
            "analogy": "Like ordering food at a restaurant - you don't wait in kitchen",
            "confidence": 0.95
          }
        }
      ],
      "citations": [
        { "url": "https://doc.rust-lang.org/std/future/", "title": "std::future", "tier": "primary_official", "snippet": "The Future trait", "relevance_score": 0.98 }
      ]
    }
  ],
  "citations": {
    "primary": [...],
    "raw": [...],
    "reference": [...],
    "encyclopedia": [...],
    "community": [...]
  },
  "metadata": {
    "sources_searched": 12,
    "total_results": 45,
    "search_time_ms": 245,
    "fetch_time_ms": 1200,
    "kag_time_ms": 450,
    "synthesis_time_ms": 180,
    "models_used": ["embeddinggemma2", "qwen2.5-0.5b"],
    "cache_hits": 3,
    "cache_misses": 9
  }
}
```

---

*End of Pipeline Data Flow Types Specification*
