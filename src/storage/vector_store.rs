use std::collections::HashMap;

use dashmap::DashMap;

/// In-memory dense vector index with brute-force cosine search.
///
/// This is intentionally simple: it is correct for small-to-medium indexes and
/// avoids the complexity and build-time cost of an HNSW dependency. Swap in
/// `hnsw_rs` later if profiling shows it is needed.
pub struct VectorStore {
    vectors: DashMap<String, Vec<f32>>,
}

impl VectorStore {
    pub fn new() -> Self {
        Self {
            vectors: DashMap::new(),
        }
    }

    pub fn insert(&self, id: String, vector: Vec<f32>) {
        self.vectors.insert(id, vector);
    }

    pub fn len(&self) -> usize {
        self.vectors.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vectors.is_empty()
    }

    /// Search for the `top_k` most similar vectors using cosine similarity.
    pub fn search(&self, query: &[f32], top_k: usize) -> Vec<(String, f64)> {
        let qnorm = norm(query).max(1e-8);
        let mut scored: Vec<(String, f64)> = self
            .vectors
            .iter()
            .map(|entry| {
                let id = entry.key().clone();
                let vec = entry.value();
                let score = cosine(query, vec, qnorm);
                (id, score)
            })
            .collect();
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    /// Return a copy of all stored vectors.
    pub fn all(&self) -> HashMap<String, Vec<f32>> {
        self.vectors
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().clone()))
            .collect()
    }
}

impl Default for VectorStore {
    fn default() -> Self {
        Self::new()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_store_cosine() {
        let store = VectorStore::new();
        store.insert("a".to_string(), vec![1.0, 0.0, 0.0]);
        store.insert("b".to_string(), vec![0.0, 1.0, 0.0]);
        store.insert("c".to_string(), vec![1.0, 1.0, 0.0]);

        let results = store.search(&[1.0, 0.0, 0.0], 2);
        assert_eq!(results[0].0, "a");
        assert!(results[0].1 > 0.99);
        assert_eq!(results.len(), 2);
    }
}
