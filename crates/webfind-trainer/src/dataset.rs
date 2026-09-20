// webfind-trainer: Synthetic training dataset generator for Stage 1 Jev routing.
// Generates realistic web telemetry batches grounded in authentic internet probe behaviors.

use burn::tensor::backend::Backend;
use burn::tensor::{Int, Tensor};
use rand::Rng;

/// Input item representing raw web probe metrics for a single domain/URL.
#[derive(Clone, Debug)]
pub struct ProbeItem {
    pub features: [f32; 8],
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
    let mut flat_inputs = Vec::with_capacity(batch_size * 8);
    let mut flat_targets = Vec::with_capacity(batch_size);
    let mut flat_scores = Vec::with_capacity(batch_size * 3);

    for item in items {
        flat_inputs.extend_from_slice(&item.features);
        flat_targets.push(item.target_protocol as i64);
        flat_scores.extend_from_slice(&item.target_scores);
    }

    let inputs = Tensor::<B, 1>::from_floats(flat_inputs.as_slice(), device)
        .reshape([batch_size, 8]);
    let target_protocols = Tensor::<B, 1, Int>::from_ints(flat_targets.as_slice(), device);
    let target_scores = Tensor::<B, 1>::from_floats(flat_scores.as_slice(), device)
        .reshape([batch_size, 3]);

    ProbeBatch {
        inputs,
        target_protocols,
        target_scores,
    }
}

/// Generate a synthetic dataset of `n` items grounded in live web probe patterns.
pub fn generate_synthetic_dataset(n: usize) -> Vec<ProbeItem> {
    let mut rng = rand::thread_rng();
    let mut dataset = Vec::with_capacity(n);

    for _ in 0..n {
        let category: f32 = rng.r#gen();
        if category < 0.25 {
            // Case 1: ManifestDirect (e.g. Astral uv, Cloudflare docs)
            let has_full = rng.gen_bool(0.4);
            let has_txt = true;
            let has_catalog = rng.gen_bool(0.3);
            let features = [
                if has_full { 1.0 } else { 0.0 },
                if has_txt { 1.0 } else { 0.0 },
                if has_catalog { 1.0 } else { 0.0 },
                1.0, // doc subdomain
                0.0, // no JS hydration needed
                0.0, // no bot challenge
                rng.gen_range(0.05..0.2), // fast latency
                rng.gen_range(0.85..0.98), // high quality prior
            ];
            dataset.push(ProbeItem {
                features,
                target_protocol: 0, // ManifestDirect
                target_scores: [rng.gen_range(0.90..0.99), rng.gen_range(0.88..0.98), 0.0],
            });
        } else if category < 0.60 {
            // Case 2: StaticFast (e.g. docs.rs, plain HTML blogs)
            let features = [
                0.0, // no full
                0.0, // no txt
                0.0, // no catalog
                if rng.gen_bool(0.7) { 1.0 } else { 0.0 },
                0.0, // no JS hydration needed
                0.0, // no bot challenge
                rng.gen_range(0.1..0.4),
                rng.gen_range(0.70..0.90),
            ];
            dataset.push(ProbeItem {
                features,
                target_protocol: 1, // StaticFast
                target_scores: [rng.gen_range(0.75..0.90), rng.gen_range(0.70..0.88), 0.0],
            });
        } else if category < 0.85 {
            // Case 3: CdpDynamic (e.g. dynamic SPAs, JS-heavy app dashboards)
            let features = [
                0.0, 0.0, 0.0,
                0.0, // not pure doc
                1.0, // requires JS hydration
                if rng.gen_bool(0.5) { 1.0 } else { 0.0 }, // bot challenge
                rng.gen_range(0.3..0.9),
                rng.gen_range(0.50..0.75),
            ];
            dataset.push(ProbeItem {
                features,
                target_protocol: 2, // CdpDynamic
                target_scores: [rng.gen_range(0.60..0.80), rng.gen_range(0.55..0.75), 0.0],
            });
        } else {
            // Case 4: DropOrBypass (e.g. dead links, hard paywalls, bot 403 blocks)
            let features = [
                0.0, 0.0, 0.0,
                0.0,
                0.0,
                1.0, // severe bot block
                rng.gen_range(0.8..1.5),
                rng.gen_range(0.1..0.3),
            ];
            dataset.push(ProbeItem {
                features,
                target_protocol: 3, // DropOrBypass
                target_scores: [rng.gen_range(0.05..0.25), rng.gen_range(0.05..0.20), 1.0], // terminate=1.0
            });
        }
    }

    dataset
}
