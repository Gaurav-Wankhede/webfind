// webfind-models: ManifestTensor for Stage 1 Jev routing.
// Evaluates boolean presence of LLM manifests and site descriptors.

use serde::{Deserialize, Serialize};

/// High-density boolean feature tensor representing site-level LLM manifest presence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub struct ManifestTensor {
    /// Presence of /llms-full.txt or /.well-known/llms-full.txt
    pub has_llms_full_txt: bool,
    /// Presence of /llms.txt or /.well-known/llms.txt
    pub has_llms_txt: bool,
    /// Presence of /ai-catalog.json, /ai-catelog.json, or .well-known variants
    pub has_ai_catalog_json: bool,
}

impl ManifestTensor {
    /// Creates a new ManifestTensor with explicit boolean flags.
    #[inline]
    #[must_use]
    pub const fn new(has_llms_full_txt: bool, has_llms_txt: bool, has_ai_catalog_json: bool) -> Self {
        Self {
            has_llms_full_txt,
            has_llms_txt,
            has_ai_catalog_json,
        }
    }

    /// Returns true if any manifest file is available for direct zero-scrape ingestion.
    #[inline]
    #[must_use]
    pub const fn has_any_manifest(&self) -> bool {
        self.has_llms_full_txt || self.has_llms_txt || self.has_ai_catalog_json
    }

    /// Encodes the manifest state into a 3-element float slice for Burn/ONNX inference.
    #[inline]
    #[must_use]
    pub fn to_features(&self) -> [f32; 3] {
        [
            if self.has_llms_full_txt { 1.0 } else { 0.0 },
            if self.has_llms_txt { 1.0 } else { 0.0 },
            if self.has_ai_catalog_json { 1.0 } else { 0.0 },
        ]
    }
}
