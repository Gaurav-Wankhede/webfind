//! Database schemas for WebFind.
//!
//! WebFind keeps its working state in the embedded Turso/libSQL store
//! (`src/storage/turso_store.rs`), which already provides FTS5 lexical search,
//! DiskANN vector search, the crawl link graph, PageRank, and cache tables.
//! This module holds the shared record shapes for that store.

pub mod documents;
pub mod crawl_graph;
pub mod metadata;

pub use documents::{DocumentRecord, DocumentWithEmbedding};
pub use crawl_graph::{CrawlNode, CrawlEdge, EdgeType};
pub use metadata::{QueryCacheEntry, EmbeddingCacheEntry, FetchCacheEntry, SessionState, AuditLogEntry, AuditSeverity};
