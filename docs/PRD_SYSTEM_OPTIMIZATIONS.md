# PRD: WebFind High-Performance Systems & Zero-Overhead Optimizations

## 1. Executive Summary & Objective
WebFind is designed as a self-hosted, pure Rust native search engine and crawler for AI agents, executing with zero API costs, zero background servers, and strict low latency.

To ensure minimal CPU/memory footprint under heavy multi-domain crawls and deep research queries, this PRD specifies targeted performance optimizations:
1. **SIMD-Accelerated UTF-8 & String Processing** (`simdutf8`, `memchr`): Accelerate raw HTTP payload decoding and token delimiter scanning.
2. **Fast In-Memory Hashing** (`ahash`): Replace default `RandomState`/SipHash with non-cryptographic hardware-accelerated AHash in hot ranking, RRF, and deduplication loops.
3. **Small-Buffer Optimization** (`smallvec`): Eliminate heap allocations on short token vectors, URL segments, and query keywords.
4. **Zero-Copy Serialization & Stream Handling**: Streamline byte slices across fetch and index pipelines.

---

## 2. Problem Statement
1. **UTF-8 Validation Overhead**: Scraped web bodies are repeatedly validated with standard `std::str::from_utf8` during crawler body reads and regex extraction.
2. **Hashing Overhead on Hot Paths**: In `src/engine/web_index/rrf.rs`, `src/engine/indexer.rs`, and `src/engine/ranker.rs`, `HashMap<String, ...>` and `HashSet<String>` use the standard library hasher (SipHash 1-3), designed for HashDoS defense rather than raw lookup speed.
3. **Small Allocation Thrashing**: Frequently allocating short vectors (`Vec<String>` for 1-5 keywords or token segments) causes allocator fragmentation during high-concurrency crawls.

---

## 3. Targeted Optimizations & Technical Specifications

### Phase 1: High-Speed SIMD UTF-8 Validation
- **Crate**: `simdutf8 = "0.1.5"` (already in dependencies tree or easily integrated).
- **Target**: `src/engine/fetcher.rs` and crawler response body processors.
- **Specification**: Replace standard UTF-8 parsing of response byte slices with `simdutf8::basic::from_utf8(&bytes)` or `simdutf8::compat::from_utf8(&bytes)`.
- **Expected Outcome**: Up to $10\times$ faster UTF-8 validation on AVX2 / ARM NEON platforms.

### Phase 2: AHash on Hot Ranking & Graph Structures
- **Crate**: `ahash = "0.8"` (or `ahash::AHashMap`, `ahash::AHashSet`).
- **Target**:
  - `src/engine/web_index/rrf.rs`: Hit deduplication and RRF score map accumulation.
  - `src/engine/indexer.rs`: BM25 term frequency tables and keyword maps.
  - `src/engine/ranker.rs`: Domain diversity tracking and URL score caches.
- **Specification**: Replace `std::collections::HashMap` with `ahash::AHashMap` on non-public internal ranking and deduplication loops.

### Phase 3: Small-Vector Heap Allocation Defense
- **Crate**: `smallvec = "1.13"`
- **Target**: Keyword extraction tokens and URL path segments.
- **Specification**: Use `SmallVec<[String; 8]>` for tokenized terms, avoiding heap allocation when query length $\le 8$ terms.

---

## 4. Verification & Benchmarking
1. **Benchmark Suite**: Run `cargo bench` against existing microbenchmarks (`score_scalar`, `url_id`, `content_deserialize`).
2. **Correctness Tests**: All 229 unit and integration tests must pass cleanly.
3. **Live Research**: Verify `webfind research` latency and memory stability.

---

## 5. Non-Goals & Invariants
- **No breaking changes** to CLI JSON response schemas (`SearchResponse`, `SearchResult`).
- **No unsafe transmutes**; all SIMD abstractions must remain memory-safe and verified.
- **Pure CLI remains intact**; zero daemon or external service requirements.
