// webfind-models: MetadataDescriptorTensor for Stage 1 Jev routing.
// Evaluates boolean presence of LLM manifests, site topology, API specs, and embedded head metadata.
// Optimized per RUST_SYSTEMS_OPTIMIZATION_HANDBOOK: [repr(C)] zero-heap cache-line packing.

use serde::{Deserialize, Serialize};

/// High-density boolean feature tensor representing site-level metadata and manifest presence.
/// 10 boolean flags packed into contiguous memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(C)]
pub struct ManifestTensor {
    // Tier 1: Machine AI Manifests (<20ms direct ingestion)
    pub has_llms_full_txt: bool,
    pub has_llms_txt: bool,
    pub has_ai_catalog_json: bool,

    // Tier 2: Site Topology & Machine API Specifications
    pub has_sitemap_xml: bool,
    pub has_robots_txt: bool,
    pub has_openapi_spec: bool,
    pub has_rss_feed: bool,

    // Tier 3: Embedded Head Semantic Metadata (<1KB first-chunk stream)
    pub has_json_ld: bool,
    pub has_open_graph: bool,
    pub has_canonical_link: bool,
}

impl ManifestTensor {
    /// Returns true if any high-tier machine manifest or structured topology is available.
    #[inline]
    #[must_use]
    pub const fn has_any_machine_metadata(&self) -> bool {
        self.has_llms_full_txt
            || self.has_llms_txt
            || self.has_ai_catalog_json
            || self.has_sitemap_xml
            || self.has_openapi_spec
    }

    /// Converts the descriptor flags into a normalized float feature array for Burn tensor ingestion.
    #[inline]
    #[must_use]
    pub fn to_features(&self) -> [f32; 10] {
        [
            if self.has_llms_full_txt { 1.0 } else { 0.0 },
            if self.has_llms_txt { 1.0 } else { 0.0 },
            if self.has_ai_catalog_json { 1.0 } else { 0.0 },
            if self.has_sitemap_xml { 1.0 } else { 0.0 },
            if self.has_robots_txt { 1.0 } else { 0.0 },
            if self.has_openapi_spec { 1.0 } else { 0.0 },
            if self.has_rss_feed { 1.0 } else { 0.0 },
            if self.has_json_ld { 1.0 } else { 0.0 },
            if self.has_open_graph { 1.0 } else { 0.0 },
            if self.has_canonical_link { 1.0 } else { 0.0 },
        ]
    }
}
