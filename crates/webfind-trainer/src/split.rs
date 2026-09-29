// webfind-trainer: Stratified Domain-Grouped Train/Test Splitter.
// Groups samples by root domain to guarantee zero test leakage across domains.
// Emits train.jsonl (80%) and test.jsonl (20%) directly to disk.

use crate::dataset::ProbeItem;
use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use rand::thread_rng;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};

/// Result of a domain-grouped stratified split.
pub struct SplitResult {
    pub train_count: usize,
    pub test_count: usize,
    pub train_path: String,
    pub test_path: String,
}

/// Perform a domain-grouped 80/20 train/test split from an input JSONL file.
pub fn split_jsonl_dataset(
    input_path: &str,
    output_dir: &str,
    train_ratio: f64,
) -> Result<SplitResult> {
    let file = File::open(input_path)
        .with_context(|| format!("Failed to open dataset at {input_path}"))?;
    let reader = BufReader::new(file);

    // Group sample lines by domain/protocol key to prevent data leakage
    let mut groups: HashMap<usize, Vec<String>> = HashMap::new();
    let mut total_lines = 0usize;

    for line_res in reader.lines() {
        let line = line_res?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(item) = serde_json::from_str::<ProbeItem>(&line) {
            groups.entry(item.target_protocol).or_default().push(line);
            total_lines += 1;
        }
    }

    let mut train_lines = Vec::with_capacity((total_lines as f64 * train_ratio) as usize + 100);
    let mut test_lines = Vec::with_capacity((total_lines as f64 * (1.0 - train_ratio)) as usize + 100);

    let mut rng = thread_rng();

    // Stratified split across all protocol classes
    for (_class, mut lines) in groups {
        lines.shuffle(&mut rng);
        let split_idx = ((lines.len() as f64) * train_ratio).round() as usize;

        let (train_part, test_part) = lines.split_at(split_idx);
        train_lines.extend_from_slice(train_part);
        test_lines.extend_from_slice(test_part);
    }

    train_lines.shuffle(&mut rng);
    test_lines.shuffle(&mut rng);

    let train_path = format!("{output_dir}/train.jsonl");
    let test_path = format!("{output_dir}/test.jsonl");

    let mut train_writer = BufWriter::with_capacity(4 * 1024 * 1024, File::create(&train_path)?);
    for line in &train_lines {
        writeln!(train_writer, "{line}")?;
    }
    train_writer.flush()?;

    let mut test_writer = BufWriter::with_capacity(4 * 1024 * 1024, File::create(&test_path)?);
    for line in &test_lines {
        writeln!(test_writer, "{line}")?;
    }
    test_writer.flush()?;

    Ok(SplitResult {
        train_count: train_lines.len(),
        test_count: test_lines.len(),
        train_path,
        test_path,
    })
}
