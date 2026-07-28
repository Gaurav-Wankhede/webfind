//! SurrealDB search engine implementation.
//! Uses SurrealDB's native BM25 FTS + HNSW vector search.

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;

use crate::engine::embedder::Embedder;
use crate::schema::content::StructuredContent;
use crate::schema::request::ContentType;
use crate::schema::response::{ScoreBreakdown, SearchResult};

use super::search_engine::SearchEngine;

/// SurrealDB-backed search engine using native BM25 + HNSW.
pub struct SurrealSearchEngine {
    db: Arc<Surreal<Any>>,
    embedder: Option<Arc<dyn Embedder>>,
}

impl SurrealSearchEngine {
    /// Create a new SurrealDB search engine.
    pub fn new(db: Arc<Surreal<Any>>) -> Self {
        Self {
            db,
            embedder: None,
        }
    }

    /// Attach an embedder for vector search.
    pub fn with_embedder(mut self, embedder: Arc<dyn Embedder>) -> Self {
        self.embedder = Some(embedder);
        self
    }

    /// Apply the unified schema DDL to the database.
    pub async fn apply_schema(&self) -> Result<()> {
        let schema = include_str!("../../schema/unified.surql");
        self.db.query(schema).await.context("apply schema")?;
        Ok(())
    }
}

#[async_trait]
impl SearchEngine for SurrealSearchEngine {
    async fn index_one(&self, content: &StructuredContent) -> Result<()> {
        if !content.is_valid_content {
            return Ok(());
        }

        // Index for BM25 search in url_node table
        self.db
            .query(
                "UPSERT type::record('url_node', $id) SET
                    url = $url,
                    domain = $domain,
                    source = 'crawl',
                    depth = 0,
                    priority = 1.0,
                    discovered_at = $fetched_at,
                    crawled = true,
                    title = $title,
                    content_text = $content_text,
                    excerpt = $excerpt,
                    word_count = $word_count,
                    reading_ease = $reading_ease,
                    grade_level = $grade_level,
                    language = $language,
                    author = $author,
                    site_name = $site_name,
                    published_at = $published_at,
                    modified_at = $modified_at,
                    schema_type = $schema_type,
                    is_paywalled = $is_paywalled,
                    ssl_valid = $ssl_valid",
            )
            .bind(("id", sha256_id(&content.url)))
            .bind(("url", content.url.clone()))
            .bind(("domain", extract_domain(&content.url)))
            .bind(("title", content.title.clone()))
            .bind(("content_text", content.content_text.clone()))
            .bind(("excerpt", content.excerpt.clone()))
            .bind(("word_count", content.word_count))
            .bind(("reading_ease", content.reading_ease))
            .bind(("grade_level", content.grade_level))
            .bind(("language", content.language.clone()))
            .bind(("author", content.author.clone().unwrap_or_default()))
            .bind(("site_name", content.site_name.clone().unwrap_or_default()))
            .bind(("published_at", content.published_at))
            .bind(("modified_at", content.modified_at))
            .bind(("schema_type", content.schema_type.clone().unwrap_or_default()))
            .bind(("is_paywalled", content.is_paywalled))
            .bind(("ssl_valid", content.ssl_valid))
            .bind(("fetched_at", content.fetched_at))
            .await
            .map_err(|e| anyhow::anyhow!("index url_node {}: {}", sha256_id(&content.url), e))?;

        // Index vector embedding if embedder is attached
        if let Some(ref embedder) = self.embedder {
            let text = format!("{} {}", content.title, content.content_text);
            let vectors = embedder.embed(&[&text])?;
            if let Some(vec) = vectors.into_iter().next() {
                self.db
                    .query("UPDATE type::record('url_node', ) SET embedding = ")
                    .bind(("id", sha256_id(&content.url)))
                    .bind(("embedding", vec))
                    .await
                    .context("index embedding")?;
            }
        }

        Ok(())
    }

    async fn index_batch(&self, items: &[StructuredContent]) -> Result<u64> {
        let valid: Vec<&StructuredContent> = items
            .iter()
            .filter(|c| c.is_valid_content)
            .collect();
        let count = valid.len() as u64;

        for content in &valid {
            self.index_one(content).await?;
        }

        Ok(count)
    }

