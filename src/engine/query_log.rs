use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use libsql::{Connection, params};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tracing::warn;

/// Suggestion result for autocomplete
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Suggestion {
    pub text: String,
    pub score: f64,
    pub source: SuggestionSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SuggestionSource {
    QueryLog,
    Ngram,
    PopularQuery,
}

const MAX_NGRAM_LEN: u32 = 3;
const MAX_SUGGESTIONS: usize = 10;
const NGRAM_MIN_COUNT: u64 = 2;

/// Query log service for autocomplete, backed by the Turso/libSQL database.
///
/// Replaces the SurrealDB-backed implementation; uses the same table names
/// (`query_log`, `query_ngram`) so the schema is portable. The `Connection`
/// is cheaply cloneable and `Send + Sync`, so the background ngram-flush task
/// shares it safely.
pub struct QueryLogService {
    conn: Connection,
    project: String,
    buffer: Arc<Mutex<HashMap<String, u64>>>,
}

impl QueryLogService {
    pub async fn new(conn: Connection, project: String) -> Self {
        // Idempotent schema for the log + ngram tables.
        if let Err(e) = conn
            .execute_batch(
                r#"
            CREATE TABLE IF NOT EXISTS query_log (
                id               INTEGER PRIMARY KEY AUTOINCREMENT,
                project          TEXT NOT NULL,
                session_id       TEXT NOT NULL DEFAULT '',
                query            TEXT NOT NULL,
                normalized_query TEXT NOT NULL,
                result_count     INTEGER NOT NULL DEFAULT 0,
                latency_ms       INTEGER NOT NULL DEFAULT 0,
                created_at       TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS query_ngram (
                ngram     TEXT PRIMARY KEY,
                ngram_len INTEGER NOT NULL,
                count     INTEGER NOT NULL DEFAULT 0,
                last_seen TEXT NOT NULL
            );
            "#,
            )
            .await
        {
            warn!("failed to ensure query_log schema: {}", e);
        }

        let service = Self {
            conn: conn.clone(),
            project: project.clone(),
            buffer: Arc::new(Mutex::new(HashMap::new())),
        };

        // Start background flush task
        let buf = service.buffer.clone();
        let conn = conn;
        let project = project.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(e) = Self::flush_buffer(&conn, &project, &buf).await {
                    warn!("failed to flush ngram buffer: {}", e);
                }
            }
        });

        service
    }

    /// Log a query and extract n-grams
    pub async fn log_query(
        &self,
        query: &str,
        session_id: &str,
        result_count: u64,
        latency_ms: u64,
    ) -> Result<()> {
        let normalized = Self::normalize_query(query);
        if normalized.is_empty() {
            return Ok(());
        }

        self.conn
            .execute(
                r#"
                INSERT INTO query_log
                    (project, session_id, query, normalized_query, result_count,
                     latency_ms, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                "#,
                params![
                    self.project.as_str(),
                    session_id,
                    query,
                    normalized.clone(),
                    result_count as i64,
                    latency_ms as i64,
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .await
            .context("log query")?;

        // Extract and buffer n-grams for async update
        let ngrams = Self::extract_ngrams(&normalized);
        if !ngrams.is_empty() {
            let mut buf = self.buffer.lock().await;
            for ng in ngrams {
                *buf.entry(ng).or_insert(0) += 1;
            }
        }

        Ok(())
    }

    /// Get autocomplete suggestions for a prefix
    pub async fn get_suggestions(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let normalized = Self::normalize_query(prefix);
        if normalized.is_empty() {
            return Ok(Vec::new());
        }

        let mut suggestions = Vec::new();
        let limit = limit.min(MAX_SUGGESTIONS);

        // 1. Try n-gram prefix completion (best for mid-typing)
        if normalized.len() >= 2 {
            match self.get_ngram_suggestions(&normalized, limit).await {
                Ok(s) => suggestions.extend(s),
                Err(e) => warn!("ngram suggestions failed: {}", e),
            }
        }

        // 2. Query log prefix match (good for full queries)
        if suggestions.len() < limit {
            match self
                .get_log_suggestions(&normalized, limit - suggestions.len())
                .await
            {
                Ok(s) => suggestions.extend(s),
                Err(e) => warn!("log suggestions failed: {}", e),
            }
        }

        // 3. Popular queries fallback (when prefix is short)
        if suggestions.len() < limit && normalized.len() <= 3 {
            match self.get_popular_queries(limit - suggestions.len()).await {
                Ok(s) => suggestions.extend(s),
                Err(e) => warn!("popular queries failed: {}", e),
            }
        }

        // Deduplicate and sort by score
        suggestions.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        suggestions.dedup_by(|a, b| a.text == b.text);
        suggestions.truncate(limit);

        Ok(suggestions)
    }

    async fn get_ngram_suggestions(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT ngram, count, ngram_len FROM query_ngram
                WHERE ngram_len <= ?1 AND ngram LIKE ?2 AND count >= ?3
                ORDER BY count DESC, ngram_len ASC
                LIMIT ?4
                "#,
                params![
                    MAX_NGRAM_LEN as i64,
                    format!("{}%", prefix),
                    NGRAM_MIN_COUNT as i64,
                    limit as i64,
                ],
            )
            .await
            .context("query ngram suggestions")?;

        let mut suggestions = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate ngram suggestions"),
            };
            let ngram: String = row.get(0).unwrap_or_default();
            let count: i64 = row.get(1).unwrap_or(0);
            let len: i64 = row.get(2).unwrap_or(0);
            if ngram.is_empty() {
                continue;
            }
            let score = (count as f64).ln().max(0.0) * 1.5
                + (MAX_NGRAM_LEN as f64 - len as f64).max(0.0) * 0.1;
            suggestions.push(Suggestion {
                text: ngram,
                score,
                source: SuggestionSource::Ngram,
            });
        }
        Ok(suggestions)
    }

    async fn get_log_suggestions(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT normalized_query, COUNT(*) AS freq
                FROM query_log
                WHERE normalized_query LIKE ?1 AND project = ?2
                GROUP BY normalized_query
                ORDER BY freq DESC
                LIMIT ?3
                "#,
                params![format!("{}%", prefix), self.project.as_str(), limit as i64],
            )
            .await
            .context("query log suggestions")?;

        let mut suggestions = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate log suggestions"),
            };
            let text: String = row.get(0).unwrap_or_default();
            let freq: i64 = row.get(1).unwrap_or(0);
            if text.is_empty() {
                continue;
            }
            suggestions.push(Suggestion {
                text,
                score: (freq as f64).ln().max(0.0) * 0.8,
                source: SuggestionSource::QueryLog,
            });
        }
        Ok(suggestions)
    }

    async fn get_popular_queries(&self, limit: usize) -> Result<Vec<Suggestion>> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT normalized_query, COUNT(*) AS freq
                FROM query_log
                WHERE project = ?1
                GROUP BY normalized_query
                ORDER BY freq DESC
                LIMIT ?2
                "#,
                params![self.project.as_str(), limit as i64],
            )
            .await
            .context("query popular queries")?;

        let mut suggestions = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate popular queries"),
            };
            let text: String = row.get(0).unwrap_or_default();
            let freq: i64 = row.get(1).unwrap_or(0);
            if text.is_empty() {
                continue;
            }
            suggestions.push(Suggestion {
                text,
                score: (freq as f64).ln().max(0.0) * 0.5,
                source: SuggestionSource::PopularQuery,
            });
        }
        Ok(suggestions)
    }

    fn normalize_query(query: &str) -> String {
        query
            .to_lowercase()
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn extract_ngrams(query: &str) -> Vec<String> {
        let words: Vec<&str> = query.split_whitespace().collect();
        if words.is_empty() {
            return Vec::new();
        }
        let mut ngrams = Vec::new();

        for len in 1..=MAX_NGRAM_LEN {
            for window in words.windows(len as usize) {
                ngrams.push(window.join(" "));
            }
        }

        ngrams
    }

    async fn flush_buffer(
        conn: &Connection,
        _project: &str,
        buffer: &Arc<Mutex<HashMap<String, u64>>>,
    ) -> Result<()> {
        let mut buf = buffer.lock().await;
        if buf.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().to_rfc3339();
        for (ngram, count) in buf.drain() {
            let len = ngram.split_whitespace().count() as i64;
            let result = conn
                .execute(
                    r#"
                    INSERT INTO query_ngram (ngram, ngram_len, count, last_seen)
                    VALUES (?1, ?2, ?3, ?4)
                    ON CONFLICT(ngram) DO UPDATE SET
                        count     = query_ngram.count + excluded.count,
                        last_seen = excluded.last_seen
                    "#,
                    params![ngram, len, count as i64, now.as_str()],
                )
                .await;
            if let Err(e) = result {
                warn!("failed to upsert ngram: {}", e);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_query() {
        assert_eq!(
            QueryLogService::normalize_query("Rust Programming"),
            "rust programming"
        );
        assert_eq!(
            QueryLogService::normalize_query("  Hello,   World!  "),
            "hello world"
        );
        assert_eq!(QueryLogService::normalize_query("rust-lang"), "rustlang");
        assert_eq!(QueryLogService::normalize_query(""), "");
    }

    #[test]
    fn test_extract_ngrams_unigram() {
        let ngrams = QueryLogService::extract_ngrams("rust programming");
        assert!(ngrams.contains(&"rust".to_string()));
        assert!(ngrams.contains(&"programming".to_string()));
    }

    #[test]
    fn test_extract_ngrams_bigram() {
        let ngrams = QueryLogService::extract_ngrams("rust programming language");
        assert!(ngrams.contains(&"rust programming".to_string()));
        assert!(ngrams.contains(&"programming language".to_string()));
    }

    #[test]
    fn test_extract_ngrams_trigram() {
        let ngrams = QueryLogService::extract_ngrams("rust programming language book");
        assert!(ngrams.contains(&"rust programming language".to_string()));
        assert!(ngrams.contains(&"programming language book".to_string()));
    }

    #[test]
    fn test_extract_ngrams_empty() {
        let ngrams = QueryLogService::extract_ngrams("");
        assert!(ngrams.is_empty());
    }

    #[test]
    fn test_extract_ngrams_short_query() {
        let ngrams = QueryLogService::extract_ngrams("rust");
        assert_eq!(ngrams.len(), 1);
        assert_eq!(ngrams[0], "rust");
    }

    #[tokio::test]
    async fn test_log_and_suggest() {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .expect("open db");
        let conn = db.connect().expect("connect");
        let svc = QueryLogService::new(conn, "test".to_string()).await;

        svc.log_query("rust programming", "s1", 5, 10)
            .await
            .expect("log query");
        svc.log_query("rust tools", "s1", 3, 8)
            .await
            .expect("log query");

        // Log-sourced suggestion by prefix.
        let s = svc.get_suggestions("rust", 5).await.expect("suggestions");
        assert!(
            !s.is_empty(),
            "expected suggestions for prefix `rust`, got {:?}",
            s
        );
    }
}
