// webfind-trainer: Source 4 - Synthetic Adversarial Perturbations Engine.
// Injects adversarial edge cases into training samples to prevent model degradation:
// - Keyword stuffing (high entropy noise in shallow DOMs)
// - Bot challenge masquerading (403/429 disguised as normal text)
// - Ambiguous coreferences and caveat inversions

use crate::dataset::ProbeItem;
use rand::Rng;

/// Adversarial perturbation transformer.
pub struct AdversarialPerturber;

impl AdversarialPerturber {
    /// Perturb a clean item into an adversarial probe item.
    pub fn perturb(item: &ProbeItem) -> ProbeItem {
        let mut rng = rand::thread_rng();
        let mode: f32 = rng.r#gen();
        let mut perturbed_features = item.features;

        if mode < 0.33 {
            // Mode 1: Keyword Stuffing / High Noise
            // Disguised high quality prior on a page with zero actual manifests and high latency
            perturbed_features[0] = 0.0;
            perturbed_features[1] = 0.0;
            perturbed_features[2] = 0.0;
            perturbed_features[6] = rng.gen_range(1.0..3.5); // high latency
            perturbed_features[7] = 0.20; // low real quality

            ProbeItem {
                features: perturbed_features,
                target_protocol: 3, // DropOrBypass
                target_scores: [0.15, 0.10, 1.0], // terminate=1.0
            }
        } else if mode < 0.66 {
            // Mode 2: Hidden Bot Challenge / CAPTCHA Wall
            perturbed_features[4] = 1.0; // requires JS
            perturbed_features[5] = 1.0; // bot challenge
            perturbed_features[7] = 0.10;

            ProbeItem {
                features: perturbed_features,
                target_protocol: 3, // DropOrBypass
                target_scores: [0.05, 0.05, 1.0],
            }
        } else {
            // Mode 3: Dynamic Single-Page App with high quality documentation
            perturbed_features[3] = 1.0; // doc subdomain
            perturbed_features[4] = 1.0; // requires JS
            perturbed_features[5] = 0.0; // no bot challenge
            perturbed_features[7] = 0.85;

            ProbeItem {
                features: perturbed_features,
                target_protocol: 2, // CdpDynamic
                target_scores: [0.82, 0.80, 0.0],
            }
        }
    }
}
