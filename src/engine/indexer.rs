use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::engine::embedder::Embedder;
use crate::engine::vector::VectorEngine;
use crate::schema::content::StructuredContent;
use crate::schema::response::{IndexFreshness, SearchMetadata, SearchResult};
use crate::storage::tantivy_store::{Bm25Hit, TantivyStore};

const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_INDEX_DIR: &str = "index_data";

/// High-level indexer that wraps TantivyStore and an optional vector engine.
pub struct Indexer {
    store: TantivyStore,
    vector: Option<VectorEngine>,
}

impl Indexer {
    /// Open or create an index at the default location relative to `base`.
    pub fn open(base: impl AsRef<Path>) -> Result<Self> {
        let path = base.as_ref().join(DEFAULT_INDEX_DIR);
        Self::open_at(path)
    }

    /// Open or create an index at an explicit path.
    pub fn open_at(path: impl AsRef<Path>) -> Result<Self> {
        let store = TantivyStore::open(path).context("failed to open tantivy store")?;
        Ok(Self {
            store,
            vector: None,
        })
    }

    /// Attach a vector engine to enable dense/hybrid search.
    pub fn with_vector_engine(mut self, engine: VectorEngine) -> Self {
        self.attach_vector_engine(engine);
        self
    }

    /// Attach a vector engine in-place.
    pub fn attach_vector_engine(&mut self, engine: VectorEngine) {
        self.vector = Some(engine);
    }

    /// Attach an embedder and lazily instantiate the vector engine.
    pub fn with_embedder(self, embedder: Option<Arc<dyn Embedder>>) -> Self {
        match embedder {
            Some(e) => self.with_vector_engine(VectorEngine::new(e)),
            None => self,
        }
    }

    /// Index a single document.
    pub fn index_one(&mut self, content: &StructuredContent) -> Result<()> {
        if !content.is_valid_content {
            tracing::debug!("skipping invalid content: {}", content.url);
            return Ok(());
        }
        self.store.index_content(content)?;
        if let Some(ref v) = self.vector {
            let text = format!("{} {}", content.title, content.content_text);
            v.index(&content.url, &text)?;
        }
        self.store.commit()?;
        Ok(())
    }

    /// Index a batch of documents and commit once.
    pub fn index_batch(&mut self, items: &[StructuredContent]) -> Result<u64> {
        let valid: Vec<&StructuredContent> = items.iter().filter(|c| c.is_valid_content).collect();
        let skipped = items.len() - valid.len();
        if skipped > 0 {
            tracing::debug!("skipping {} invalid documents", skipped);
        }
        let count = self.store.index_batch(&valid)?;
        if let Some(ref v) = self.vector {
            let batch: Vec<(String, String)> = valid
                .iter()
                .map(|c| (c.url.clone(), format!("{} {}", c.title, c.content_text)))
                .collect();
            v.index_batch(&batch)?;
        }
        Ok(count)
    }

    /// Search with BM25 and return ranked results.
    pub fn search_bm25(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let hits: Vec<Bm25Hit> = self.store.search(query, limit)?;

        let max_score = hits.iter().map(|h| h.bm25_score).fold(0.0f64, f64::max);

        let results: Vec<SearchResult> = hits
            .into_iter()
            .enumerate()
            .map(|(i, hit)| hit.to_search_result((i + 1) as u32, max_score))
            .collect();

        Ok(results)
    }

    /// Attach full content blocks to search results when the caller has the
    /// raw structured content available (e.g. after a fresh crawl).
    pub fn attach_content(results: &mut [SearchResult], contents: &[StructuredContent]) {
        let mut by_url: HashMap<&str, &StructuredContent> = HashMap::with_capacity(contents.len());
        for c in contents {
            by_url.insert(c.url.as_str(), c);
        }
        for r in results {
            if let Some(c) = by_url.get(r.url.as_str()) {
                r.content = Some(c.to_content_block());
                r.modified_at = c.modified_at;
                r.author = c.author.clone();
                r.site_name = c.site_name.clone();
            }
        }
    }
    /// Dense vector search for `query`. Returns an empty map if no vector engine is attached.
    pub fn search_vector(&self, query: &str, limit: usize) -> Result<HashMap<String, f64>> {
        match self.vector {
            Some(ref v) => v.search(query, limit),
            None => Ok(HashMap::new()),
        }
    }

