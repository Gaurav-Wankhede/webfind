# WebFind Unified Input/Output Contract

## Overview
This document defines the complete input/output contract for the WebFind pipeline, from user query to final report output.

## Version
v1.0.0

---

## 1. QueryInput

### 1.1 Core Query Input
```rust
pub struct QueryInput {
    pub query: String,
    pub options: QueryOptions,
}

pub struct QueryOptions {
    pub depth: SearchDepth,           // "quick" | "standard" | "deep"
    pub max_sources: usize,           // Default: 20
    pub max_results: usize,           // Default: 50
    pub include_images: bool,         // Default: true
    pub languages: Vec<String>,       // Default: ["en"]
    pub source_tiers: Vec<SourceTier>, // Default: all
    pub date_range: Option<DateRange>, // Optional
    pub output_format: OutputFormat,  // "markdown" | "json" | "html" | "pdf"
    pub include_explanations: bool,   // Default: true
    pub max_explanation_length: usize, // Default: 500 words
    pub citation_style: CitationStyle, // "inline" | "footnote" | "bibliography"
}

pub enum SearchDepth {
    Quick,      // BM25 only, top 10, no KAG
    Standard,   // Hybrid + KAG (2 hops), top 20
    Deep,       // Full hybrid + KAG (3 hops) + clustering, top 50
}

pub struct DateRange {
    pub from: DateTime<Utc>,
    pub to: DateTime<Utc>,
}

pub enum OutputFormat {
    Markdown,
    Json,
    Html,
    Pdf,
}

pub enum CitationStyle {
    Inline,
    Footnote,
    Bibliography,
}
```

### 1.2 QueryInput JSON Schema
```json
{
  "query": "string (required, 1-500 chars)",
  "options": {
    "depth": "enum: quick|standard|deep",
    "max_sources": "integer (1-100)",
    "max_results": "integer (1-200)",
    "include_images": "boolean",
    "languages": ["string (ISO 639-1)"],
    "source_tiers": ["enum: raw_source|primary_official|reference|encyclopedia|community|unknown"],
    "date_range": {
      "from": "ISO8601 datetime",
      "to": "ISO8601 datetime"
    },
    "output_format": "enum: markdown|json|html|pdf",
    "include_explanations": "boolean",
    "max_explanation_length": "integer (50-2000)",
    "citation_style": "enum: inline|footnote|bibliography"
  }
}
```

---

## 2. QueryRoute (Internal)

```rust
pub struct QueryRoute {
    pub intent: QueryIntent,
    pub domain: String,
    pub language: String,
    pub sources: Vec<String>,
    pub confidence: f32,
    pub strategy: SearchStrategy,
}

pub enum QueryIntent {
    Vulnerability,      // CVE, GHSA, OSV
    RustError,          // E0xxx
    CodeExplanation,    // how-to, implement, async fn
    ApiDocs,            // API reference, method
    Specification,      // RFC, spec, standard
    FactLookup,         // what is, define
    Comparison,         // vs, versus
    Tutorial,           // how to, guide
    General,
}

pub enum SearchStrategy {
    Deterministic,
    Embedding,
    Language,
    Fallback,
}
```

---

## 3. HybridSearchResult

```rust
pub struct HybridSearchResult {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub snippet: String,
    
    // Individual scores
    pub bm25_score: f32,        // 0.0-1.0 (higher = better)
    pub dense_score: f32,       // cosine similarity 0.0-1.0
    pub sparse_score: f32,      // BM25 sparse 0.0-1.0
    
    // Fused scores
    pub rrf_score: f32,         // RRF fused score
    pub final_score: f32,       // RRF * SourceTier boost
    
    // Rank positions
    pub bm25_rank: usize,
    pub dense_rank: usize,
    pub sparse_rank: usize,
}
```

---

## 4. ExtractedExample

