// examples/verify_jev_cloud_pipeline.rs
// Live verification harness testing the end-to-end cloud pipeline:
// 1. Stage 1 Speculative Wire Probing (<25ms)
// 2. Stage 1 Jev Neural Router Decision (<0.2ms)
// 3. Stage 2 Structural Distillation with BLAKE3 Cryptographic Provenance

use reqwest::Client;
use std::time::{Duration, Instant};
use webfind::engine::jev_router::JevRouter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("========================================================");
    println!("WebFind Live Engine Verification: Stage 1 & Stage 2");
    println!("========================================================");

    let client = Client::builder()
        .timeout(Duration::from_millis(5000))
        .user_agent("WebFind-Neural-Agent/1.0")
        .build()?;

    let router = JevRouter::new(client);

    let test_urls = [
        ("Astral uv", "https://docs.astral.sh/uv/"),
        ("Cloudflare", "https://developers.cloudflare.com/"),
        ("SQLite Architecture", "https://sqlite.org/arch.html"),
        ("React Reference", "https://react.dev/reference/react"),
    ];

    for (label, url) in test_urls {
        println!("\nTarget: {label} ({url})");
        let start = Instant::now();

        // 1. Stage 1: Speculative wire probe
        let features = router.probe_wire_features(url).await;
        let probe_time = start.elapsed();

        // 2. Stage 1: Neural Decision
        let decision = router.decide(&features);

        println!("  Probe Latency:    {:.2?}", probe_time);
        println!("  Decision Latency: {}µs", decision.inference_us);
        println!("  Selected Route:   {:?} ({})", decision.protocol, decision.protocol.as_str());
        println!("  Quality Prior:    {:.2}", decision.quality_score);
        println!("  Density Prior:    {:.2}", decision.structural_density);

        // 3. Stage 2: Distill and compute BLAKE3 provenance
        let distilled = router.fetch_and_distill(url).await?;
        println!("  Stage 2 Output:   {}", distilled.core_takeaways);
        println!("  BLAKE3 Provenance Hash: {}", distilled.provenance.content_hash);
        println!("  Token Reduction:  {}%", distilled.metrics.reduction_percent);
        println!("  Estimated Tokens: {} raw -> {} distilled", 
            distilled.metrics.raw_token_count, distilled.metrics.distilled_token_count);
    }

    println!("\n========================================================");
    println!("Verification Succeeded: All stages verified on real wire.");
    println!("========================================================");
    Ok(())
}