    /// True if a vector engine is attached.
    pub fn has_vector(&self) -> bool {
        self.vector.is_some()
    }

    /// Total indexed documents.
    pub fn doc_count(&self) -> Result<u64> {
        self.store.doc_count()
    }

    /// Build search metadata for responses.
    pub fn metadata(&self, signals: Vec<String>) -> Result<SearchMetadata> {
        let count = self.doc_count()?;
        Ok(SearchMetadata {
            index_version: "1".to_string(),
            index_size: count,
            engine_version: ENGINE_VERSION.to_string(),
            searched_at: chrono::Utc::now(),
            signals_used: signals,
            index_freshness: IndexFreshness {
                oldest_page: None,
                newest_page: None,
                avg_age_days: 0.0,
            },
        })
    }

    /// Path to the index.
    pub fn index_path(&self) -> &Path {
        self.store.path()
    }

    /// Optimize the index (force merge segments).
    pub fn optimize(&mut self) -> Result<()> {
        self.store.drop_writer()?;
        tracing::info!("index optimized (writer dropped, segments merged by OS)");
        Ok(())
    }

    /// Drop the writer to release resources.
    pub fn close(mut self) -> Result<()> {
        self.store.drop_writer()
    }
}

/// Resolve the default index directory path.
pub fn default_index_path() -> PathBuf {
    dirs().join(DEFAULT_INDEX_DIR)
}

fn dirs() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::content::StructuredContent;
    use chrono::{TimeZone, Utc};
    use tempfile::TempDir;

    fn sample(url: &str, title: &str, body: &str) -> StructuredContent {
        StructuredContent {
            url: url.to_string(),
            final_url: url.to_string(),
            status_code: 200,
            title: title.to_string(),
            description: None,
            canonical_url: None,
            language: "en".to_string(),
            language_confidence: 0.95,
            published_at: Some(Utc.with_ymd_and_hms(2025, 6, 15, 10, 0, 0).unwrap()),
            modified_at: None,
            author: None,
            site_name: None,
            content_text: body.to_string(),
            content_html: format!("<p>{}</p>", body),
            content_markdown: body.to_string(),
            excerpt: body.chars().take(200).collect(),
            word_count: body.split_whitespace().count() as u32,
            char_count: body.len() as u32,
            sentence_count: 1,
            reading_time_seconds: 30,
            reading_ease: 65.0,
            grade_level: 8.0,
            keywords: vec![],
            open_graph: None,
            twitter_card: None,
            json_ld: vec![],
            schema_type: None,
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: body.to_string(),
            fetched_at: Utc::now(),
            fetch_duration_ms: 150,
            html_size_bytes: 1024,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            is_paywalled: false,
            is_valid_content: true,
        }
    }

    #[test]
    fn test_indexer_search_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let mut indexer = Indexer::open(tmp.path()).unwrap();

        let docs = vec![
            sample(
                "https://rust-lang.org",
                "Rust Language",
                "Rust is a systems language for reliability and performance.",
            ),
            sample(
                "https://go.dev",
                "Go Language",
                "Go is a simple language for building fast concurrent software.",
            ),
            sample(
                "https://typescriptlang.org",
                "TypeScript",
                "TypeScript adds static types to JavaScript for large scale apps.",
            ),
        ];

        let count = indexer.index_batch(&docs).unwrap();
        assert_eq!(count, 3);

        let results = indexer.search_bm25("systems language", 10).unwrap();
        assert!(!results.is_empty());
        assert!(results[0].title.contains("Rust"));

        let meta = indexer.metadata(vec!["bm25".to_string()]).unwrap();
        assert_eq!(meta.index_size, 3);
    }

    #[test]
    fn test_indexer_vector_roundtrip() {
        use crate::engine::embedder::DummyEmbedder;
        let tmp = TempDir::new().unwrap();
        let mut indexer = Indexer::open(tmp.path())
            .unwrap()
            .with_vector_engine(VectorEngine::new(Arc::new(DummyEmbedder)));

        let docs = vec![
            sample(
                "https://rust-lang.org",
                "Rust Language",
                "Rust is a systems language for reliability and performance.",
            ),
            sample(
                "https://go.dev",
                "Go Language",
                "Go is a simple language for building fast concurrent software.",
            ),
        ];
        indexer.index_batch(&docs).unwrap();
        let hits = indexer.search_vector("systems language", 5).unwrap();
        assert!(hits.contains_key("https://rust-lang.org"));
    }
}
