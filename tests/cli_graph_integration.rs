use std::path::PathBuf;
use std::process::Command;
use std::sync::Once;

mod common;

static BUILD: Once = Once::new();

fn webfind_binary() -> PathBuf {
    BUILD.call_once(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--bin", "webfind", "--quiet"])
            .status()
            .expect("cargo build should start");
        assert!(status.success(), "cargo build --bin webfind failed");
    });
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/webfind")
}

async fn run_webfind(
    args: Vec<String>,
    env_vars: Vec<(&'static str, String)>,
) -> std::process::Output {
    let binary = webfind_binary();
    tokio::task::spawn_blocking(move || {
        let mut cmd = Command::new(binary);
        cmd.args(args);
        for (k, v) in env_vars {
            cmd.env(k, v);
        }
        cmd.output().expect("webfind command should run")
    })
    .await
    .expect("spawn_blocking should succeed")
}

#[tokio::test]
async fn test_cli_bulk_crawl_persists_to_surrealdb_and_graph_traverses() {
    let (_server, base_url) = common::start_test_server().await;
    let tmp = tempfile::tempdir().unwrap();
    let data_dir = tmp.path();
    let surreal_path = data_dir.join("graph.surrealkv");
    let surreal_url = format!("surrealkv://{}", surreal_path.display());

    // Bulk crawl with SurrealDB graph store.
    let crawl = run_webfind(
        vec![
            "crawl".to_string(),
            "--bulk".to_string(),
            "--graph-store".to_string(),
            "surrealdb".to_string(),
            "--surreal-url".to_string(),
            surreal_url.clone(),
            "--surreal-ns".to_string(),
            "webfind_test".to_string(),
            "--surreal-db".to_string(),
            "webfind_test".to_string(),
            "--seed".to_string(),
            base_url.clone(),
            "--max-pages".to_string(),
            "3".to_string(),
            "--delay".to_string(),
            "50".to_string(),
            "--rps".to_string(),
            "100".to_string(),
        ],
        vec![
            ("WEBFIND_DATA_DIR", data_dir.to_str().unwrap().to_string()),
            ("RUST_LOG", "info".to_string()),
        ],
    )
    .await;
    let crawl_stdout = String::from_utf8_lossy(&crawl.stdout);
    let crawl_stderr = String::from_utf8_lossy(&crawl.stderr);
    println!("CRAWL STDOUT:\n{}", crawl_stdout);
    println!("CRAWL STDERR:\n{}", crawl_stderr);
    assert!(
        crawl.status.success(),
        "crawl failed:\nstdout: {}\nstderr: {}",
        crawl_stdout,
        crawl_stderr
    );
    assert!(
        crawl_stdout.contains("Bulk crawl complete"),
        "crawl did not report completion:\n{}",
        crawl_stdout
    );

    // Graph traversal should discover linked pages.
    let graph = run_webfind(
        vec![
            "graph".to_string(),
            base_url.clone(),
            "--direction".to_string(),
            "outbound".to_string(),
            "--depth".to_string(),
            "2".to_string(),
            "--surreal-url".to_string(),
            surreal_url,
            "--surreal-ns".to_string(),
            "webfind_test".to_string(),
            "--surreal-db".to_string(),
            "webfind_test".to_string(),
        ],
        vec![],
    )
    .await;
    let graph_stdout = String::from_utf8_lossy(&graph.stdout);
    let graph_stderr = String::from_utf8_lossy(&graph.stderr);
    println!("GRAPH STDOUT:\n{}", graph_stdout);
    println!("GRAPH STDERR:\n{}", graph_stderr);
    assert!(
        graph.status.success(),
        "graph failed:\nstdout: {}\nstderr: {}",
        graph_stdout,
        graph_stderr
    );

    assert!(
        graph_stdout.contains("/page1"),
        "graph output missing /page1:\n{}",
        graph_stdout
    );
    assert!(
        graph_stdout.contains("/page2"),
        "graph output missing /page2:\n{}",
        graph_stdout
    );
    assert!(
        graph_stdout.contains("Discovered"),
        "graph output missing summary line:\n{}",
        graph_stdout
    );
}
