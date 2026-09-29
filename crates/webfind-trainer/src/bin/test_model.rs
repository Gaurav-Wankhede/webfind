// webfind-trainer: Standalone Model Inference and Diagnostic Tester.
// Loads the exported models/jev_router.mpk weights into a Burn neural network
// and executes sub-millisecond protocol classification on arbitrary URLs or feature vectors.

use anyhow::Result;
use burn::module::Module;
use burn_wgpu::{Wgpu, WgpuDevice};
use reqwest::Client;
use std::env;
use std::time::Instant;
use url::Url;
use webfind_models::RouteProtocol;
use webfind_trainer::model::JevModelConfig;

async fn probe_live_features(client: &Client, target_url: &str) -> [f32; 12] {
    let base_url = if let Ok(parsed) = Url::parse(target_url) {
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or_default();
        format!("{scheme}://{host}")
    } else {
        target_url.to_string()
    };

    let mut features = [0.0f32; 12];

    // Probe 1: llms.txt and llms-full.txt
    let llms_txt_url = format!("{base_url}/llms.txt");
    if let Ok(resp) = client.head(&llms_txt_url).send().await
        && resp.status().is_success()
    {
        features[1] = 1.0;
        let full_url = format!("{base_url}/llms-full.txt");
        if let Ok(full_resp) = client.head(&full_url).send().await
            && full_resp.status().is_success()
        {
            features[0] = 1.0;
        }
    }

    // Probe 2: openapi.json and sitemap.xml
    let openapi_url = format!("{base_url}/openapi.json");
    if let Ok(resp) = client.head(&openapi_url).send().await
        && resp.status().is_success()
    {
        features[5] = 1.0;
    }

    let sitemap_url = format!("{base_url}/sitemap.xml");
    if let Ok(resp) = client.head(&sitemap_url).send().await
        && resp.status().is_success()
    {
        features[3] = 1.0;
    }

    // Domain heuristics
    let lower = target_url.to_lowercase();
    if lower.contains("docs.") || lower.contains("/docs/") || lower.contains("/api/") {
        features[9] = 1.0;
    }

    features
}

fn softmax(logits: &[f32]) -> Vec<f32> {
    let max = logits.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|&x| x / sum).collect()
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    let target = if args.len() > 1 {
        args[1].clone()
    } else {
        "https://docs.astral.sh/uv/".to_string()
    };

    println!("========================================================");
    println!("WebFind Stage 1 Jev Neural Model Tester");
    println!("========================================================");

    let device = WgpuDevice::DefaultDevice;
    type MyBackend = Wgpu;

    // 1. Load exported model weights from disk
    let model_path = "models/jev_router";
    let full_path = format!("{model_path}.mpk");
    println!("Loading exported model weights from: {full_path}");
    let start_load = Instant::now();

    let recorder = burn::record::NamedMpkFileRecorder::<burn::record::FullPrecisionSettings>::default();
    let config = JevModelConfig::new();
    let uninit_model = config.init::<MyBackend>(&device);
    let model = uninit_model
        .load_file(model_path, &recorder, &device)
        .map_err(|e| anyhow::anyhow!("Failed to load {full_path}: {e:?}"))?;

    println!("Model loaded successfully in {:.2}ms", start_load.elapsed().as_secs_f64() * 1000.0);

    // 2. Extract or resolve 12-dimensional features
    let client = Client::builder()
        .timeout(std::time::Duration::from_millis(1500))
        .build()?;

    println!("Target URL: {target}");
    let probe_start = Instant::now();
    let features = probe_live_features(&client, &target).await;
    let probe_dur = probe_start.elapsed();
    println!("Speculative probe completed in {:.2}ms", probe_dur.as_secs_f64() * 1000.0);
    println!("Feature Vector: {:?}", features);

    // 3. Execute Neural Inference via loaded Burn Model
    let eval_start = Instant::now();
    let input_tensor = burn::tensor::Tensor::<MyBackend, 1>::from_floats(
        features.as_slice(),
        &device,
    )
    .reshape([1, 12]);

    let preds = model.forward(input_tensor);
    let logits_data = preds.protocol_logits.into_data();
    let logits = logits_data.as_slice::<f32>().unwrap();
    let probs = softmax(logits);

    let quality: f32 = preds.quality.into_data().as_slice::<f32>().unwrap()[0];
    let density: f32 = preds.density.into_data().as_slice::<f32>().unwrap()[0];
    let terminate: f32 = preds.terminate.into_data().as_slice::<f32>().unwrap()[0];
    let eval_dur = eval_start.elapsed();

    // 4. Map best class
    let mut best_idx = 0;
    let mut best_prob = probs[0];
    for (idx, &p) in probs.iter().enumerate().skip(1) {
        if p > best_prob {
            best_prob = p;
            best_idx = idx;
        }
    }

    let protocol = match best_idx {
        0 => RouteProtocol::ManifestDirect,
        1 => RouteProtocol::StructuredMetadata,
        2 => RouteProtocol::StaticFast,
        3 => RouteProtocol::CdpDynamic,
        _ => RouteProtocol::DropOrBypass,
    };

    println!("--------------------------------------------------------");
    println!("Inference Execution Latency: {:.2}µs ({:?})", eval_dur.as_micros() as f64, eval_dur);
    println!("Selected Transport Protocol: {:?} (Confidence: {:.2}%)", protocol, best_prob * 100.0);
    println!("Softmax Probabilities Across 5 Protocols:");
    println!("  [0] ManifestDirect:     {:.2}%", probs[0] * 100.0);
    println!("  [1] StructuredMetadata: {:.2}%", probs[1] * 100.0);
    println!("  [2] StaticFast:         {:.2}%", probs[2] * 100.0);
    println!("  [3] CdpDynamic:         {:.2}%", probs[3] * 100.0);
    println!("  [4] DropOrBypass:       {:.2}%", probs[4] * 100.0);
    println!("Multi-Head Quality Priors:");
    println!("  Predicted Saliency/Quality: {:.4}", quality);
    println!("  Structural Asset Density:   {:.4}", density);
    println!("  Early Terminate Gate Prob:  {:.4}", terminate);
    println!("========================================================");

    Ok(())
}
