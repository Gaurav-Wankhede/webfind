# WebFind Enhancement Catalog

> **Purpose:** Comprehensive catalog of proven, production-ready alternatives that enhance every layer of the WebFind stack — from crawling to ranking to query understanding.
> **Date:** 2026-08-08
> **Scope:** Each enhancement is evaluated against the current implementation with migration effort, impact, and risk assessment.

---

## Executive Summary

| Layer | Current | Recommended Enhancement | Impact | Effort |
|-------|---------|------------------------|--------|--------|
| **Embedding** | all-MiniLM-L6-v2 (384d) | bge-small-en-v1.5 (384d) or nomic-embed-text-v2 (768d) | +5-10% retrieval@10 | Low |
| **Content Extraction** | readability-rust + html2text | rs-trafilatura or readex | +15-20% F1 score | Medium |
| **Query Spelling** | None | symspell_rs | Handles 10-15% misspelled queries | Low |
| **Vector Search** | Brute-force DashMap | diskann-rs (disk-backed) or Turso native DiskANN | 6-10× lower RAM, 3-30× faster | Medium |
| **Reranking** | None (RRF only) | lunaris-rerank (bge-reranker-v2-m3) | +5-15% MRR | Medium |
| **Deduplication** | SimHash only | txtfp (MinHash + LSH + SimHash) | Better near-dup detection | Low |
| **Query Intent** | None | rust_memex::query or rule-based classifier | Route queries to optimal strategy | Low |
| **FTS Engine** | Tantivy (external) | Turso native FTS or SeekStorm | Unified storage + search | High |

---

## 1. Embedding Model Upgrade

### Current State

WebFind uses `fastembed` with `all-MiniLM-L6-v2` (22.7M params, 384-dim, 256-token context). This is the default for many RAG systems but is no longer state-of-the-art.

### Options

| Model | Params | Dim | Context | CPU Speed | Retrieval@10 | License | Best For |
|-------|--------|-----|---------|-----------|-------------|---------|----------|
| **all-MiniLM-L6-v2** (current) | 22.7M | 384 | 256 | 2,849 eps | ~82% | Apache-2.0 | Throughput-first |
| **bge-small-en-v1.5** | 33M | 384 | 512 | ~2,500 eps | ~88% | MIT | Best quality/size ratio |
| **nomic-embed-text-v2** | 475M (305M active) | 768 | 8192 | 580 chunks/s | 88% | Apache-2.0 | CPU-only, multilingual |
| **jina-embeddings-v3** | 570M | 1024 (→256 Matryoshka) | 8192 | 220 chunks/s | **92%** | CC-BY-NC 4.0 | Best overall accuracy |
| **bge-m3** | 568M | 1024 | 8192 | ~200 chunks/s | ~89% | MIT | Dense + sparse + ColBERT |
| **Qwen3-Embedding-0.6B** | 0.6B | 1024 (→32 MRL) | 32768 | ~150 chunks/s | **93%** | Apache-2.0 | Best quality-per-VRAM |

### Recommendation

**Tier 1 (Drop-in replacement):** `bge-small-en-v1.5`
- Same 384 dimensions → no index rebuild needed
- 33M params (vs 22.7M) → negligible speed difference on CPU
- +5-6% retrieval@10 over MiniLM
- MIT license (commercial-safe)
- Already supported by `fastembed` crate: `EmbeddingModel::BGESmallENV15`

**Tier 2 (Best quality):** `nomic-embed-text-v2`
- 768 dimensions → requires index rebuild
- 580 chunks/s on CPU (5× faster than 1024-dim alternatives)
- 88% retrieval@10, multilingual (100+ languages)
- Apache-2.0 license
- Mixture-of-experts: activates only 305M of 475M params per token

**Tier 3 (Maximum accuracy):** `jina-embeddings-v3`
- 1024 dimensions with Matryoshka truncation to 256/512/768
- 92% retrieval@10 (best in class)
- 89-language support
- **Warning:** CC-BY-NC 4.0 license — not commercial-safe without paid license

### Migration Path

