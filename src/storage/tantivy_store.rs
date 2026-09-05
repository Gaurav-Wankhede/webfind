//! Dead module removed 2026-08-13 (quality audit).
//!
//! `TantivyStore` had zero callers — the search engine is
//! `engine::search_engine::InMemorySearchEngine` (BM25 + fastembed), not a
//! Tantivy-backed store. `pub mod tantivy_store` was dropped from
//! `storage/mod.rs`; README "Tantivy index/engine" claims corrected.
//!
//! Delete this file.
