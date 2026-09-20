// webfind-trainer: Stage 1 Jev System-1 Neural Routing Classifier.
// Pure-Rust architecture built on Burn with multi-head output:
// 1. Route Protocol classification (4-class: ManifestDirect, StaticFast, CdpDynamic, DropOrBypass)
// 2. Saliency score regression [0.0, 1.0] (Sigmoid)
// 3. Quality score regression [0.0, 1.0] (Sigmoid)
// 4. Terminate gate prediction [0.0, 1.0] (Sigmoid)

use burn::config::Config;
use burn::module::Module;
use burn::nn::{Gelu, Linear, LinearConfig};
use burn::tensor::backend::Backend;
use burn::tensor::Tensor;

/// Jev System-1 Model Configuration.
#[derive(Config, Debug)]
pub struct JevModelConfig {
    /// Number of input features (default: 8 features -> 3 manifest booleans, 5 domain/header indicators).
    #[config(default = 8)]
    pub input_dim: usize,
    /// Hidden dimension for dense feature projection.
    #[config(default = 64)]
    pub hidden_dim: usize,
}

/// Jev System-1 Neural Network Model.
#[derive(Module, Debug)]
pub struct JevModel<B: Backend> {
    linear1: Linear<B>,
    linear2: Linear<B>,
    gelu: Gelu,
    protocol_head: Linear<B>,
    scores_head: Linear<B>,
}

/// Multi-head predictions emitted by Jev Model forward pass.
pub struct JevPredictions<B: Backend> {
    /// Logits over the 4 RouteProtocol classes (ManifestDirect=0, StaticFast=1, CdpDynamic=2, DropOrBypass=3).
    pub protocol_logits: Tensor<B, 2>,
    /// Saliency score in [0.0, 1.0].
    pub saliency: Tensor<B, 2>,
    /// Content quality score in [0.0, 1.0].
    pub quality: Tensor<B, 2>,
    /// Terminate gate probability in [0.0, 1.0].
    pub terminate: Tensor<B, 2>,
}

impl JevModelConfig {
    /// Initialize the Jev neural model on the specified backend device.
    pub fn init<B: Backend>(&self, device: &B::Device) -> JevModel<B> {
        let linear1 = LinearConfig::new(self.input_dim, self.hidden_dim).init(device);
        let linear2 = LinearConfig::new(self.hidden_dim, self.hidden_dim).init(device);
        let gelu = Gelu::new();
        // 4 classes: ManifestDirect, StaticFast, CdpDynamic, DropOrBypass
        let protocol_head = LinearConfig::new(self.hidden_dim, 4).init(device);
        // 3 scalar outputs: saliency, quality, terminate gate
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
    /// Forward pass: transforms [batch_size, input_dim] feature tensor into multi-head predictions.
    pub fn forward(&self, input: Tensor<B, 2>) -> JevPredictions<B> {
        let x = self.linear1.forward(input);
        let x = self.gelu.forward(x);
        let x = self.linear2.forward(x);
        let x = self.gelu.forward(x);

        let protocol_logits = self.protocol_head.forward(x.clone());
        let scores = self.scores_head.forward(x);

        // Slice multi-task score predictions: index 0 = saliency, 1 = quality, 2 = terminate
        let saliency = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 0..1]));
        let quality = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 1..2]));
        let terminate = burn::tensor::activation::sigmoid(scores.clone().slice([0..scores.dims()[0], 2..3]));

        JevPredictions {
            protocol_logits,
            saliency,
            quality,
            terminate,
        }
    }
}
