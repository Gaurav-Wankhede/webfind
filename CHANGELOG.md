# Changelog

All notable changes to this project will be documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [0.3.0] - 2026-09-13

### Performance

- **4.4x search latency reduction** — live queries now complete in ~2,500ms vs 11,000ms previously.
- **Eliminated unconditional FTS rebuild on read path** — `rebuild_fts()` is now gated by `fts_version != graph_version` in `graph_meta`; no more full-table wipe and 460k-document rescan on every search invocation.
- **Speculative engine stream completion** — replaced `join_all` (waits for all 11 engines) with `FuturesUnordered` + early-exit: once ≥3 engines return ≥2×limit candidate hits after 1,200ms, RRF fusion proceeds immediately without stalling on slow/blocked endpoints.
- **Tightened per-engine HTTP timeout** — reduced from 10,000ms to 3,500ms in the live search path; unresponsive engines fail fast rather than consuming the full timeout budget.
- **FTS version tracking** — `fts_version` stored in `graph_meta`; `rebuild_fts` records the graph version it indexed from, enabling the read path to skip rebuilding on unchanged graphs.
- **PageRank recompute guard** — already version-gated; now shares the same `current_graph_ver` lookup as the FTS guard, eliminating one redundant DB round-trip per search.

### Bug Fixes

- **Phantom freshness default** (`src/commands/search.rs`) — `freshness` was previously `unwrap_or(1.0)`, assigning maximum freshness to every page with no publication date. Result: irrelevant pages (bestbuy.com, merriam-webster.com) ranked in top 5 for niche queries. Fixed to `Option<f64>` — `None` when no date is known, weight only applied when a real date is present.
- **Phantom quality default** — `quality` was computed as `snippet_len / 160` (snippet verbosity, not page quality) and applied unconditionally. Fixed to `None` for all live-only hits with no fetched body; signal absent from formula when `None`.
- **Final score formula conditional** — formula now branches: with freshness `score * 0.75 + freshness * 0.25 + fusion_bonus`; without freshness `score + fusion_bonus`. No phantom signal inflation.
- **Synthetic signal score collapse** (`src/commands/search.rs`, `src/storage/turso_store.rs`) — `ScoreBreakdown.bm25/vector/graph` all reported the same composite RRF value, not per-signal contributions. Fixed by tracking `bm25_score`, `vector_score`, `graph_score` individually in `HybridHit` and `TursoSearchHit`, propagated through to the rendered breakdown.
- **DDG snippet index desynchronization** (`src/engine/web_index/engines/ddg.rs`) — global `links[i]` was paired with global `snippets[i]` by position; any ad, widget, or snippet-less result shifted all downstream pairings. Fixed: row-based parsing (`table tr`) scopes each link to its own parent row's snippet; fallback to index-pairing uses `.get(i)` (bounds-safe) instead of direct indexing.
- **Potential panic in `aggregate_fused`** (`src/engine/web_index/rrf.rs`) — `.expect("key exists in map")` replaced with `if let Some(target) = by_key.get_mut(&path_key)`, eliminating a panic path on structural map changes.
- **Store-only fallback and sparse-fill unscaled scores** (`src/commands/search.rs`) — when falling back to the store or padding sparse results (< limit), store hits were previously returned at raw RRF fraction scale (~0.016–0.05) while live-fused results display in [0, 1]. Fixed: min-max normalization applied across fallback and sparse-fill store hits.
- **Non-live search score normalization** (`src/commands/search.rs`) — running `webfind search` without `--live` returned raw hybrid RRF fractions (top scores around ~0.032). Fixed: min-max normalization applied so non-live queries consistently yield scores in [0.0, 1.0].
- **Phantom quality default in Ranker** (`src/engine/ranker.rs`) — `quality` previously defaulted to `0.5` via `unwrap_or(0.5)` when content metrics were absent, inflating unanalyzed documents with an unearned 8-10% score boost. Fixed: quality is now strictly `Option<f64>` and active scoring weights dynamically rebalance over present signals.
- **Clean database FTS version mismatch** (`src/storage/turso_store.rs`) — `fts_version` was unseeded while `graph_version` seeded to `1`, causing initial clean databases to spuriously run `rebuild_fts()` on first search before any content was recorded. Fixed: `fts_version` initialized to `1` on schema creation.
- **Zero-result queries now return results** — 3-pass query relaxation (`full query → drop last token → keep first 3 words`) ensures non-empty responses for niche queries. The previously zero-result query "how the best creators design thumbnails 2025 2026 Paddy Galloway 1of10" now returns 10 relevant results.
- **RRF score normalization** — raw RRF fractions (~0.016–0.064) are now normalized to [0, 1] via min-max after `reciprocal_rank_fusion`, so displayed `Final` scores are human-readable and comparable across result sets.

### Additions

- `TursoStore::fts_version()` — reads `fts_version` from `graph_meta` for rebuild-skip guard.
- `normalize_rrf_scores()` in `src/engine/web_index/rrf.rs` — min-max scales raw RRF fractions to [0, 1]; exported from `web_index`.
- `relax_query(query, drop)` helper — drops trailing tokens to build progressively broader fallback queries.
- `log_engine_failures(outcome)` helper — emits `WARN`/`DEBUG` lines for failed engines without cluttering call sites.
- `RESULT_ROW` selector in DDG engine — enables context-scoped row-based parsing.
- 15 new unit tests across `rrf.rs` and `commands/search.rs` covering: score normalization, query relaxation, freshness/quality option semantics, multi-engine fusion bonus, URL dedup tracking, and store/live content priority.

### Internal Refactors

- `HybridHit` extended with `bm25_score: Option<f64>`, `vector_score: Option<f64>`, `graph_score: Option<f64>`.
- `TursoSearchHit` extended with same three fields, propagated from `hybrid_search` through `TursoStore::search`.
- `store_hit_to_result` drops redundant `has_bm25/has_vec/has_graph` booleans; reads per-signal scores directly.
- Duplicate `signals.push` calls in `hybrid_search` replaced with dedup-guarded pushes.
- `join_all` import removed from `web_index.rs`; replaced with `futures::stream::{FuturesUnordered, StreamExt}`.

---

## [0.2.0] - 2026-09-12

### Added

- Migrated graph store from SurrealDB to embedded Turso/libSQL — zero external server dependency.
- Hybrid search: BM25 (FTS5) + vector (DiskANN/fastembed) + PageRank RRF fusion.
- 11-engine live search fan-out: DuckDuckGo Lite, Bing, Mojeek, Marginalia, HN, Lobsters, Arxiv, Wikipedia, StackOverflow, Crates.io, MDN.
- Pure CLI architecture — removed MCP server mode; every operation is a single command invocation.
- Research command (`webfind research`) with multi-page crawl + Turso persistence.
- Curated seed catalog and curation daemon for background index growth.
- CLI flags: `--live`, `--deep`, `--dynamic`, `--hybrid`, `--graph-store`, `--turso-path`, `--output`.
- BLAKE3 content hashing, `simdutf8` UTF-8 validation, `ahash` hash maps for performance.
- Privacy fingerprint audit trail (`ip_health`, `fingerprint_log`) in Turso.
- DiskANN vector index with Turso id→URL mapping for restart-safe reopening.

### Removed

- MCP server transport layer.
- SurrealDB dependency entirely.

---

## [0.1.0] - 2026-09-10

### Added

- Initial release: WebFind MCP search engine with SurrealDB graph store, privacy fingerprinting, and Docker packaging.
