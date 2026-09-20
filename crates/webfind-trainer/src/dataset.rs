// webfind-trainer: Multi-tier metadata probe item and batching utilities.
// Grounded in authentic internet network boundaries and metadata hierarchy.

use burn::tensor::backend::Backend;
use burn::tensor::{Int, Tensor};
use rand::Rng;
use serde::{Deserialize, Serialize};

/// Input item representing multi-tier metadata probe metrics for a single domain/URL.
/// Features: 12-dimensional vector:
/// [0] has_llms_full_txt
/// [1] has_llms_txt
/// [2] has_ai_catalog_json
/// [3] has_sitemap_xml
/// [4] has_robots_txt
/// [5] has_openapi_spec
/// [6] has_rss_feed
/// [7] has_json_ld
/// [8] has_open_graph
/// [9] is_doc_subdomain
/// [10] requires_js
/// [11] bot_challenge
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProbeItem {
    pub features: [f32; 12],
    pub target_protocol: usize,
    pub target_scores: [f32; 3],
}

/// Batch container ready for Burn tensor operations.
pub struct ProbeBatch<B: Backend> {
    pub inputs: Tensor<B, 2>,
    pub target_protocols: Tensor<B, 1, Int>,
    pub target_scores: Tensor<B, 2>,
}

/// Batching helper converting a slice of ProbeItem into Burn Tensors.
pub fn batch_items<B: Backend>(items: &[ProbeItem], device: &B::Device) -> ProbeBatch<B> {
    let batch_size = items.len();
    let mut flat_inputs = Vec::with_capacity(batch_size * 12);
    let mut flat_targets = Vec::with_capacity(batch_size);
    let mut flat_scores = Vec::with_capacity(batch_size * 3);

    for item in items {
        flat_inputs.extend_from_slice(&item.features);
        flat_targets.push(item.target_protocol as i64);
        flat_scores.extend_from_slice(&item.target_scores);
    }

    let inputs = Tensor::<B, 1>::from_floats(flat_inputs.as_slice(), device)
        .reshape([batch_size, 12]);
    let target_protocols =
        Tensor::<B, 1, Int>::from_ints(flat_targets.as_slice(), device).reshape([batch_size]);
    let target_scores = Tensor::<B, 1>::from_floats(flat_scores.as_slice(), device)
        .reshape([batch_size, 3]);

    ProbeBatch {
        inputs,
        target_protocols,
        target_scores,
    }
}

/// Synthesize realistic training batches for initial cold-start bootstrapping.
pub fn generate_synthetic_probe_batch(batch_size: usize) -> Vec<ProbeItem> {
    let mut rng = rand::thread_rng();
    let mut items = Vec::with_capacity(batch_size);

    for _ in 0..batch_size {
        let has_llms_full = rng.gen_bool(0.12);
        let has_llms_txt = rng.gen_bool(0.20);
        let has_ai_catalog = rng.gen_bool(0.08);
        let has_sitemap = rng.gen_bool(0.65);
        let has_robots = rng.gen_bool(0.80);
        let has_openapi = rng.gen_bool(0.15);
        let has_rss = rng.gen_bool(0.25);
        let has_json_ld = rng.gen_bool(0.40);
        let has_open_graph = rng.gen_bool(0.60);
        let is_doc = rng.gen_bool(0.35);
        let requires_js = rng.gen_bool(0.30);
        let bot_challenge = rng.gen_bool(0.10);

        let target_protocol = if bot_challenge {
            4 // DropOrBypass
        } else if has_llms_full || has_llms_txt || has_ai_catalog {
            0 // ManifestDirect
        } else if has_sitemap || has_openapi || has_json_ld {
            1 // StructuredMetadata
        } else if !requires_js || is_doc {
            2 // StaticFast
        } else {
            3 // CdpDynamic
        };

        let quality_score: f32 = match target_protocol {
            0 => 0.98,
            1 => 0.88,
            2 => 0.75,
            3 => 0.60,
            _ => 0.10,
        };

        let density: f32 = if is_doc || has_llms_full { 0.90 } else { 0.50 };
        let early_term: f32 = if target_protocol <= 1 { 0.95 } else { 0.10 };

        items.push(ProbeItem {
            features: [
                if has_llms_full { 1.0 } else { 0.0 },
                if has_llms_txt { 1.0 } else { 0.0 },
                if has_ai_catalog { 1.0 } else { 0.0 },
                if has_sitemap { 1.0 } else { 0.0 },
                if has_robots { 1.0 } else { 0.0 },
                if has_openapi { 1.0 } else { 0.0 },
                if has_rss { 1.0 } else { 0.0 },
                if has_json_ld { 1.0 } else { 0.0 },
                if has_open_graph { 1.0 } else { 0.0 },
                if is_doc { 1.0 } else { 0.0 },
                if requires_js { 1.0 } else { 0.0 },
                if bot_challenge { 1.0 } else { 0.0 },
            ],
            target_protocol,
            target_scores: [quality_score, density, early_term],
        });
    }

    items
}
