// webfind-trainer: Open-access BEIR grounding streamer.
// Converts passage benchmarks into 12-dimensional training items.

use crate::dataset::ProbeItem;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::io::Read;
use std::time::Duration;

#[derive(Debug, Deserialize)]
pub struct BeirRecord {
    #[serde(rename = "_id")]
    pub id: String,
    pub title: String,
    pub text: String,
}

/// Download and stream a designated BEIR benchmark dataset.
pub async fn download_beir_dataset(name: &str, limit: usize) -> Result<Vec<BeirRecord>> {
    let url = format!("https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/{name}.zip");
    let client = Client::builder()
        .timeout(Duration::from_secs(45))
        .build()?;

    let resp = client.get(&url).send().await.context("Failed to download BEIR dataset zip")?;
    if !resp.status().is_success() {
        anyhow::bail!("BEIR mirror returned status {}", resp.status());
    }

    let bytes = resp.bytes().await?;
    let reader = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(reader)?;

    let corpus_entry_name = format!("{name}/corpus.jsonl");
    let mut corpus_file = zip
        .by_name(&corpus_entry_name)
        .with_context(|| format!("Missing {corpus_entry_name} in BEIR archive"))?;

    let mut content = String::new();
    corpus_file.read_to_string(&mut content)?;

    let mut records = Vec::new();
    for line in content.lines().take(limit) {
        if let Ok(rec) = serde_json::from_str::<BeirRecord>(line) {
            records.push(rec);
        }
    }

    Ok(records)
}

/// Map BEIR text records to 12-dimensional probe items.
pub fn beir_to_probe_items(records: &[BeirRecord]) -> Vec<ProbeItem> {
    records
        .iter()
        .map(|r| {
            let has_code = r.text.contains("fn ") || r.text.contains("def ") || r.text.contains("class ");
            let has_table = r.text.contains('|');
            let is_dense = r.text.len() > 800;

            ProbeItem {
                features: [
                    0.0, // has_llms_full
                    0.0, // has_llms_txt
                    0.0, // has_ai_catalog
                    1.0, // has_sitemap
                    1.0, // has_robots
                    0.0, // has_openapi
                    0.0, // has_rss
                    1.0, // has_json_ld (academic papers often have scholarly JSON-LD)
                    0.0, // has_open_graph
                    1.0, // is_doc
                    0.0, // requires_js
                    0.0, // bot_challenge
                ],
                target_protocol: 1, // StructuredMetadata
                target_scores: [
                    if is_dense { 0.90 } else { 0.75 },
                    if has_code || has_table { 0.85 } else { 0.40 },
                    0.80,
                ],
            }
        })
        .collect()
}
