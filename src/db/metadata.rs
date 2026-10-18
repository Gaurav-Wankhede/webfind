//! Metadata and cache tables schema
//!
//! This module defines metadata tables for caching, query history, and session state.

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Query cache entry for caching search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryCacheEntry {
    pub id: i64,
    pub query_hash: String,
    pub query_text: String,
    pub results_json: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub hit_count: u32,
}

/// Embedding cache for caching computed embeddings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingCacheEntry {
    pub id: i64,
    pub text_hash: String,
    pub text_preview: String,
    pub embedding: Vec<f32>,
    pub model_name: String,
    pub created_at: DateTime<Utc>,
}

/// Fetch cache for HTTP responses
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchCacheEntry {
    pub id: i64,
    pub url_hash: String,
    pub url: String,
    pub response_json: String,
    pub status_code: u16,
    pub content_hash: String,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// Session state for tracking user sessions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub id: i64,
    pub session_key: String,
    pub value_json: String,
    pub updated_at: DateTime<Utc>,
}

/// Security audit log entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogEntry {
    pub id: i64,
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub details_json: String,
    pub severity: AuditSeverity,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuditSeverity {
    Info,
    Warning,
    Error,
    Critical,
}

impl Default for AuditSeverity {
    fn default() -> Self {
        AuditSeverity::Info
    }
}

/// SQL statements for metadata tables
pub mod sql {
    /// Create query cache table
    pub const CREATE_QUERY_CACHE: &str = r#"
        CREATE TABLE IF NOT EXISTS query_cache (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            query_hash TEXT NOT NULL UNIQUE,
            query_text TEXT NOT NULL,
            results_json TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at DATETIME NOT NULL,
            hit_count INTEGER NOT NULL DEFAULT 0
        );
    "#;

    /// Create embedding cache table
    pub const CREATE_EMBEDDING_CACHE: &str = r#"
        CREATE TABLE IF NOT EXISTS embedding_cache (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            text_hash TEXT NOT NULL UNIQUE,
            text_preview TEXT NOT NULL,
            embedding BLOB NOT NULL,
            model_name TEXT NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    /// Create fetch cache table
    pub const CREATE_FETCH_CACHE: &str = r#"
        CREATE TABLE IF NOT EXISTS fetch_cache (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url_hash TEXT NOT NULL UNIQUE,
            url TEXT NOT NULL,
            response_json TEXT NOT NULL,
            status_code INTEGER NOT NULL,
            content_hash TEXT,
            fetched_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            expires_at DATETIME NOT NULL
        );
    "#;

    /// Create session state table
    pub const CREATE_SESSION_STATE: &str = r#"
        CREATE TABLE IF NOT EXISTS session_state (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_key TEXT NOT NULL UNIQUE,
            value_json TEXT NOT NULL,
            updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    /// Create audit log table
    pub const CREATE_AUDIT_LOG: &str = r#"
        CREATE TABLE IF NOT EXISTS audit_log (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            event_type TEXT NOT NULL,
            details_json TEXT NOT NULL,
            severity TEXT NOT NULL DEFAULT 'info'
        );
    "#;

    /// Create indexes for metadata tables
    pub const CREATE_INDEXES: &str = r#"
        CREATE INDEX IF NOT EXISTS idx_query_cache_expires ON query_cache(expires_at);
        CREATE INDEX IF NOT EXISTS idx_embedding_cache_text_hash ON embedding_cache(text_hash);
        CREATE INDEX IF NOT EXISTS idx_fetch_cache_expires ON fetch_cache(expires_at);
        CREATE INDEX IF NOT EXISTS idx_session_state_key ON session_state(session_key);
        CREATE INDEX IF NOT EXISTS idx_audit_log_timestamp ON audit_log(timestamp);
        CREATE INDEX IF NOT EXISTS idx_audit_log_severity ON audit_log(severity);
    "#;

    /// Insert query cache entry
    pub const INSERT_QUERY_CACHE: &str = r#"
        INSERT INTO query_cache (query_hash, query_text, results_json, expires_at)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(query_hash) DO UPDATE SET
            results_json = excluded.results_json,
            expires_at = excluded.expires_at,
            hit_count = hit_count + 1;
    "#;

    /// Get query cache entry
    pub const GET_QUERY_CACHE: &str = r#"
        SELECT query_text, results_json, hit_count
        FROM query_cache
        WHERE query_hash = ? AND expires_at > CURRENT_TIMESTAMP;
    "#;

    /// Clean expired cache entries
    pub const CLEAN_EXPIRED_CACHE: &str = r#"
        DELETE FROM query_cache WHERE expires_at <= CURRENT_TIMESTAMP;
        DELETE FROM fetch_cache WHERE expires_at <= CURRENT_TIMESTAMP;
    "#;

    /// Insert embedding cache
    pub const INSERT_EMBEDDING_CACHE: &str = r#"
        INSERT INTO embedding_cache (text_hash, text_preview, embedding, model_name)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(text_hash) DO UPDATE SET
            text_preview = excluded.text_preview,
            embedding = excluded.embedding,
            model_name = excluded.model_name;
    "#;

    /// Get embedding cache
    pub const GET_EMBEDDING_CACHE: &str = r#"
        SELECT embedding, model_name
        FROM embedding_cache
        WHERE text_hash = ?;
    "#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_constants_exist() {
        assert!(!sql::CREATE_QUERY_CACHE.is_empty());
        assert!(!sql::CREATE_EMBEDDING_CACHE.is_empty());
        assert!(!sql::CREATE_FETCH_CACHE.is_empty());
        assert!(!sql::CREATE_SESSION_STATE.is_empty());
        assert!(!sql::CREATE_AUDIT_LOG.is_empty());
    }
}