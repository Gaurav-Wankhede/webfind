// webfind-models: Type-safe decision tensors and distilled metadata schemas.

pub mod decision;
pub mod manifest;
pub mod payload;
pub mod span;

pub use decision::{JevDecision, RouteProtocol};
pub use manifest::ManifestTensor;
pub use payload::{
    CalloutAsset, CodeAsset, CompressionMetrics, DiagramAsset, DistilledDocument,
    DistillationTrainingPair, DocumentProvenance, MathAsset, StructuralAssets, TableAsset,
};
pub use span::TextRange;
