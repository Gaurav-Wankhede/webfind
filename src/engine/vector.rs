use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use tracing::warn;

use crate::engine::embedder::Embedder;
use crate::storage::vector_store::VectorStore;

/// Dense-vector search engine.
///
/// Wraps an embedder and an in-memory vector store. Callers index documents
/// as they are added to the BM25 index, then search with a query string.
pub struct VectorEngine {
    embedder: Arc<dyn Embedder>,
    store: VectorStore,
}

impl VectorEngine {
    pub fn new(embedder: Arc<dyn Embedder>) -> Self {
        Self {
            embedder,
            store: VectorStore::new(),
        }
    }

    /// Embed and store a single document.
    pub fn index(&self, id: &str, text: &str) -> Result<()> {
        let text = if text.trim().is_empty() { id } else { text };
        let vectors = self.embedder.embed(&[text])?;
        if let Some(vec) = vectors.into_iter().next() {
            self.store.insert(id.to_string(), vec);
        }
        Ok(())
    }

    /// Batch index several documents.
    pub fn index_batch(&self, items: &[(String, String)]) -> Result<()> {
        if items.is_empty() {
            return Ok(());
        }
        let inputs: Vec<&str> = items
            .iter()
            .map(|(id, text)| {
                if text.trim().is_empty() {
                    id.as_str()
                } else {
                    text.as_str()
                }
            })
            .collect();
        let vectors = self.embedder.embed(&inputs)?;
        for ((id, _), vec) in items.iter().zip(vectors) {
            self.store.insert(id.clone(), vec);
        }
        Ok(())
    }

    /// Search for the top-k most similar documents and return a URL -> score map.
    pub fn search(&self, query: &str, top_k: usize) -> Result<HashMap<String, f64>> {
        let qvec = self.embedder.embed(&[query])?;
        let Some(query_vec) = qvec.into_iter().next() else {
            return Ok(HashMap::new());
        };
        if self.store.is_empty() {
            warn!("vector search requested but no vectors are indexed");
            return Ok(HashMap::new());
        }
        Ok(self.store.search(&query_vec, top_k).into_iter().collect())
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::embedder::DummyEmbedder;

    #[test]
    fn test_vector_engine_roundtrip() {
        let engine = VectorEngine::new(Arc::new(DummyEmbedder));
        engine.index("a", "the quick brown fox").unwrap();
        engine.index("b", "lazy dog sleeping").unwrap();

        let hits = engine.search("quick fox", 5).unwrap();
        assert!(hits.contains_key("a"));
        assert!(hits.get("a").unwrap() > hits.get("b").unwrap());
    }
}
