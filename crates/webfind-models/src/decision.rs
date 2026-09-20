// webfind-models: JevDecision representing Stage 1 System-1 classification output.

use serde::{Deserialize, Serialize};

/// Recommended fetch protocol determined by the Jev routing classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteProtocol {
    /// Ingest authoritative markdown directly from `/llms-full.txt` or `/llms.txt`.
    ManifestDirect,
    /// Fast static HTTP fetch (reqwest + readability / html5ever parser).
    StaticFast,
    /// Full CDP Chromium browser execution for JS-heavy or bot-protected sites.
    CdpDynamic,
    /// Abort fetch or drop URL due to bot challenge, paywall, or spam degradation.
    DropOrBypass,
}

/// Jev System-1 classification decision produced in <2ms.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevDecision {
    /// Selected protocol for page acquisition.
    pub protocol: RouteProtocol,
    /// Projected query-context relevance / semantic utility in [0.0, 1.0].
    pub saliency_score: f32,
    /// Predicted content cleanliness and structural density in [0.0, 1.0].
    pub quality_score: f32,
    /// Whether the crawler should terminate further branch discovery from this URL.
    pub terminate_gate: bool,
}

impl JevDecision {
    /// Creates a new JevDecision.
    #[inline]
    #[must_use]
    pub const fn new(
        protocol: RouteProtocol,
        saliency_score: f32,
        quality_score: f32,
        terminate_gate: bool,
    ) -> Self {
        Self {
            protocol,
            saliency_score,
            quality_score,
            terminate_gate,
        }
    }

    /// Helper indicating whether the candidate page should be crawled at all.
    #[inline]
    #[must_use]
    pub const fn should_fetch(&self) -> bool {
        !matches!(self.protocol, RouteProtocol::DropOrBypass) && !self.terminate_gate
    }
}
