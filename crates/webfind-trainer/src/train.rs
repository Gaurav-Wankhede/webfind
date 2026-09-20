use crate::dataset::{ProbeBatch, ProbeItem};
use crate::model::{JevModel, JevModelConfig};
use burn::nn::loss::CrossEntropyLossConfig;
use burn::optim::decay::WeightDecayConfig;
use burn::optim::{AdamConfig, Optimizer};
use burn::tensor::backend::AutodiffBackend;
use burn::tensor::Tensor;

/// Combined multi-task training step for Jev across 5 protocol classes.
pub fn train_step<B: AutodiffBackend>(
    model: JevModel<B>,
    batch: ProbeBatch<B>,
    device: &B::Device,
) -> (Tensor<B, 1>, JevModel<B>) {
    let preds = model.forward(batch.inputs);

    // 1. Classification Loss (Cross Entropy) on 5 RouteProtocol classes
    let ce_loss = CrossEntropyLossConfig::new()
        .init(device)
        .forward(preds.protocol_logits, batch.target_protocols);

    // 2. Score Regression Loss (Mean Squared Error on Quality, Density, Terminate)
    let pred_scores = Tensor::cat(vec![preds.quality, preds.density, preds.terminate], 1);
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
    let mut model = config.init(device);

    let mut optim = AdamConfig::new()
        .with_weight_decay(WeightDecayConfig::new(1e-4).into())
        .init();

    let num_items = items.len();
    if num_items == 0 {
        return model;
    }

    println!("Starting Stage 1 Jev Training (12 Features -> 5 Protocols):");
    println!("  Samples:     {num_items}");
    println!("  Epochs:      {epochs}");
    println!("  Batch Size:  {batch_size}");
    println!("  Learning Rate: {lr}");

    for epoch in 1..=epochs {
        let mut total_epoch_loss = 0.0;
        let mut num_batches = 0;

        for chunk in items.chunks(batch_size) {
            let batch = crate::dataset::batch_items::<B>(chunk, device);
            let (loss, updated_model) = train_step(model, batch, device);

            let loss_val: f32 = loss.clone().into_data().as_slice::<f32>().unwrap()[0];
            total_epoch_loss += loss_val;
            num_batches += 1;

            let grads = loss.backward();
            let grads = burn::optim::GradientsParams::from_grads(grads, &updated_model);
            model = optim.step(lr, updated_model, grads);
        }

        let avg_loss = if num_batches > 0 {
            total_epoch_loss / num_batches as f32
        } else {
            0.0
        };

        if epoch == 1 || epoch % 5 == 0 || epoch == epochs {
            println!("  [Epoch {epoch:2}/{epochs}] Average Multi-Task Loss: {avg_loss:.5}");
        }
    }

    println!("Stage 1 Jev Model Training Complete.\n");
    model
}
