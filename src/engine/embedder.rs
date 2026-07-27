use anyhow::{Context, Result};
use fastembed::TextEmbedding;

/// Embedding backend for dense vector search.
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts into normalized float vectors.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Production embedder backed by `fastembed`.
pub struct FastembedEmbedder {
    model: TextEmbedding,
}

impl FastembedEmbedder {
    pub fn new() -> Result<Self> {
        let model = TextEmbedding::try_new(Default::default())
            .context("load fastembed text embedding model")?;
        Ok(Self { model })
    }
}

impl Embedder for FastembedEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        self.model
            .embed(texts.to_vec(), None)
            .map_err(|e| anyhow::anyhow!("fastembed failed: {}", e))
    }
}

/// Deterministic dummy embedder for tests and CI.
///
/// Produces a 32-dimensional vector derived from the input text so tests
/// do not need to download a model.
pub struct DummyEmbedder;

impl Embedder for DummyEmbedder {
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        let mut out = Vec::with_capacity(texts.len());
        for text in texts {
            let mut vec = vec![0.0f32; 32];
            let bytes = text.as_bytes();
            for (i, b) in bytes.iter().enumerate() {
                vec[i % 32] += (*b as f32) / 255.0;
            }
            normalize(&mut vec);
            out.push(vec);
        }
        Ok(out)
    }
}

fn normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|v| v * v).sum::<f32>().sqrt().max(1e-8);
    for v in vec.iter_mut() {
        *v /= norm;
    }
}
