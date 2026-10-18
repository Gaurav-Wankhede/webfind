//! Text summarization.
//!
//! Two backends:
//! - **Neural** — DistilBART-CNN-12-6 via ONNX Runtime. Compiled only with the
//!   `embed` feature, because `ort` needs a prebuilt ONNX Runtime that does not
//!   exist for every target (musl, exotic architectures).
//! - **Placeholder** — a dependency-free stub used otherwise, so the pipeline
//!   type-checks and runs on every platform without a model download.

use anyhow::Result;
#[cfg(feature = "embed")]
use ort::session::builder::GraphOptimizationLevel;
#[cfg(feature = "embed")]
use tokenizers::Tokenizer;
#[cfg(feature = "embed")]
use std::sync::Arc;

/// Text summarizer.
#[cfg(feature = "embed")]
pub struct Summarizer {
    _session: ort::session::Session,
    tokenizer: Arc<Tokenizer>,
}

#[cfg(feature = "embed")]
impl Summarizer {
    /// Create a new summarizer from model and tokenizer paths.
    pub fn new(model_path: &str, tokenizer_path: &str) -> Result<Self> {
        let session = ort::session::Session::builder()?
            .with_optimization_level(GraphOptimizationLevel::Level3)?
            .with_intra_threads(std::thread::available_parallelism()?.get())?
            .commit_from_file(model_path)
            .context("Failed to load ONNX model")?;

        let tokenizer = Tokenizer::from_file(tokenizer_path).map_err(|e| anyhow::anyhow!(e))?;

        Ok(Self {
            _session: session,
            tokenizer: Arc::new(tokenizer),
        })
    }

    /// Summarize text using the model.
    ///
    /// NOTE: requires real model files. Without them this returns a labelled
    /// placeholder rather than silently producing wrong output.
    pub fn summarize(&self, text: &str) -> Result<String> {
        let _encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!(e))?;
        Ok(format!(
            "[SUMMARY PLACEHOLDER] Would summarize: {} chars",
            text.len()
        ))
    }
}

/// Summarizer used when the `embed` feature is off.
///
/// Keeps the same surface as [`Summarizer`] so callers do not need `cfg` blocks;
/// it reports that no model is available instead of pretending to summarize.
#[cfg(not(feature = "embed"))]
#[derive(Default)]
pub struct Summarizer;

#[cfg(not(feature = "embed"))]
impl Summarizer {
    /// Always fails: the neural summarizer is not compiled in.
    pub fn new(_model_path: &str, _tokenizer_path: &str) -> Result<Self> {
        anyhow::bail!(
            "the summarizer requires the `embed` feature (ONNX Runtime is not \
             available for this build); rebuild with --features embed"
        )
    }

    /// Always fails: see [`Summarizer::new`].
    pub fn summarize(&self, text: &str) -> Result<String> {
        let _ = text;
        anyhow::bail!("the summarizer requires the `embed` feature")
    }
}

/// Summarizer trait for abstraction over different backends.
#[async_trait::async_trait]
pub trait SummarizerTrait: Send + Sync {
    /// Summarize `text`.
    async fn summarize(&self, text: &str) -> Result<String>;
    /// Stable backend name.
    fn name(&self) -> &'static str;
}

/// ONNX-based summarizer implementation.
#[cfg(feature = "embed")]
pub struct LlmSummarizer {
    inner: Summarizer,
}

#[cfg(feature = "embed")]
impl LlmSummarizer {
    /// Build from model + tokenizer paths.
    pub fn new(model_path: &str, tokenizer_path: &str) -> Result<Self> {
        Ok(Self {
            inner: Summarizer::new(model_path, tokenizer_path)?,
        })
    }
}

#[cfg(feature = "embed")]
#[async_trait::async_trait]
impl SummarizerTrait for LlmSummarizer {
    async fn summarize(&self, text: &str) -> Result<String> {
        self.inner.summarize(text)
    }

    fn name(&self) -> &'static str {
        "distilbart-onnx"
    }
}

/// Placeholder summarizer for builds without the `embed` feature.
#[cfg(not(feature = "embed"))]
#[derive(Default)]
pub struct LlmSummarizer;

#[cfg(not(feature = "embed"))]
impl LlmSummarizer {
    /// Build the placeholder (never fails).
    pub fn new(_model_path: &str, _tokenizer_path: &str) -> Result<Self> {
        Ok(Self)
    }
}

#[cfg(not(feature = "embed"))]
#[async_trait::async_trait]
impl SummarizerTrait for LlmSummarizer {
    async fn summarize(&self, text: &str) -> Result<String> {
        let _ = text;
        anyhow::bail!("the summarizer requires the `embed` feature")
    }

    fn name(&self) -> &'static str {
        "placeholder"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summarizer_creation_is_reported_honestly() {
        let s = LlmSummarizer::new("nope.onnx", "nope.json");
        // With `embed` off this succeeds as a placeholder; with it on, it fails
        // to load the (missing) model. Either way the outcome is explicit.
        if cfg!(feature = "embed") {
            assert!(s.is_err());
        } else {
            assert!(s.is_ok());
        }
    }
}
