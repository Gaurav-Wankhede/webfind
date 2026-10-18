//! Documents table schema with vector search support
//!
//! This module defines the documents table schema including:
//! - Full-text search via FTS5
//! - Vector similarity search via DiskANN
//! - Source-tier classification

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};
use crate::engine::source_tier::SourceTier;

/// Document record stored in the database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRecord {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub content_markdown: String,
    pub content_text: String,
    pub domain: String,
    pub language: String,
    pub source_tier: SourceTier,
    pub fetched_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Document with vector embedding for similarity search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentWithEmbedding {
    #[serde(flatten)]
    pub record: DocumentRecord,
    pub embedding: Vec<f32>,
}

/// SQL statements for creating the documents table and indexes
pub mod sql {
    /// Create the main documents table
    pub const CREATE_DOCUMENTS_TABLE: &str = r#"
        CREATE TABLE IF NOT EXISTS documents (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL,
            content_markdown TEXT NOT NULL,
            content_text TEXT NOT NULL,
            domain TEXT NOT NULL,
            language TEXT NOT NULL DEFAULT 'en',
            source_tier TEXT NOT NULL DEFAULT 'unknown',
            fetched_at DATETIME NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    /// Create FTS5 virtual table for full-text search
    pub const CREATE_FTS5_INDEX: &str = r#"
        CREATE VIRTUAL TABLE IF NOT EXISTS documents_fts USING fts5(
            title,
            content_text,
            content_markdown,
            url,
            domain,
            tokenize = 'porter unicode61'
        );
    "#;

    /// Create trigger to keep FTS5 index in sync
    pub const CREATE_FTS5_TRIGGERS: &str = r#"
        CREATE TRIGGER IF NOT EXISTS documents_fts_insert AFTER INSERT ON documents BEGIN
            INSERT INTO documents_fts (title, content_text, content_markdown, url, domain)
            VALUES (new.title, new.content_text, new.content_markdown, new.url, new.domain);
        END;

        CREATE TRIGGER IF NOT EXISTS documents_fts_delete AFTER DELETE ON documents BEGIN
            DELETE FROM documents_fts WHERE url = old.url;
        END;

        CREATE TRIGGER IF NOT EXISTS documents_fts_update AFTER UPDATE ON documents BEGIN
            DELETE FROM documents_fts WHERE url = old.url;
            INSERT INTO documents_fts (title, content_text, content_markdown, url, domain)
            VALUES (new.title, new.content_text, new.content_markdown, new.url, new.domain);
        END;
    "#;

    /// Create DiskANN vector index for approximate nearest neighbor search
    pub const CREATE_VECTOR_INDEX: &str = r#"
        CREATE INDEX IF NOT EXISTS documents_embedding_idx 
        ON documents (libsql_vector_idx(embedding, 'type=diskann', 'metric=cosine'));
    "#;

    /// Insert document with embedding
    pub const INSERT_DOCUMENT: &str = r#"
        INSERT INTO documents (
            url, title, content_markdown, content_text, domain, language,
            source_tier, fetched_at, embedding
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, vector32(?))
        ON CONFLICT(url) DO UPDATE SET
            title = excluded.title,
            content_markdown = excluded.content_markdown,
            content_text = excluded.content_text,
            domain = excluded.domain,
            language = excluded.language,
            source_tier = excluded.source_tier,
            fetched_at = excluded.fetched_at,
            embedding = excluded.embedding,
            updated_at = CURRENT_TIMESTAMP;
    "#;

    /// Search by vector similarity (exact)
    pub const SEARCH_EMBEDDING_EXACT: &str = r#"
        SELECT id, url, title, domain, source_tier,
               vector_distance_cos(embedding, vector32(?)) as distance
        FROM documents
        WHERE embedding IS NOT NULL
        ORDER BY distance ASC
        LIMIT ?;
    "#;

    /// Search by vector similarity (approximate via DiskANN)
    pub const SEARCH_EMBEDDING_ANN: &str = r#"
        SELECT id, url, title, domain, source_tier, distance
        FROM vector_top_k('documents_embedding_idx', vector32(?), ?)
        JOIN documents ON documents.rowid = vector_top_k.id;
    "#;

    /// Hybrid search: BM25 + vector similarity
    pub const SEARCH_HYBRID: &str = r#"
        WITH bm25_results AS (
            SELECT rowid, bm25(documents_fts) as bm25_score
            FROM documents_fts
            WHERE documents_fts MATCH ?
            ORDER BY bm25_score
            LIMIT ?
        ),
        vector_results AS (
            SELECT rowid, vector_distance_cos(embedding, vector32(?)) as vector_distance
            FROM documents
            WHERE embedding IS NOT NULL
            ORDER BY vector_distance
            LIMIT ?
        )
        SELECT d.*, 
               COALESCE(b.bm25_score, 999999) as bm25_score,
               COALESCE(v.vector_distance, 999999) as vector_distance
        FROM documents d
        LEFT JOIN bm25_results b ON d.rowid = b.rowid
        LEFT JOIN vector_results v ON d.rowid = v.rowid
        ORDER BY (COALESCE(b.bm25_score, 999999) * 0.5 + COALESCE(v.vector_distance, 999999) * 0.5) ASC
        LIMIT ?;
    "#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_constants_exist() {
        assert!(!sql::CREATE_DOCUMENTS_TABLE.is_empty());
        assert!(!sql::CREATE_FTS5_INDEX.is_empty());
        assert!(!sql::CREATE_VECTOR_INDEX.is_empty());
    }
}