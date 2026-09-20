use crate::dataset::{ProbeBatch, ProbeItem};
use crate::model::{JevModel, JevModelConfig};
use burn::nn::loss::CrossEntropyLossConfig;
use burn::optim::decay::WeightDecayConfig;
use burn::optim::{AdamConfig, Optimizer};
use burn::tensor::backend::AutodiffBackend;
use burn::tensor::{ElementConversion, Tensor};

/// Combined multi-task training step for Jev.
pub fn train_step<B: AutodiffBackend>(
    model: JevModel<B>,
    batch: ProbeBatch<B>,
    device: &B::Device,
) -> (Tensor<B, 1>, JevModel<B>) {
    let preds = model.forward(batch.inputs);

    // 1. Classification Loss (Cross Entropy) on 4 RouteProtocol classes
    let ce_loss = CrossEntropyLossConfig::new()
        .init(device)
        .forward(preds.protocol_logits, batch.target_protocols);

    // 2. Score Regression Loss (Mean Squared Error on Saliency, Quality, Terminate)
    let pred_scores = Tensor::cat(vec![preds.saliency, preds.quality, preds.terminate], 1);
    let mse_loss = (pred_scores - batch.target_scores).powf_scalar(2.0).mean();

    // Total Loss = CrossEntropy + 0.5 * MSE
    let total_loss = ce_loss + mse_loss.mul_scalar(0.5);

    (total_loss, model)
}

/// Execute a complete local training session on the given backend.
pub fn train_jev_model<B: AutodiffBackend>(
    items: Vec<ProbeItem>,
    device: &B::Device,
    epochs: usize,
    batch_size: usize,
    lr: f64,
) -> JevModel<B> {
    let config = JevModelConfig::new();
    let mut model = config.init::<B>(device);
    let mut optim = AdamConfig::new()
        .with_weight_decay(Some(WeightDecayConfig::new(1e-4)))
        .init();

    let num_batches = items.len().div_ceil(batch_size);

    println!("Starting Stage 1 Jev Training on Apple Silicon M4 / Metal Backend:");
    println!("  Dataset Size: {} probe items", items.len());
    println!("  Batch Size:   {}", batch_size);
    println!("  Epochs:       {}", epochs);
    println!("  Batches/Ep:   {}", num_batches);

    for epoch in 1..=epochs {
        let mut total_epoch_loss = 0.0f32;

        for chunk in items.chunks(batch_size) {
            let batch = crate::dataset::batch_items::<B>(chunk, device);
            let (loss, updated_model) = train_step(model, batch, device);
            model = updated_model;

            let loss_val: f32 = loss.clone().into_scalar().elem();
            total_epoch_loss += loss_val;

            let grads = loss.backward();
            let grads = burn::optim::GradientsParams::from_grads(grads, &model);
            model = optim.step(lr, model, grads);
        }

        let avg_loss = total_epoch_loss / num_batches as f32;
        println!("  Epoch [{:>2}/{}]: Avg Loss = {:.5}", epoch, epochs, avg_loss);
    }

    println!("Stage 1 Jev Model Training Complete (Pure Rust / Metal).");
    model
}