```rust
pub struct ExtractedExample {
    pub example_type: ExampleType,
    pub content: String,
    pub language: Option<String>,       // for code: "rust", "python", etc.
    pub latex: Option<String>,          // for math formulas
    pub smiles: Option<String>,         // for chemical structures
    pub markdown: Option<String>,       // for tables
    pub mermaid: Option<String>,        // for diagrams
    pub image_url: Option<String>,      // for images
    pub alt_text: Option<String>,       // for images
    pub surrounding_context: Option<String>, // 500 chars before/after
    pub source_url: String,
    pub source_tier: SourceTier,
}

pub enum ExampleType {
    Code,
    Math,
    Chemical,
    Table,
    Diagram,
    Text,
    Image,
}
```

---

## 5. ConceptExplanation (Qwen2.5 Output)

```rust
pub struct ConceptExplanation {
    pub summary: String,                    // One-sentence plain English
    pub steps: Vec<ExplanationStep>,        // Numbered steps with "why"
    pub key_concepts: Vec<SimpleConcept>,   // Terms with simple definitions
    pub when_to_use: String,                // When to apply this technique
    pub pitfalls: Vec<String>,              // Common mistakes
    pub analogy: Option<String>,            // Real-world analogy
    pub confidence: f32,                    // 0.0-1.0
}

pub struct ExplanationStep {
    pub number: usize,
    pub description: String,
    pub why: String,                        // Why this step matters
}

pub struct SimpleConcept {
    pub term: String,
    pub plain_english: String,
    pub example_usage: Option<String>,      // Tiny usage snippet
}
```

---

## 5. EvidenceCluster

```rust
pub struct EvidenceCluster {
    pub topic: String,
    pub representative_url: String,
    pub rank: f32,                          // PageRank score
    pub examples: Vec<ExampleWithExplanation>,
    pub citations: Vec<Citation>,
    pub related_entities: Vec<String>,
}

pub struct ExampleWithExplanation {
    pub example: ExtractedExample,
    pub explanation: Option<ConceptExplanation>, // None if match-based skipped
}

pub struct Citation {
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub snippet: String,
    pub relevance_score: f32,
}
```

---

## 6. CitationGraph

```rust
pub struct CitationGraph {
    pub nodes: Vec<CitationNode>,
    pub edges: Vec<CitationEdge>,
}

pub struct CitationNode {
    pub id: String,                         // URL
    pub url: String,
    pub title: String,
    pub source_tier: SourceTier,
    pub cluster_ids: Vec<usize>,            // Which clusters cite this
}

pub struct CitationEdge {
    pub from: String,                       // Source URL
    pub to: String,                         // Target URL
    pub relationship: CitationRelationship,
    pub weight: f32,
}

pub enum CitationRelationship {
    Supports,       // A supports B
    Contradicts,    // A contradicts B
    Extends,        // A extends B
    References,     // A references B
    Duplicate,      // A duplicates B
}
```

---

## 7. EvidencePack

```rust
pub struct EvidencePack {
    pub query: String,
    pub route: QueryRoute,
    pub clusters: Vec<EvidenceCluster>,
    pub citation_graph: CitationGraph,
    pub reasoning_chain: Option<ReasoningChain>,
    pub metadata: EvidenceMetadata,
}

pub struct EvidenceMetadata {
    pub sources_searched: usize,
    pub total_results: usize,
    pub search_time_ms: u64,
    pub fetch_time_ms: u64,
    pub kag_time_ms: u64,
    pub synthesis_time_ms: u64,
    pub models_used: Vec<String>,           // ["embeddinggemma2", "qwen2.5-0.5b"]
    pub cache_hits: usize,
    pub cache_misses: usize,
}
```

---

## 7. ReasoningChain

