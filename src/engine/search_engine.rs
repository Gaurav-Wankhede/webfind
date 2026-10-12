//! Search engine trait: abstracts BM25 + vector search operations.
//! Implementations: SurrealDB (production), in-memory (tests).

use std::collections::HashMap;

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::schema::content::StructuredContent;
use crate::schema::response::{ScoreBreakdown, SearchResult};

/// High-level search operations abstracting BM25 + vector search.
#[async_trait]
pub trait SearchEngine: Send + Sync {
    /// Index a single document for BM25 search.
    async fn index_one(&self, content: &StructuredContent) -> Result<()>;

    /// Index a batch of documents and commit once.
    async fn index_batch(&self, items: &[StructuredContent]) -> Result<u64>;

    /// Search with BM25 and return ranked results.
    async fn search_bm25(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>;

    /// Index a document for vector search.
    async fn index_vector(&self, id: &str, text: &str) -> Result<()>;

    /// Batch index documents for vector search.
    async fn index_vector_batch(&self, items: &[(String, String)]) -> Result<()>;

    /// Search for top-k most similar documents using vector similarity.
    async fn search_vector(&self, query: &str, top_k: usize) -> Result<HashMap<String, f64>>;

    /// Total indexed documents.
    async fn doc_count(&self) -> Result<u64>;

    /// Search BM25 returning `Arc<StructuredContent>` to avoid field-by-field clone.
    /// Default implementation returns empty — real implementations should override.
    async fn search_bm25_arc(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<std::sync::Arc<StructuredContent>>> {
        Ok(Vec::new())
    }

    /// Optimize the index (force merge segments).
    async fn optimize(&self) -> Result<()>;
}

/// In-memory search engine for tests and local crawls.
///
/// Lexical scoring is delegated to [`Bm25Index`], which implements real BM25
/// (IDF + document-length normalization + term-frequency saturation) over an
/// inverted index. The previous implementation scored with
/// `title.contains(term) ? 10.0 : 0.0`, which had none of those properties and
/// scanned every document on every query.
pub struct InMemorySearchEngine {
    documents: RwLock<Vec<std::sync::Arc<StructuredContent>>>,
    vectors: dashmap::DashMap<String, Vec<f32>>,
    embedder: Option<std::sync::Arc<dyn crate::engine::embedder::Embedder>>,
    /// Real BM25 lexical index, keyed by document URL.
    lexical: crate::engine::bm25_index::Bm25Index,
}

impl InMemorySearchEngine {
    pub fn new() -> Self {
        Self {
            documents: RwLock::new(Vec::new()),
            vectors: dashmap::DashMap::new(),
            embedder: None,
            lexical: crate::engine::bm25_index::Bm25Index::default(),
        }
    }

    pub fn with_embedder(embedder: std::sync::Arc<dyn crate::engine::embedder::Embedder>) -> Self {
        Self {
            documents: RwLock::new(Vec::new()),
            vectors: dashmap::DashMap::new(),
            embedder: Some(embedder),
            lexical: crate::engine::bm25_index::Bm25Index::default(),
        }
    }

    /// The text a document is lexically indexed under: title plus body, so a
    /// title match outranks an incidental body mention through term frequency
    /// rather than an arbitrary constant.
    fn lexical_text(content: &StructuredContent) -> String {
        if content.title.is_empty() {
            content.content_text.clone()
        } else {
            format!("{} {}", content.title, content.content_text)
        }
    }
}

impl Default for InMemorySearchEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchEngine for InMemorySearchEngine {
    async fn index_one(&self, content: &StructuredContent) -> Result<()> {
        if !content.is_valid_content {
            return Ok(());
        }
        // Index vector if embedder is available.
        if let Some(ref embedder) = self.embedder {
            let text = if content.title.is_empty() {
                content.url.clone()
            } else {
                format!("{} {}", content.title, content.excerpt)
            };
            let vectors = embedder.embed(&[&text])?;
            if let Some(vec) = vectors.into_iter().next() {
                self.vectors.insert(content.url.clone(), vec);
            }
        }
        // Real BM25 lexical index, keyed by URL so re-indexing replaces.
        self.lexical
            .upsert(&content.url, &Self::lexical_text(content));
        self.documents
            .write()
            .await
            .push(std::sync::Arc::new(content.clone()));
        Ok(())
    }

    async fn index_batch(&self, items: &[StructuredContent]) -> Result<u64> {
        let valid: Vec<std::sync::Arc<StructuredContent>> = items
            .iter()
            .filter(|c| c.is_valid_content)
            .map(|c| std::sync::Arc::new(c.clone()))
            .collect();
        let count = valid.len() as u64;
        // Index vectors for documents if an embedder is available.
        if let Some(ref embedder) = self.embedder {
            let texts: Vec<String> = valid
                .iter()
                .map(|c| {
                    if c.title.is_empty() {
                        c.url.clone()
                    } else {
                        format!("{} {}", c.title, c.excerpt)
                    }
                })
                .collect();
            if !texts.is_empty() {
                let inputs: Vec<&str> = texts.iter().map(|t| t.as_str()).collect();
                let vectors = embedder.embed(&inputs)?;
                for (item, vec) in valid.iter().zip(vectors) {
                    self.vectors.insert(item.url.clone(), vec);
                }
            }
        }
        // Feed the lexical index, then re-fit avgdl against the new corpus so
        // length normalization tracks the real document set rather than the
        // initial estimate.
        for item in &valid {
            self.lexical.upsert(&item.url, &Self::lexical_text(item));
        }
        if count > 0 {
            self.lexical.refit();
        }
        self.documents.write().await.extend(valid);
        Ok(count)
    }

    async fn search_bm25(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Real BM25 via the inverted index — only documents sharing a token
        // with the query are scored, and scores carry IDF and length
        // normalization. No linear scan over the corpus.
        let hits = self.lexical.search(query, limit);

        let docs = self.documents.read().await;
        let by_url: std::collections::HashMap<&str, &std::sync::Arc<StructuredContent>> =
            docs.iter().map(|d| (d.url.as_str(), d)).collect();

        let results = hits
            .into_iter()
            .enumerate()
            .filter_map(|(rank, (url, score))| {
                let doc = by_url.get(url.as_str())?;
                Some(SearchResult {
                    rank: (rank + 1) as u32,
                    url: doc.url.clone(),
                    title: doc.title.clone(),
                    snippet: doc.excerpt.clone(),
                    domain: url::Url::parse(&doc.url)
                        .map(|u| u.host_str().unwrap_or("").to_string())
                        .unwrap_or_default(),
                    published_at: doc.published_at,
                    modified_at: doc.modified_at,
                    crawled_at: doc.fetched_at,
                    author: doc.author.clone(),
                    site_name: doc.site_name.clone(),
                    score: f64::from(score),
                    scores: ScoreBreakdown {
                        bm25: Some(f64::from(score)),
                        vector: None,
                        graph: None,
                        freshness: Some(crate::engine::ranker::Ranker::freshness_score(
                            doc.published_at.unwrap_or(doc.fetched_at),
                        )),
                        quality: Some(crate::engine::ranker::Ranker::quality_score(
                            doc.reading_ease,
                            doc.grade_level,
                        )),
                        ax_score: None,
                        final_score: f64::from(score),
                    },
                    content: None,
                    tier: None,
                    examples: None,
                    keywords: None,
                    metrics: None,
                    favicon: None,
                    thumbnail: None,
                    llms_txt: None,
                    ai_catalog: None,
                    openapi_spec: None,
                    mcp_server: None,
                    language: doc.language.clone(),
                    content_type: doc.content_type.clone(),
                })
            })
            .collect();

        Ok(results)
    }

    /// Search returning `Arc<StructuredContent>` to avoid field-by-field clone.
    /// Use this when the caller needs full content or will do its own conversion.
    ///
    /// Uses the same real BM25 index as [`SearchEngine::search_bm25`].
    async fn search_bm25_arc(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<std::sync::Arc<StructuredContent>>> {
        let hits = self.lexical.search(query, limit);
        let docs = self.documents.read().await;
        let by_url: std::collections::HashMap<&str, &std::sync::Arc<StructuredContent>> =
            docs.iter().map(|d| (d.url.as_str(), d)).collect();

        Ok(hits
            .into_iter()
            .filter_map(|(url, _)| by_url.get(url.as_str()).map(|d| (*d).clone()))
            .collect())
    }

    async fn index_vector(&self, id: &str, text: &str) -> Result<()> {
        if let Some(ref embedder) = self.embedder {
            let vectors = embedder.embed(&[text])?;
            if let Some(vec) = vectors.into_iter().next() {
                self.vectors.insert(id.to_string(), vec);
            }
        }
        Ok(())
    }

    async fn index_vector_batch(&self, items: &[(String, String)]) -> Result<()> {
        if let Some(ref embedder) = self.embedder {
            let inputs: Vec<&str> = items.iter().map(|(_, text)| text.as_str()).collect();
            let vectors = embedder.embed(&inputs)?;
            for ((id, _), vec) in items.iter().zip(vectors) {
                self.vectors.insert(id.clone(), vec);
            }
        }
        Ok(())
    }

    async fn search_vector(&self, query: &str, top_k: usize) -> Result<HashMap<String, f64>> {
        if let Some(ref embedder) = self.embedder {
            let qvec = embedder.embed(&[query])?;
            if let Some(query_vec) = qvec.into_iter().next() {
                let qnorm = norm(&query_vec).max(1e-8);
                let mut scored: Vec<(String, f64)> = self
                    .vectors
                    .iter()
                    .map(|entry| {
                        let id = entry.key().clone();
                        let vec = entry.value();
                        let score = cosine(&query_vec, vec, qnorm);
                        (id, score)
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                scored.truncate(top_k);
                return Ok(scored.into_iter().collect());
            }
        }
        Ok(HashMap::new())
    }

    async fn doc_count(&self) -> Result<u64> {
        Ok(self.documents.read().await.len() as u64)
    }

    async fn optimize(&self) -> Result<()> {
        // No-op for in-memory store.
        Ok(())
    }
}

fn cosine(a: &[f32], b: &[f32], a_norm: f64) -> f64 {
    let min_len = a.len().min(b.len());
    let dot: f64 = (0..min_len).map(|i| (a[i] as f64) * (b[i] as f64)).sum();
    let b_norm = norm(b).max(1e-8);
    dot / (a_norm * b_norm)
}

fn norm(v: &[f32]) -> f64 {
    v.iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt()
}
