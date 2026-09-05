//! Pure-CLI integration test: drives the actual `webfind` binary's `research`
//! command the way a local model harness would (single call, JSON to file,
//! records persisted to the Turso graph store). No MCP, no Python.
//!
//! Requires live internet + a Chromium-capable build; ignored by default.
//! Run: cargo test --test cli_research_persist_test -- --nocapture --ignored

use std::process::Command;
use std::time::Duration;

const BIN: &str = env!("CARGO_BIN_EXE_webfind");

/// Run `webfind research` writing JSON to `output`, return (exit, stderr, stdout).
fn run_research(query: &str, output: &str) -> (i32, String, String) {
    let output_dir = std::env::temp_dir();
    let out_path = output_dir.join(output);
    let _ = std::fs::remove_file(&out_path);

    let mut cmd = Command::new(BIN);
    cmd.arg("research")
        .arg(query)
        .arg("--max-pages")
        .arg("5")
        .arg("--delay")
        .arg("300")
        .arg("--output")
        .arg(&out_path);

    // Give the crawl time; deep/dynamic off for a fast deterministic-ish run.
    let out = cmd.output().expect("run webfind research");
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    let code = out.status.code().unwrap_or(-1);
    (code, stdout, stderr)
}

/// A single CLI research call must (a) write a valid JSON file, (b) leave
/// stdout empty, (c) report persistence in stderr. Live-internet test.
#[test]
#[ignore = "requires live internet"]
fn cli_research_writes_json_and_persists_to_graph() {
    let (code, stdout, stderr) =
        run_research("postgresql vs mysql database 2026", "webfind_cli_test.json");
    assert_eq!(code, 0, "research failed, stderr: {stderr}");

    // stdout must be empty when --output is used (pure file flow).
    assert!(
        stdout.trim().is_empty(),
        "stdout should be empty with --output, got: {stdout:?}"
    );
    // stderr must show persistence into the graph store.
    assert!(
        stderr.contains("Graph store: turso") || stderr.contains("Graph store:"),
        "expected graph-store line in stderr: {stderr}"
    );
    assert!(
        stderr.contains("persist"),
        "expected persistence message in stderr: {stderr}"
    );

    // The JSON file must exist and parse.
    let out_path = std::env::temp_dir().join("webfind_cli_test.json");
    assert!(out_path.exists(), "output file not written");
    let contents = std::fs::read_to_string(&out_path).expect("read output file");
    let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid JSON");
    assert!(
        parsed.get("total_results").is_some(),
        "JSON missing total_results: {contents}"
    );
    let _ = Duration::from_secs(1);
}