```rust
pub struct ReasoningChain {
    pub hops: Vec<ReasoningHop>,
    pub final_entities: Vec<ResolvedEntity>,
    pub confidence: f32,
    pub logical_form: LogicalForm,
}

pub struct ReasoningHop {
    pub from_entity: String,
    pub relation: String,
    pub to_entity: String,
    pub relation_type: RelationType,
    pub weight: f32,
    pub evidence_urls: Vec<String>,
}

pub enum RelationType {
    Mentions, RelatedTo, InstanceOf, SubclassOf,
    PartOf, Causes, DependsOn, Uses, Implements, Extends, References,
}

pub enum LogicalForm {
    EntityLookup { name: String, entity_type: Option<String> },
    Relation { subject: String, predicate: String, object: String },
    And(Box<LogicalForm>, Box<LogicalForm>),
    Or(Box<LogicalForm>, Box<LogicalForm>),
    Not(Box<LogicalForm>),
    Filter(Box<LogicalForm>, String),
}

pub struct ResolvedEntity {
    pub canonical_name: String,
    pub entity_type: String,
    pub description: String,
    pub confidence: f32,
    pub aliases: Vec<String>,
    pub source_urls: Vec<String>,
}
```

---

## 8. ReportOutput (Final)

### 8.1 Markdown Format
```markdown
# Research Report: "{query}"

## Executive Summary
{3-5 paragraph narrative summary generated by Qwen2.5}

---

## Section 1: {topic}
### Narrative
{section narrative from Qwen2.5}

### Example 1: {type}
```{language}
{code/content}
```

### 💡 Explanation
**Summary:** {one-sentence}
**Steps:**
1. {step} - {why}
2. {step} - {why}
**Key Concepts:**
- **{term}**: {plain_english}
**When to Use:** {when}
**Pitfalls:** {pitfalls}
**Analogy:** {analogy}

### Citations
1. 🟢 **Primary** - [{title}]({url}) - {snippet}

---

## Section 2: {topic}
...

---

## Citations
### Primary Sources (🟢)
1. [{title}]({url}) - {domain} - {snippet}

### Raw Sources (🔵)
...

### Reference Sources (🟡)
...

### Encyclopedia (🟠)
...

### Community (🔴)
...

---

*Generated by WebFind | Models: EmbeddingGemma2 (embed), Qwen2.5-0.5B (explain) | Sources: {N} | Time: {X.Y}s*

### 8.2 JSON Format
```json
{
  "query": "string",
  "executive_summary": "string",
  "sections": [
    {
      "title": "string",
      "narrative": "string",
      "examples_with_explanations": [
        {
          "example": { ... },
          "explanation": { ... }
        }
      ],
      "citations": [
        { "url": "...", "title": "...", "tier": "...", "snippet": "..." }
      ]
    }
  ],
  "citations": { "primary": [...], "raw": [...], "reference": [...], "encyclopedia": [...], "community": [...] },
  "metadata": { ... }
}
```

---

## 9. ErrorOutput

```rust
pub struct ErrorOutput {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<serde_json::Value>,
    pub retry_after_secs: Option<u64>,
}

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

## 10. HealthCheckOutput

```rust
pub struct HealthOutput {
    pub status: HealthStatus,
    pub version: String,
    pub uptime_secs: u64,
    pub databases: DatabaseHealth,
    pub models: ModelHealth,
    pub cache: CacheStats,
}

pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

pub struct DatabaseHealth {
    pub turso: ComponentHealth,
    pub qdrant: ComponentHealth,
    pub falkordb: ComponentHealth,
}

pub struct ComponentHealth {
    pub status: HealthStatus,
    pub latency_ms: u64,
    pub last_check: DateTime<Utc>,
}

pub struct ModelHealth {
    pub embedding: ComponentHealth,
    pub explanation: ComponentHealth,
}

pub struct CacheStats {
    pub query_cache_hits: u64,
    pub query_cache_misses: u64,
    pub embedding_cache_size: usize,
}
```

---

## 11. Versioning & Compatibility

### API Version Header
```
X-WebFind-Version: 1.0.0
```

### Backward Compatibility
- All fields are optional with sensible defaults
- New fields added as optional
- Enum variants only added, never removed
- Breaking changes require major version bump

---

*End of I/O Contract Specification*