```rust
// Current (fastembed)
let model = TextEmbedding::try_new(
    TextInitOptions::new(EmbeddingModel::AllMiniLML6V2)
)?;

// Tier 1: Drop-in replacement
let model = TextEmbedding::try_new(
    TextInitOptions::new(EmbeddingModel::BGESmallENV15)
)?;

// Tier 2: Best CPU performance
let model = TextEmbedding::try_new(
    TextInitOptions::new(EmbeddingModel::NomicEmbedTextV2)
)?;
```

**Source:** [PromptQuorum benchmark](https://www.promptquorum.com/power-local-llm/best-embedding-models-local-rag-2026), [rust-embedding-bench](https://github.com/jerrythomas/rust-embedding-bench), [RunLocalAI embeddings](https://www.runlocalai.co/embeddings)

---

## 2. Content Extraction Upgrade

### Current State

WebFind uses `readability-rust` + `html2text` for content extraction. This is a basic approach that handles simple articles well but struggles with forums, product pages, listings, and documentation.

### Options

| Crate | Algorithms | F1 Score | Speed | Page Types | Metadata | License |
|-------|-----------|----------|-------|------------|----------|---------|
| **readability-rust** (current) | Mozilla Readability | ~0.80 | Fast | Articles only | Basic | MIT |
| **rs-trafilatura** | Trafilatura (ML) | **0.966** | 46 files/s | 7 types | Rich | MIT |
| **readex** | Readability + Trafilatura + htmldate | **0.932** | Fast | All types | ~15 fields | Apache-2.0 |

### Recommendation

**Primary: `rs-trafilatura`**
- F1 = 0.966 on ScrapingHub benchmark (vs ~0.80 for readability-rust)
- ML page-type classification (article, forum, product, collection, listing, documentation, service)
- Per-type extraction profiles (12 forum platforms, 4 documentation frameworks)
- Extraction quality predictor (0.0-1.0 confidence) — pages below 0.80 can trigger LLM fallback
- Native `spider-rs` integration (WebFind already uses `spider` crate)
- GitHub: [Murrough-Foley/rs-trafilatura](https://github.com/Murrough-Foley/rs-trafilatura)

**Alternative: `readex`**
- Combines Mozilla Readability + Trafilatura cascade + htmldate
- ~15 metadata fields (title, byline, language, dates, categories, tags, image, license, hostname)
- Differential parity testing against Python Trafilatura on every release
- Apache-2.0 license
- GitHub: [0x4D44/readex](https://github.com/0x4D44/readex)

### Migration Path

```toml
# Cargo.toml
[dependencies]
rs-trafilatura = { version = "0.2", features = ["spider"] }
```

```rust
// Replace readability-rust + html2text with:
use rs_trafilatura::spider_integration::extract_page;

// In the crawler pipeline:
if let Ok(result) = extract_page(page) {
    content.title = result.metadata.title.unwrap_or_default();
    content.content_text = result.content_text;
    content.content_markdown = result.content_markdown;
    content.author = result.metadata.author;
    content.published_at = result.metadata.date;
    // Use extraction_quality for confidence threshold
    if result.extraction_quality < 0.80 {
        // Trigger LLM fallback or flag for review
    }
}
```

**Source:** [rs-trafilatura GitHub](https://github.com/Murrough-Foley/rs-trafilatura), [readex docs.rs](https://docs.rs/readex/latest/readex/), [contextractor](https://github.com/contextractor/contextractor)

---

## 3. Query Spelling Correction

### Current State

WebFind has no spelling correction. 10-15% of web search queries contain misspelled terms, leading to zero results or irrelevant matches.

### Recommendation: `symspell_rs`

- **1 million times faster** than Norvig's algorithm
- Symmetric Delete algorithm: only deletes needed (no transposes/replaces/inserts)
- Compound-aware multi-word correction
- Word segmentation for noisy text
- Language independent
- 0.033ms per word (edit distance 2) on single core
- MIT license
- GitHub: [wolfgarbe/symspell_rs](https://github.com/wolfgarbe/symspell_rs)

### Integration

```toml
[dependencies]
symspell_rs = "6.8"
```

```rust
use symspell_rs::{SymSpell, Verbosity};

struct QueryCorrector {
    symspell: SymSpell,
}

impl QueryCorrector {
    fn new() -> Self {
        let mut symspell = SymSpell::new(2, 7, 1);
        // Load from WebFind's indexed terms (build dictionary from Tantivy/Turso terms)
        symspell.load_dictionary("terms.txt", 0, 1, " ");
        Self { symspell }
    }

    fn correct(&self, query: &str) -> String {
        let suggestions = self.symspell.lookup_compound(query, 2);
        suggestions.first().map(|s| s.term.clone()).unwrap_or_else(|| query.to_string())
    }

    // For autocomplete: return top-N suggestions per prefix
    fn suggest(&self, prefix: &str, count: usize) -> Vec<String> {
        self.symspell.lookup(prefix, Verbosity::Top, 2)
            .into_iter()
            .take(count)
            .map(|s| s.term)
            .collect()
    }
}
```

**Source:** [symspell_rs docs.rs](https://docs.rs/symspell_rs/latest/), [SeekStorm blog](https://seekstorm.com/blog/sub-millisecond-compound-aware-automatic-spelling-correction/)

---

## 4. Vector Search Engine Upgrade

### Current State

WebFind uses a brute-force `DashMap<String, Vec<f32>>` with cosine similarity scan. This is O(n) per query and requires all vectors in RAM. The `hnsw_rs` dependency exists but is unused.

### Options

| Approach | Latency | Memory | Scale | Build | License | Notes |
|----------|---------|--------|-------|-------|---------|-------|
| **DashMap brute-force** (current) | O(n) scan | All in RAM | <100K | None | - | Simple, correct |
| **hnsw_rs** (already dep) | Sub-ms | All in RAM | <1M | Medium | MIT | In-memory HNSW |
| **diskann-rs** | 55µs | **6-10× lower** | 10M-1B+ | 1.6× slower | MIT | Disk-backed, incremental |
| **rust-diskann** | ~100µs | Low | 1M+ | Medium | MIT | Pure Rust DiskANN |
| **Turso native DiskANN** | <15ms | In-file | 1M+ | Automatic | - | Via libsql_vector_idx |

### Recommendation

**For <100K vectors:** Keep brute-force or use `hnsw_rs` (already a dependency)
- Brute-force is correct and fast enough at this scale
- `hnsw_rs` gives sub-ms latency with minimal code change

**For 100K-10M vectors:** `diskann-rs`
- 6-10× lower memory than HNSW (disk-backed with mmap)
- Incremental updates (add/delete without rebuild)
- SIMD acceleration (AVX2, SSE4.1, NEON)
- Product Quantization (64× compression)
- Filtered search with metadata predicates
- GitHub: [diskann-rs](https://crates.io/crates/diskann-rs)

**For 10M+ vectors:** Turso native DiskANN
- Automatic index maintenance
- No separate vector store process
- Query via `vector_top_k()` SQL function

### Migration Path (diskann-rs)

```toml
[dependencies]
diskann-rs = "0.1"
anndists = "0.1"
```

```rust
use diskann_rs::{DiskAnnIndex, IndexBuilder};
use anndists::dist::DistL2;

struct VectorIndex {
    index: DiskAnnIndex<f32, DistL2>,
}

impl VectorIndex {
    fn new(dimension: usize, index_path: &Path) -> Self {
        let index = IndexBuilder::new(dimension, DistL2)
            .with_max_degree(64)
            .with_search_list_size(128)
            .build(index_path)
            .expect("build DiskANN index");
        Self { index }
    }

    fn insert(&mut self, id: u32, vector: &[f32]) {
        self.index.add_vector(id, vector).expect("add vector");
    }

    fn search(&self, query: &[f32], k: usize) -> Vec<(u32, f32)> {
        self.index.search(query, k).expect("search")
    }
}
```

**Source:** [diskann-rs crates.io](https://crates.io/crates/diskann-rs), [DiskANN vs HNSW benchmark](https://github.com/JohnDouglasJDX/diskann-vs-hnsw-benchmark), [ann-search-rs benchmarks](https://github.com/GregorLueg/ann-search-rs)

---

## 5. Cross-Encoder Reranking

### Current State

WebFind uses Reciprocal Rank Fusion (RRF) to combine BM25 and vector scores. No learned reranking stage exists.

### Options

| Crate | Model | Latency | Size | License | Status |
|-------|-------|---------|------|---------|--------|
| **lunaris-rerank** | bge-reranker-v2-m3 | 12ms p50 (CPU) | 1.1GB | MIT | Production |
| **lunaris-rerank-native** | bge-reranker-v2-m3 (candle) | 40-80ms (CPU) | 2.3GB | MIT | Production |
| **rerank-rs** | ms-marco-MiniLM-L6-v2 | ~12ms | 90MB | MIT | Production |
| **axil-rerank** | answerai-colbert-small-v1 | ~80ms | 33M params | MIT | Opt-in only |

### Recommendation

**Primary: `lunaris-rerank` with `fastembed` backend**
- BGE-Reranker-v2-m3 (568M params, XLM-RoBERTa architecture)
- 12ms p50 latency on CPU (batch of 30 candidates)
- Multilingual (89 languages)
- Apache-2.0 license
- NoopReranker fallback when model unavailable
- GitHub: [lunaris-rerank](https://docs.rs/lunaris-rerank/latest/)

**Lightweight alternative: `rerank-rs`**
- ms-marco-MiniLM-L6-v2 (22M params)
- ~12ms per pair on CPU
- Much smaller model (90MB vs 1.1GB)
- Good for latency-sensitive deployments
- GitHub: [bhk97/rerank-rs](https://github.com/bhk97/rerank-rs)

### Integration

```rust
use lunaris_rerank::{Reranker, BgeRerankerV2M3, NoopReranker};

struct SearchPipeline {
    reranker: Box<dyn Reranker>,
}

impl SearchPipeline {
    fn new() -> Self {
        // Try to load BGE reranker, fall back to no-op
        let reranker: Box<dyn Reranker> = match BgeRerankerV2M3::try_new_from_default_cache() {
            Ok(r) => Box::new(r),
            Err(_) => {
                tracing::warn!("reranker model unavailable, using no-op");
                Box::new(NoopReranker)
            }
        };
        Self { reranker }
    }

    async fn search(&self, query: &str, candidates: Vec<SearchResult>) -> Vec<SearchResult> {
        // 1. Get top-30 candidates via RRF (BM25 + vector)
        let rrf_results = self.rrf_search(query, 30).await;

        // 2. Rerank with cross-encoder
        let docs: Vec<String> = rrf_results.iter()
            .map(|r| r.snippet.clone())
            .collect();
        let reranked = self.reranker.rerank(query, docs).await?;

        // 3. Return top-k (typically 5-10)
        reranked.into_iter()
            .zip(rrf_results)
            .map(|(score, mut result)| {
                result.scores.rerank = Some(score);
                result
            })
            .take(10)
            .collect()
    }
}
```

**Source:** [lunaris-rerank docs.rs](https://docs.rs/lunaris-rerank/latest/), [rerank-rs GitHub](https://github.com/bhk97/rerank-rs), [Ralph Hero reranker research](https://github.com/cdubiel08/ralph-hero/blob/main/thoughts/shared/research/2026-04-26-GH-0901-local-cross-encoder-reranker-m5-pro.md)

---

## 6. Deduplication & Near-Duplicate Detection

### Current State

WebFind uses SimHash for near-duplicate detection. This is a single-algorithm approach that misses many near-duplicate cases.

### Options

| Crate | Algorithms | Speed | Use Case | License |
|-------|-----------|-------|----------|---------|
| **txtfp** | MinHash + LSH + SimHash + TLSH + Embeddings | 9K docs/s (MinHash) | Full dedup pipeline | MIT |
| **gaoya** | MinHash + SimHash | Fast | Dedup + clustering | MIT |
| **sketchir** | MinHash + SimHash + LSH | Fast | Near-dup detection | MIT |

### Recommendation: `txtfp`

- **MinHash + LSH:** Sub-linear near-duplicate lookup (O(1) avg query)
- **SimHash:** Bit-LSH near-duplicate detection (Hamming distance)
- **TLSH:** Byte-level locality-sensitive hash
- **Semantic embeddings:** ONNX local embedding similarity
- **Unicode-correct canonicalization:** NFKC + casefold + Bidi strip (defends against Trojan Source)
- **Streaming + offline fingerprinters:** Both whole-doc and chunk-fed variants
- **Byte-stable hash layouts:** `repr(C)` `bytemuck::Pod`, semver-frozen
- GitHub: [themankindproject/txtfp](https://github.com/themankindproject/txtfp)

### Integration

```toml
[dependencies]
txtfp = "0.3"  # minhash + simhash + lsh (default features)
```

```rust
use txtfp::{
    Canonicalizer, Fingerprinter, MinHashFingerprinter, ShingleTokenizer,
    WordTokenizer, jaccard,
};

struct DeduplicationIndex {
    canonicalizer: Canonicalizer,
    fingerprinter: MinHashFingerprinter<ShingleTokenizer<WordTokenizer>, 128>,
    // LSH index for sub-linear lookup
    lsh_index: txtfp::LshIndex,
}

impl DeduplicationIndex {
    fn new() -> Self {
        let canonicalizer = Canonicalizer::default();
        let tokenizer = ShingleTokenizer { k: 5, inner: WordTokenizer };
        let fingerprinter = MinHashFingerprinter::<_, 128>::new(canonicalizer.clone(), tokenizer);
        let lsh_index = txtfp::LshIndexBuilder::for_threshold(0.8).build();
        Self { canonicalizer, fingerprinter, lsh_index }
    }

    /// Check if a document is a near-duplicate of anything already indexed
    fn is_near_duplicate(&self, text: &str) -> Option<(usize, f64)> {
        let canonical = self.canonicalizer.canonicalize(text);
        let signature = self.fingerprinter.fingerprint(&canonical).ok()?;
        let candidates = self.lsh_index.query(&signature);
        candidates.into_iter()
            .map(|(id, ref_sig)| (id, jaccard(&signature, ref_sig)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .filter(|(_, sim)| *sim > 0.8)
    }
}
```

**Source:** [txtfp GitHub](https://github.com/themankindproject/txtfp), [gaoya crates.io](https://crates.io/crates/gaoya), [sketchir](https://github.com/arclabs561/sketchir)

---

## 7. Query Intent Detection & Routing

### Current State

WebFind treats all queries identically — BM25 + vector + graph fusion regardless of intent. A query like "when did X happen" (temporal) gets the same treatment as "what is X" (definitional).

### Options

| Approach | Latency | Accuracy | Complexity | License |
|----------|---------|----------|------------|---------|
| **Rule-based heuristics** | <1ms | ~70% | Low | - |
| **rust_memex::query** | <1ms | ~75% | Low | MIT |
| **sqry-nl classifier** | 2-5ms | **99.75%** | Medium | MIT |
| **computer-says-no** | ~5ms | 85-87% | Medium | Apache-2.0 |

### Recommendation

**Tier 1 (Immediate): Rule-based intent detection**
- Zero dependencies, <1ms latency
- Detect: temporal ("when", "latest"), navigational ("official", "homepage"), transactional ("download", "buy")
- Route to appropriate search strategy

**Tier 2 (Enhanced): `sqry-nl` approach**
- Fine-tuned all-MiniLM-L6-v2 for 8 intent classes
- ONNX INT8 quantized (57MB)
- 99.75% accuracy, 2.1ms p50 latency
- 4-tier response: Execute / Confirm / Disambiguate / Reject
- GitHub: [sqry-nl](https://docs.rs/sqry-nl/latest/)

### Integration

```rust
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryIntent {
    Factual,      // "what is X", "define Y"
    Temporal,     // "when did X", "latest news about Y"
    Navigational, // "official site of X", "Y homepage"
    Transactional,// "download X", "buy Y"
    Comparative,  // "X vs Y", "difference between"
    Procedural,   // "how to X", "steps for Y"
    Local,        // "X near me", "Y in [city]"
    Hybrid,       // Multiple intents detected
}

pub struct QueryRouter;

impl QueryRouter {
    /// Fast heuristic-based intent detection (<1ms)
    pub fn detect_intent(query: &str) -> QueryIntent {
        let q = query.to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();

        // Temporal signals
        if words.iter().any(|w| matches!(*w, "when" | "latest" | "recent" | "today" | "yesterday" | "ago")) {
            return QueryIntent::Temporal;
        }

        // Navigational signals
        if words.iter().any(|w| matches!(*w, "official" | "homepage" | "website" | "url")) {
            return QueryIntent::Navigational;
        }

        // Transactional signals
        if words.iter().any(|w| matches!(*w, "download" | "buy" | "price" | "purchase" | "free")) {
            return QueryIntent::Transactional;
        }

        // Comparative signals
        if q.contains(" vs ") || q.contains(" versus ") || q.contains("difference between") {
            return QueryIntent::Comparative;
        }

        // Procedural signals
        if words.first().map_or(false, |w| *w == "how") || q.contains("steps to") {
            return QueryIntent::Procedural;
        }

        // Default: factual/hybrid
        QueryIntent::Factual
    }

    /// Route query to optimal search strategy
    pub fn route(&self, query: &str) -> SearchStrategy {
        match Self::detect_intent(query) {
            QueryIntent::Temporal => SearchStrategy {
                bm25_weight: 0.3,
                vector_weight: 0.3,
                freshness_weight: 0.4,  // Prioritize recency
                ..Default::default()
            },
            QueryIntent::Navigational => SearchStrategy {
                bm25_weight: 0.6,       // Exact match matters
                vector_weight: 0.1,
                domain_boost: true,     // Boost official domains
                ..Default::default()
            },
            QueryIntent::Factual => SearchStrategy {
                bm25_weight: 0.4,
                vector_weight: 0.4,
                graph_weight: 0.2,
                ..Default::default()
            },
            _ => SearchStrategy::default(),
        }
    }
}
```

**Source:** [rust_memex::query](https://docs.rs/rust-memex/latest/rust_memex/query/), [sqry-nl docs.rs](https://docs.rs/sqry-nl/latest/), [computer-says-no GitHub](https://github.com/srobroek/computer-says-no)

---

## 8. Alternative Full-Text Search Engines

### Current State

WebFind uses Tantivy as the primary BM25 index. This is a separate on-disk index that must be kept in sync with the graph store.

### Options

| Engine | Type | Latency | Scale | Integration | License |
|--------|------|---------|-------|-------------|---------|
| **Tantivy** (current) | Standalone library | <5ms | 100M+ docs | Separate index | Mature |
| **Turso native FTS** | In-process SQL | <5ms | 1M+ docs | Unified with storage | Beta |
| **SeekStorm** | Rust search server | <1ms | 100M+ docs | HTTP API | AGPL/MIT |

### Recommendation

**For Turso migration:** Use Turso's native FTS (Tantivy-powered)
- Same Tantivy engine, but index lives inside the Turso file
- No separate index directory to manage
- Transactional consistency with data
- `fts_match()`, `fts_score()`, `fts_highlight()` functions

**For advanced use cases:** SeekStorm
- Sub-millisecond full-text search
- Multi-tenancy support
- Built-in SymSpell spelling correction
- BM25 + vector + graph in one server
- GitHub: [SeekStorm](https://github.com/SeekStorm/SeekStorm)

**Source:** [Turso FTS docs](https://docs.turso.tech/sql-reference/functions/fts), [SeekStorm](https://github.com/SeekStorm/SeekStorm)

---

## 9. Additional Enhancements

### 9.1 Query Expansion with Synonyms

**Problem:** BM25 misses results that use synonyms (e.g., "car" vs "automobile").

**Solution:** Static synonym map with FTS query expansion.

```rust
/// Expand query tokens with synonyms for FTS5/Turso FTS
fn expand_query(query: &str) -> String {
    let synonyms: &[(&[&str], &[&str])] = &[
        (&["buy", "purchase", "bought"], &["buy", "purchase", "bought", "acquire"]),
        (&["movie", "film"], &["movie", "film", "cinema"]),
        (&["doctor", "physician"], &["doctor", "physician", "dr"]),
    ];

    let mut expanded = Vec::new();
    for token in query.split_whitespace() {
        expanded.push(token.to_string());
        for (terms, expansions) in synonyms {
            if terms.contains(&token.to_lowercase().as_str()) {
                for exp in *expansions {
                    if !terms.contains(exp) {
                        expanded.push(exp.to_string());
                    }
                }
            }
        }
    }
    expanded.join(" OR ")
}
```

**Source:** [mag query expansion PR](https://github.com/George-RD/mag/pull/49)

### 9.2 Content Classification

**Problem:** WebFind indexes all content equally. A forum post and a research paper get the same treatment.

**Solution:** Use `rs-trafilatura`'s page-type classification to apply different indexing strategies:

```rust
match result.metadata.page_type {
    PageType::Article => {
        // Full indexing: BM25 + vector + graph
        index_full(content);
    }
    PageType::Forum => {
        // Thread-level indexing, lower priority
        index_with_priority(content, Priority::Low);
    }
    PageType::Product => {
        // Structured extraction: price, rating, specs
        index_structured(content);
    }
    PageType::Documentation => {
        // Section-level chunking for better retrieval
        index_chunked(content, ChunkStrategy::BySection);
    }
    _ => {
        // Default indexing
        index_basic(content);
    }
}
```

### 9.3 Streaming Index Updates

**Problem:** WebFind indexes in batch. New content is not searchable until the next batch commit.

**Solution:** Use Tantivy's `IndexWriter::commit()` with a short interval (e.g., 5 seconds) for near-real-time indexing:

```rust
// In the crawler pipeline:
let mut writer = index.writer(50_000_000)?;
writer.add_document(doc)?;
// Commit every 5 seconds or every 100 documents
if Instant::now() - last_commit > Duration::from_secs(5) {
    writer.commit()?;
    last_commit = Instant::now();
}
```

### 9.4 Query Result Caching

**Problem:** Repeated queries (especially popular ones) re-execute the full search pipeline.

**Solution:** LRU cache for search results with TTL:

```rust
use moka::sync::Cache;

struct SearchCache {
    cache: Cache<String, SearchResponse>,
}

impl SearchCache {
    fn new() -> Self {
        let cache = Cache::builder()
            .max_capacity(10_000)
            .time_to_live(Duration::from_secs(300))  // 5 minute TTL
            .time_to_idle(Duration::from_secs(60))   // 1 minute idle
            .build();
        Self { cache }
    }

    fn get_or_compute<F>(&self, query: &str, f: F) -> SearchResponse
    where F: FnOnce() -> SearchResponse {
        if let Some(cached) = self.cache.get(query) {
            return cached;
        }
        let result = f();
        self.cache.insert(query.to_string(), result.clone());
        result
    }
}
```

### 9.5 Async Embedding with `ort` Directly

**Problem:** `fastembed` is synchronous and blocks the async runtime during embedding.

**Solution:** Use `ort` (ONNX Runtime) directly with `tokio::task::spawn_blocking`:

```rust
use ort::{Environment, Session, Value};
use tokio::task;

async fn embed_batch(texts: &[&str]) -> Result<Vec<Vec<f32>>> {
    let texts = texts.to_vec();
    task::spawn_blocking(move || {
        let session = Session::builder()?
            .with_model_from_file("model.onnx")?;
        // Batch inference
        let outputs = session.run(ort::inputs!["input_ids" => ...])?;
        // Extract embeddings
        Ok(extract_embeddings(outputs))
    }).await?
}
```

**Source:** [rust-embedding-bench](https://github.com/jerrythomas/rust-embedding-bench) — shows `ort` fp32 at 3,052 eps vs fastembed at 2,849 eps on Apple M4 Max.

---

## 10. Prioritized Implementation Roadmap

### Phase 1 — Quick Wins (1-2 weeks each)

| # | Enhancement | Impact | Effort | Dependencies |
|---|------------|--------|--------|--------------|
| 1 | Embedding upgrade to `bge-small-en-v1.5` | +5% retrieval | Low | None (fastembed already supports it) |
| 2 | Query spelling correction with `symspell_rs` | Handles 10-15% queries | Low | Build dictionary from indexed terms |
| 3 | Query intent detection (rule-based) | Better routing | Low | None |
| 4 | Query result caching with `moka` | Faster repeat queries | Low | Add moka dependency |

### Phase 2 — Medium Effort (2-4 weeks each)

| # | Enhancement | Impact | Effort | Dependencies |
|---|------------|--------|--------|--------------|
| 5 | Content extraction upgrade to `rs-trafilatura` | +15% F1 score | Medium | Replace readability-rust |
| 6 | Deduplication with `txtfp` (MinHash + LSH) | Better dedup | Low | Add txtfp dependency |
| 7 | Vector search upgrade to `diskann-rs` | 6-10× lower RAM | Medium | Replace DashMap vector store |
| 8 | Query expansion with synonyms | Better recall | Low | Static synonym map |

### Phase 3 — Advanced (4-8 weeks each)

| # | Enhancement | Impact | Effort | Dependencies |
|---|------------|--------|--------|--------------|
| 9 | Cross-encoder reranking with `lunaris-rerank` | +5-15% MRR | Medium | Model download (1.1GB) |
| 10 | Query intent classifier (ONNX) | 99.75% accuracy | Medium | Train on WebFind query logs |
| 11 | Content classification by page type | Better indexing | Medium | rs-trafilatura integration |
| 12 | Async embedding with `ort` directly | Non-blocking | Medium | ort dependency |

---

## Appendix: Research Sources

### Embedding Models
- [PromptQuorum: Best Local Embedding Models 2026](https://www.promptquorum.com/power-local-llm/best-embedding-models-local-rag-2026) — 6 models tested on 4 document types, 100 queries each
- [rust-embedding-bench](https://github.com/jerrythomas/rust-embedding-bench) — Apples-to-apples benchmark: fastembed vs ort vs candle vs ollama vs llama-cpp-2
- [RunLocalAI Embeddings](https://www.runlocalai.co/embeddings) — Model comparison matrix with MTEB scores
- [D-Central: Local Embedding Models](https://d-central.tech/local-embedding-models/) — Quality-per-VRAM analysis

### Content Extraction
- [rs-trafilatura GitHub](https://github.com/Murrough-Foley/rs-trafilatura) — F1=0.966, 7 page types, spider-rs integration
- [readex docs.rs](https://docs.rs/readex/latest/readex/) — Readability + Trafilatura + htmldate cascade
- [contextractor](https://github.com/contextractor/contextractor) — Production deployment of rs-trafilatura

### Vector Search
- [diskann-rs crates.io](https://crates.io/crates/diskann-rs) — Disk-backed ANN with incremental updates
- [DiskANN vs HNSW Benchmark](https://github.com/JohnDouglasJDX/diskann-vs-hnsw-benchmark) — 3.38M vectors, 3.3× QPS advantage
- [ann-search-rs](https://github.com/GregorLueg/ann-search-rs) — 150K samples, all standard algorithms

### Reranking
- [lunaris-rerank docs.rs](https://docs.rs/lunaris-rerank/latest/) — BGE-Reranker-v2-m3, 12ms p50
- [rerank-rs GitHub](https://github.com/bhk97/rerank-rs) — Lightweight cross-encoder
- [Ralph Hero reranker research](https://github.com/cdubiel08/ralph-hero/blob/main/thoughts/shared/research/2026-04-26-GH-0901-local-cross-encoder-reranker-m5-pro.md) — M5 Pro latency analysis

### Deduplication
- [txtfp GitHub](https://github.com/themankindproject/txtfp) — MinHash + LSH + SimHash + TLSH + embeddings
- [gaoya crates.io](https://crates.io/crates/gaoya) — MinHash + SimHash, Python bindings
- [sketchir](https://github.com/arclabs561/sketchir) — Sketching primitives for IR

### Query Understanding
- [rust_memex::query](https://docs.rs/rust-memex/latest/rust_memex/query/) — Intent detection and routing
- [sqry-nl docs.rs](https://docs.rs/sqry-nl/latest/) — NL to query translation, 99.75% accuracy
- [computer-says-no GitHub](https://github.com/srobroek/computer-says-no) — ~5ms intent classification

### Spelling Correction
- [symspell_rs docs.rs](https://docs.rs/symspell_rs/latest/) — 1M× faster than Norvig
- [SeekStorm blog](https://seekstorm.com/blog/sub-millisecond-compound-aware-automatic-spelling-correction/) — SymSpell integration guide

---

*End of Enhancement Catalog*
