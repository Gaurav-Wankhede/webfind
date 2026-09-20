// webfind-models: Type-safe decision tensors and distilled metadata schemas for WebFind.
// Implements Schema-First structures for Stage 1 Jev routing and Stage 2 metadata distillation.

pub mod decision;
pub mod manifest;
pub mod payload;

pub use decision::{JevDecision, RouteProtocol};
pub use manifest::ManifestTensor;
pub use payload::{
    CalloutAsset, CodeAsset, CompressionMetrics, DiagramAsset, DistilledDocument,
    DocumentProvenance, MathAsset, StructuralAssets, TableAsset,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[test]
    fn test_manifest_tensor_serialization() {
        let tensor = ManifestTensor::new(true, false, true);
        assert!(tensor.has_any_manifest());
        assert_eq!(tensor.to_features(), [1.0, 0.0, 1.0]);

        let json = serde_json::to_string(&tensor).expect("serialize manifest");
        let deserialized: ManifestTensor = serde_json::from_str(&json).expect("deserialize manifest");
        assert_eq!(tensor, deserialized);
    }

    #[test]
    fn test_jev_decision_serialization() {
        let decision = JevDecision::new(RouteProtocol::ManifestDirect, 0.95, 0.88, false);
        assert!(decision.should_fetch());

        let json = serde_json::to_string(&decision).expect("serialize decision");
        let deserialized: JevDecision = serde_json::from_str(&json).expect("deserialize decision");
        assert_eq!(decision, deserialized);
    }

    #[test]
    fn test_distilled_document_roundtrip_with_structural_assets() {
        let doc = DistilledDocument {
            provenance: DocumentProvenance {
                canonical_url: "https://example.com/docs/architecture".into(),
                title: "System Architecture".into(),
                crawled_at: Utc::now(),
                content_hash: "abcdef0123456789".into(),
            },
            core_takeaways: "System uses two-stage Jev pipeline for 98% token compression.".into(),
            structural_assets: StructuralAssets {
                diagrams: vec![DiagramAsset {
                    format: "mermaid".into(),
                    raw: "graph TD; A-->B;".into(),
                    caption: Some("Pipeline Overview".into()),
                }],
                tables: vec![TableAsset {
                    headers: vec!["Component".into(), "Latency".into()],
                    rows: vec![vec!["Jev".into(), "<2ms".into()]],
                    markdown: Some("| Component | Latency |\n|---|---|\n| Jev | <2ms |".into()),
                    caption: None,
                }],
                equations: vec![MathAsset {
                    latex: r"\mathcal{L} = -\sum y \log \hat{y}".into(),
                    is_block: true,
                    label: Some("eq:cross_entropy".into()),
                }],
                code_contracts: vec![CodeAsset {
                    language: "rust".into(),
                    code: "pub fn route() -> RouteProtocol { RouteProtocol::ManifestDirect }".into(),
                    signature: Some("crates/webfind-models/src/lib.rs".into()),
                }],
                callouts: vec![CalloutAsset {
                    severity: "warning".into(),
                    message: "Do not bypass manifest sniffing for AI-enabled domains.".into(),
                }],
            },
            key_insights: vec!["LLM manifests reduce context ingestion latency to near zero.".into()],
            metrics: CompressionMetrics {
                raw_token_count: 40000,
                distilled_token_count: 350,
                reduction_percent: 99,
            },
        };

        let json = serde_json::to_string_pretty(&doc).expect("serialize doc");
        let deserialized: DistilledDocument = serde_json::from_str(&json).expect("deserialize doc");
        assert_eq!(doc.provenance.canonical_url, deserialized.provenance.canonical_url);
        assert_eq!(doc.structural_assets.total_count(), 5);
        assert_eq!(doc.structural_assets.diagrams.len(), 1);
        assert_eq!(doc.structural_assets.tables.len(), 1);
        assert_eq!(doc.structural_assets.equations.len(), 1);
        assert_eq!(doc.structural_assets.code_contracts.len(), 1);
        assert_eq!(doc.structural_assets.callouts.len(), 1);
    }
}
