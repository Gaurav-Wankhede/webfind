// webfind-trainer: Source 2 - Open-Access Grounding Corpora Downloader & Streamer.
// Direct verified download and parsing of BEIR academic benchmarks (e.g. SciFact, FiQA):
// Canonical Primary Source: https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/

use crate::dataset::ProbeItem;
use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Cursor};
use std::time::Duration;
use zip::ZipArchive;

/// BEIR standard corpus record representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BeirCorpusRecord {
    pub _id: String,
    pub title: String,
    pub text: String,
}

/// Download and extract a real BEIR dataset directly into memory in pure Rust.
pub async fn download_beir_dataset(
    dataset_name: &str,
    limit: usize,
) -> Result<Vec<BeirCorpusRecord>> {
    let url = format!(
        "https://public.ukp.informatik.tu-darmstadt.de/thakur/BEIR/datasets/{dataset_name}.zip"
    );

    println!("Downloading open-access benchmark: {url}...");
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;

    let resp = client.get(&url).send().await
        .with_context(|| format!("Failed to download BEIR dataset from {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("Server returned HTTP error {}", resp.status());
    }

    let bytes = resp.bytes().await?;
    println!("  Downloaded {:.2} MB. Extracting corpus.jsonl...", bytes.len() as f64 / 1_048_576.0);

    let reader = Cursor::new(bytes);
    let mut archive = ZipArchive::new(reader)
        .context("Failed to read zip archive")?;

    let mut corpus_records = Vec::with_capacity(limit);

    // Look for corpus.jsonl inside the archive
    let corpus_filename = format!("{dataset_name}/corpus.jsonl");
    let mut found = false;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let name = file.name().to_string();

        if name == corpus_filename || name.ends_with("/corpus.jsonl") || name == "corpus.jsonl" {
            found = true;
            let buf = BufReader::new(&mut file);

            for line in buf.lines().take(limit) {
                let line_str = line?;
                if let Ok(rec) = serde_json::from_str::<BeirCorpusRecord>(&line_str) {
                    corpus_records.push(rec);
                }
            }
            break;
        }
    }

    if !found {
        anyhow::bail!("corpus.jsonl not found inside {dataset_name}.zip archive");
    }

    println!("  Extracted {} authoritative benchmark passages from {dataset_name}.", corpus_records.len());
    Ok(corpus_records)
}

/// Convert real BEIR corpus records into high-quality ProbeItems for model training.
pub fn beir_to_probe_items(records: &[BeirCorpusRecord]) -> Vec<ProbeItem> {
    records
        .iter()
        .map(|r| {
            let word_count = r.text.split_whitespace().count();
            let is_substantial = word_count > 40;

            let features = [
                0.0, // open academic corpus has no /llms-full.txt
                0.0, // no /llms.txt
                0.0, // no /ai-catalog.json
                1.0, // academic passage treated as reference documentation
                0.0, // clean raw text requires zero JS
                0.0, // no bot challenge
                0.05, // zero latency
                if is_substantial { 0.95 } else { 0.80 },
            ];

            ProbeItem {
                features,
                target_protocol: 1, // StaticFast
                target_scores: [
                    if is_substantial { 0.92 } else { 0.75 },
                    if is_substantial { 0.95 } else { 0.80 },
                    0.0,
                ],
            }
        })
        .collect()
}
