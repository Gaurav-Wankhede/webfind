// webfind-trainer: Stage 1 Jev System-1 Neural Routing Classifier.
// Pure-Rust architecture built on Burn with multi-head output:
// 1. 5-class Protocol Routing Logits (ManifestDirect, StructuredMetadata, StaticFast, CdpDynamic, DropOrBypass)
// 2. Continuous Score Heads: Saliency/Quality, Structural Density, Early Terminate

use burn::config::Config;
use burn::module::Module;
use burn::nn::{Gelu, Linear, LinearConfig};
use burn::tensor::backend::Backend;
use burn::tensor::Tensor;

/// Jev System-1 Model Configuration.
#[derive(Config, Debug)]
pub struct JevModelConfig {
    /// Number of input features (default: 12 features across the 5 metadata tiers).
    #[config(default = 12)]
    pub input_dim: usize,
    /// Hidden dimension for dense feature projection.
    #[config(default = 64)]
    pub hidden_dim: usize,
}

/// Multi-head predictions emitted by Jev Model forward pass.
pub struct JevPredictions<B: Backend> {
    /// Logits over the 5 RouteProtocol classes:
    /// 0: ManifestDirect, 1: StructuredMetadata, 2: StaticFast, 3: CdpDynamic, 4: DropOrBypass.
    pub protocol_logits: Tensor<B, 2>,
    /// Saliency and quality score in [0.0, 1.0].
    pub quality: Tensor<B, 2>,
    /// Structural asset density prior in [0.0, 1.0].
    pub density: Tensor<B, 2>,
    /// Terminate gate probability in [0.0, 1.0].
    pub terminate: Tensor<B, 2>,
}

#[derive(Module, Debug)]
pub struct JevModel<B: Backend> {
    linear1: Linear<B>,
    linear2: Linear<B>,
    gelu: Gelu,
    protocol_head: Linear<B>,
    scores_head: Linear<B>,
}

impl JevModelConfig {
    /// Initialize the Jev neural model on the specified backend device.
    pub fn init<B: Backend>(&self, device: &B::Device) -> JevModel<B> {
        let linear1 = LinearConfig::new(self.input_dim, self.hidden_dim).init(device);
        let linear2 = LinearConfig::new(self.hidden_dim, self.hidden_dim).init(device);
        let gelu = Gelu::new();
        // 5 classes: ManifestDirect, StructuredMetadata, StaticFast, CdpDynamic, DropOrBypass
        let protocol_head = LinearConfig::new(self.hidden_dim, 5).init(device);
        // 3 scalar outputs: quality, density, terminate gate
        let scores_head = LinearConfig::new(self.hidden_dim, 3).init(device);

        JevModel {
            linear1,
            linear2,
            gelu,
            protocol_head,
            scores_head,
        }
    }
}

impl<B: Backend> JevModel<B> {
    /// Forward pass: transforms [batch_size, 12] feature tensor into multi-head predictions.
    pub fn forward(&self, input: Tensor<B, 2>) -> JevPredictions<B> {
        let x = self.linear1.forward(input);
        let x = self.gelu.forward(x);
        let x = self.linear2.forward(x);
        let x = self.gelu.forward(x);

        let protocol_logits = self.protocol_head.forward(x.clone());
        let scores = self.scores_head.forward(x);

        // Slice multi-task score predictions: 0 = quality, 1 = density, 2 = terminate
        let quality = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 0..1]));
        let density = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 1..2]));
        let terminate = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 2..3]));

        JevPredictions {
            protocol_logits,
            quality,
            density,
            terminate,
        }
    }
}
