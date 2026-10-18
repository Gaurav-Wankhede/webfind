//! Shared example types for the pipeline.
//!
//! These were previously defined in `engine::report_synthesis`, which was
//! removed along with the server-side retrieval layer. The types are small and
//! pipeline-local, so they live here now.

use serde::{Deserialize, Serialize};

use crate::engine::source_tier::SourceTier;

/// Types of examples that can be extracted from page content.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ExampleType {
    Code,
    Math,
    Chemical,
    Table,
    Diagram,
    Text,
    Image,
}

/// A single extracted example from fetched content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedExample {
    pub example_type: ExampleType,
    pub content: String,
    pub language: Option<String>,
    pub latex: Option<String>,
    pub smiles: Option<String>,
    pub markdown: Option<String>,
    pub mermaid: Option<String>,
    pub image_url: Option<String>,
    pub alt_text: Option<String>,
    pub surrounding_context: Option<String>,
    pub source_url: String,
    pub source_tier: SourceTier,
}

impl Default for ExtractedExample {
    fn default() -> Self {
        Self {
            example_type: ExampleType::Text,
            content: String::new(),
            language: None,
            latex: None,
            smiles: None,
            markdown: None,
            mermaid: None,
            image_url: None,
            alt_text: None,
            surrounding_context: None,
            source_url: String::new(),
            source_tier: SourceTier::Unknown,
        }
    }
}
