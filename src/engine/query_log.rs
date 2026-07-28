use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
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

/// Query log service for autocomplete
pub struct QueryLogService {
    db: Arc<surrealdb::Surreal<surrealdb::engine::any::Any>>,
    project: String,
    buffer: Arc<Mutex<HashMap<String, u64>>>,
}

impl QueryLogService {
    pub fn new(
        db: Arc<surrealdb::Surreal<surrealdb::engine::any::Any>>,
        project: String,
    ) -> Self {
        let service = Self {
            db: db.clone(),
            project: project.clone(),
            buffer: Arc::new(Mutex::new(HashMap::new())),
        };

        // Start background flush task
        let buf = service.buffer.clone();
        let db = db.clone();
        let project = project.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(60));
            loop {
                interval.tick().await;
                if let Err(e) = Self::flush_buffer(&db, &project, &buf).await {
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

        // Insert query log entry via CREATE
        let sql = r#"
            CREATE query_log CONTENT {
                project: $project,
                session_id: $session_id,
                query: $query,
                normalized_query: $normalized,
                result_count: $result_count,
                latency_ms: $latency_ms,
                created_at: time::now()
            }
        "#;

        let _: Vec<JsonValue> = self
            .db
            .query(sql)
            .bind(("project", format!("project:{}", self.project)))
            .bind(("session_id", session_id.to_string()))
            .bind(("query", query.to_string()))
            .bind(("normalized", normalized.clone()))
            .bind(("result_count", result_count as i64))
            .bind(("latency_ms", latency_ms as i64))
            .await?
            .take(0)?;

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
            match self.get_log_suggestions(&normalized, limit - suggestions.len()).await {
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
        suggestions.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap());
        suggestions.dedup_by(|a, b| a.text == b.text);
        suggestions.truncate(limit);

        Ok(suggestions)
    }

    async fn get_ngram_suggestions(&self, prefix: &str, limit: usize) -> Result<Vec<Suggestion>> {
        let sql = r#"
            SELECT ngram, count FROM query_ngram
            WHERE ngram_len <= $max_len
            AND string::starts_with(ngram, $prefix)
            AND count >= $min_count
            ORDER BY count DESC, ngram_len ASC
            LIMIT $limit
        "#;

        let mut response = self
            .db
            .query(sql)
            .bind(("max_len", MAX_NGRAM_LEN as i64))
            .bind(("prefix", prefix.to_string()))
            .bind(("min_count", NGRAM_MIN_COUNT as i64))
            .bind(("limit", limit as i64))
            .await?;

        let rows: Vec<JsonValue> = response.take(0)?;
        let mut suggestions = Vec::new();

        for row in rows {
            let ngram = Self::extract_string(&row, "ngram");
            let count = Self::extract_number(&row, "count");
            let len = Self::extract_number(&row, "ngram_len");

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
        let sql = r#"
            SELECT normalized_query, count() AS freq FROM query_log
            WHERE string::starts_with(normalized_query, $prefix)
            AND project = $project
            GROUP BY normalized_query
            ORDER BY freq DESC
            LIMIT $limit
        "#;

        let mut response = self
            .db
            .query(sql)
            .bind(("prefix", prefix.to_string()))
            .bind(("project", format!("project:{}", self.project)))
            .bind(("limit", limit as i64))
            .await?;

        let rows: Vec<JsonValue> = response.take(0)?;
        let mut suggestions = Vec::new();

        for row in rows {
            let text = Self::extract_string(&row, "normalized_query");
            let freq = Self::extract_number(&row, "freq");
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
        let sql = r#"
            SELECT normalized_query, count() AS freq FROM query_log
            WHERE project = $project
            GROUP BY normalized_query
            ORDER BY freq DESC
            LIMIT $limit
        "#;

        let mut response = self
            .db
            .query(sql)
            .bind(("project", format!("project:{}", self.project)))
            .bind(("limit", limit as i64))
            .await?;

        let rows: Vec<JsonValue> = response.take(0)?;
        let mut suggestions = Vec::new();

        for row in rows {
            let text = Self::extract_string(&row, "normalized_query");
            let freq = Self::extract_number(&row, "freq");
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

    fn extract_string(value: &JsonValue, key: &str) -> String {
        if let JsonValue::Object(obj) = value {
            if let Some(v) = obj.get(key) {
                if let JsonValue::String(s) = v {
                    return s.clone();
                }
            }
        }
        if let JsonValue::Array(arr) = value {
            if let Some(first) = arr.first() {
                return Self::extract_string(first, key);
            }
        }
        String::new()
    }

    fn extract_number(value: &JsonValue, key: &str) -> i64 {
        if let JsonValue::Object(obj) = value {
            if let Some(v) = obj.get(key) {
                return match v {
                    JsonValue::Number(n) => n.as_i64().unwrap_or(0),
                    _ => 0,
                };
            }
        }
        if let JsonValue::Array(arr) = value {
            if let Some(first) = arr.first() {
                return Self::extract_number(first, key);
            }
        }
        0
    }

    async fn flush_buffer(
        db: &surrealdb::Surreal<surrealdb::engine::any::Any>,
        _project: &str,
        buffer: &Arc<Mutex<HashMap<String, u64>>>,
    ) -> Result<()> {
        let mut buf = buffer.lock().await;
        if buf.is_empty() {
            return Ok(());
        }

        // Batch upsert ngrams using UPSERT with count increment
        for (ngram, count) in buf.drain() {
            let words: Vec<&str> = ngram.split_whitespace().collect();
            let len = words.len() as u32;

            // UPSERT pattern: if record exists, increment count; otherwise create with count.
            let sql = r#"
                UPSERT query_ngram SET
                    ngram = $ngram,
                    ngram_len = $len,
                    count += $count,
                    last_seen = time::now()
            "#;

            if let Err(e) = db
                .query(sql)
                .bind(("ngram", ngram))
                .bind(("len", len as i64))
                .bind(("count", count as i64))
                .await
            {
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
        assert_eq!(QueryLogService::normalize_query("Rust Programming"), "rust programming");
        assert_eq!(QueryLogService::normalize_query("  Hello,   World!  "), "hello world");
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
}
