//! Embedding backend for dense vector search.
//!
//! Two implementations are provided:
//! - [`FastembedEmbedder`] — the neural backend (EmbeddingGemma via ONNX).
//!   Compiled only with the `embed` feature, because `ort` needs a prebuilt
//!   ONNX Runtime that does not exist for every target (musl, exotic arches).
//! - [`DummyEmbedder`] — deterministic, zero-dependency hashing embedder.
//!   Always available, so builds and tests work on every platform.

use anyhow::Result;

/// Embedding backend for dense vector search.
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts into normalized float vectors.
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
}

/// Deterministic dummy embedder for tests, CI, and platforms without ONNX.
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

/// Neural embedder backed by `fastembed` / ONNX Runtime.
///
/// Only compiled with the `embed` feature — see the module docs for why the
/// dependency is optional.
#[cfg(feature = "embed")]
mod neural {
    use super::Embedder;
    use anyhow::{Context, Result};
    use fastembed::TextEmbedding;

    /// Production embedder backed by `fastembed`.
    pub struct FastembedEmbedder {
        model: TextEmbedding,
    }

    impl FastembedEmbedder {
        /// Load the default embedding model.
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
}

#[cfg(feature = "embed")]
pub use neural::FastembedEmbedder;
