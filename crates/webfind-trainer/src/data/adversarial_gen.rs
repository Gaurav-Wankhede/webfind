// webfind-trainer: Adversarial perturbation engine for Stage 1 Jev routing.
// Injects realistic edge cases across the 12-dimensional feature space:
// - Masqueraded bot challenge boundaries
// - Spoofed manifest presence
// - Inconsistent sitemap and robots directives

use crate::dataset::ProbeItem;
use rand::Rng;

pub struct AdversarialPerturber;

impl AdversarialPerturber {
    /// Perturbs a ground-truth probe sample into a hard edge-case sample.
    pub fn perturb(base: &ProbeItem) -> ProbeItem {
        let mut rng = rand::thread_rng();
        let mut perturbed_features = base.features;
        let strategy = rng.gen_range(0..4);

        let (target_protocol, target_scores) = match strategy {
            0 => {
                // Strategy 1: Masqueraded CAPTCHA / WAF on an otherwise valid doc site
                perturbed_features[11] = 1.0; // bot_challenge = true
                (4, [0.05, base.target_scores[1], 0.99]) // DropOrBypass, quality collapses, terminate
            }
            1 => {
                // Strategy 2: Sitemap exists but site is a complex dynamic SPA requiring Chromium
                perturbed_features[3] = 1.0;  // has_sitemap = true
                perturbed_features[10] = 1.0; // requires_js = true
                (3, [base.target_scores[0], base.target_scores[1], 0.10]) // CdpDynamic
            }
            2 => {
                // Strategy 3: Machine OpenAPI spec discovered on root
                perturbed_features[5] = 1.0;  // has_openapi = true
                (1, [0.95, 0.90, base.target_scores[2]]) // StructuredMetadata, high quality API contract
            }
            _ => {
                // Strategy 4: High-quality JSON-LD embedded article
                perturbed_features[7] = 1.0;  // has_json_ld = true
                perturbed_features[8] = 1.0;  // has_open_graph = true
                (1, [0.88, base.target_scores[1], base.target_scores[2]]) // StructuredMetadata
            }
        };

        ProbeItem {
            features: perturbed_features,
            target_protocol,
            target_scores,
        }
    }
}
