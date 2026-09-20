// webfind-models: RouteProtocol decision enum for Stage 1 Jev routing.
// Classifies the optimal retrieval protocol across the 5 tiers of the metadata extraction pyramid.

use serde::{Deserialize, Serialize};

/// Optimal extraction protocol selected by the Stage 1 routing model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RouteProtocol {
    /// Tier 1: Machine AI Manifest Direct (/llms.txt, /llms-full.txt, /ai-catalog.json).
    /// Zero DOM parsing required (<20ms).
    ManifestDirect = 0,

    /// Tier 2 & 3: Structured Topology or Embedded Head Metadata.
    /// Fast sitemap URL tree or JSON-LD / Open Graph extracted from first 4KB stream (<100ms).
    StructuredMetadata = 1,

    /// Tier 4: Fast Static HTML AST Scraping (curl / reqwest + CSS selector distillation) (<250ms).
    StaticFast = 2,

    /// Tier 5: Dynamic Headless Chromium Execution (CDP / JS hydration for complex SPAs).
    CdpDynamic = 3,

    /// Drop or Bypass: CAPTCHA challenge, paywalled, rate-limited, or low-quality dead link.
    DropOrBypass = 4,
}

impl RouteProtocol {
    /// Returns a human-readable identifier for terminal logging and telemetry.
    #[inline]
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::ManifestDirect => "manifest_direct",
            Self::StructuredMetadata => "structured_metadata",
            Self::StaticFast => "static_fast",
            Self::CdpDynamic => "cdp_dynamic",
            Self::DropOrBypass => "drop_or_bypass",
        }
    }
}

/// Complete routing decision emitted by Stage 1 Jev System-1 classifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JevDecision {
    /// Recommended transport and extraction protocol.
    pub protocol: RouteProtocol,
    /// Information quality prior [0.0, 1.0].
    pub quality_score: f32,
    /// Estimated structural asset density prior [0.0, 1.0].
    pub structural_density: f32,
    /// Early-termination threshold confidence [0.0, 1.0].
    pub terminate_early: bool,
    /// Inference latency in microseconds.
    pub inference_us: u64,
}