    async fn search_bm25(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Use SurrealDB's native BM25 FTS
        let mut response = self
            .db
            .query(
                "SELECT url, title, excerpt, content_text,
                    search::score(0) AS title_score,
                    search::score(1) AS content_score,
                    search::score(0) * 10 + search::score(1) AS bm25
                 FROM url_node
                 WHERE title @0@ $query OR content_text @1@ $query
                 ORDER BY bm25 DESC
                 LIMIT $limit",
            )
            .bind(("query", query.to_string()))
            .bind(("limit", limit as i64))
            .await
            .context("bm25 search")?;

        let results: Vec<SearchResult> = response
            .take::<Vec<serde_json::Value>>(0)?
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                let title_score = row
                    .get("title_score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);
                let content_score = row
                    .get("content_score")
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0);

                let bm25_score = title_score * 10.0 + content_score;
                let url_str = row
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                let domain = url::Url::parse(&url_str)
                    .map(|u| u.host_str().unwrap_or("").to_string())
                    .unwrap_or_default();

                SearchResult {
                    rank: (i + 1) as u32,
                    url: url_str,
                    title: row
                        .get("title")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    snippet: row
                        .get("excerpt")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    domain,
                    published_at: None,
                    modified_at: None,
                    crawled_at: chrono::Utc::now(),
                    author: None,
                    site_name: None,
                    score: bm25_score,
                    scores: ScoreBreakdown {
                        bm25: bm25_score,
                        vector: None,
                        graph: None,
                        freshness: None,
                        quality: None,
                        final_score: bm25_score,
                    },
                    content: None,
                    keywords: None,
                    metrics: None,
                    favicon: None,
                    thumbnail: None,
                    language: "en".to_string(),
                    content_type: ContentType::Any,
                }
            })
            .collect();

        Ok(results)
    }

    async fn index_vector(&self, id: &str, text: &str) -> Result<()> {
        if let Some(ref embedder) = self.embedder {
            let vectors = embedder.embed(&[text])?;
            if let Some(vec) = vectors.into_iter().next() {
                self.db
                    .query("UPDATE type::record('url_node', ) SET embedding = ")
                    .bind(("id", id.to_string()))
                    .bind(("embedding", vec))
                .await
                .map_err(|e| anyhow::anyhow!("index vector {}: {}", id, e))?;
            }
        }
        Ok(())
    }

    async fn index_vector_batch(&self, items: &[(String, String)]) -> Result<()> {
        if let Some(ref embedder) = self.embedder {
            let inputs: Vec<&str> = items.iter().map(|(_, text)| text.as_str()).collect();
            let vectors = embedder.embed(&inputs)?;
            for ((id, _), vec) in items.iter().zip(vectors) {
                self.db
                    .query("UPDATE type::record('url_node', ) SET embedding = ")
                    .bind(("id", id.clone()))
                    .bind(("embedding", vec))
                    .await
                    .context("index vector batch")?;
            }
        }
        Ok(())
    }

    async fn search_vector(&self, query: &str, top_k: usize) -> Result<HashMap<String, f64>> {
        if let Some(ref embedder) = self.embedder {
            let qvec = embedder.embed(&[query])?;
            if let Some(query_vec) = qvec.into_iter().next() {
                // Use SurrealDB's HNSW vector search
                let mut response = self
                    .db
                    .query(
                        "SELECT url, vector::distance::knn() AS distance
                         FROM url_node
                         WHERE embedding <|1,$limit|> $query_vec
                         ORDER BY distance",
                    )
                    .bind(("query_vec", query_vec))
                    .bind(("limit", top_k as i64))
                    .await
                    .context("vector search")?;

                let results: HashMap<String, f64> = response
                    .take::<Vec<serde_json::Value>>(0)?
                    .into_iter()
                    .filter_map(|row| {
                        let url = row.get("url")?.as_str()?.to_string();
                        let distance = row.get("distance")?.as_f64()?;
                        // Convert distance to similarity score (1 / (1 + distance))
                        let score = 1.0 / (1.0 + distance);
                        Some((url, score))
                    })
                    .collect();

                return Ok(results);
            }
        }
        Ok(HashMap::new())
    }

    async fn doc_count(&self) -> Result<u64> {
        let mut response = self
            .db
            .query("SELECT count() FROM url_node GROUP ALL")
            .await
            .context("doc count")?;

        let count: u64 = response
            .take::<Option<serde_json::Value>>(0)?
            .and_then(|v| v.get("count")?.as_u64())
            .unwrap_or(0);

        Ok(count)
    }

    async fn optimize(&self) -> Result<()> {
        // SurrealDB handles optimization automatically.
        // No-op for now.
        Ok(())
    }
}

/// Generate a SHA-256 based ID for a URL (first16 bytes as hex).
fn sha256_id(url: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(url.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result[..16])
}

/// Extract domain from URL.
fn extract_domain(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .and_then(|host| host.split(':').next())
        .unwrap_or(url)
        .to_string()
}
