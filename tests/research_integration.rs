use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::{Router, response::Html, routing::get};
use tokio::net::TcpListener;
use webfind::engine::indexer::Indexer;
use webfind::engine::search_engine::InMemorySearchEngine;

#[tokio::test]
async fn test_research_endpoint_crawls_seed_and_returns_results() {
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().to_path_buf();
    let indexer = Indexer::new(Arc::new(InMemorySearchEngine::new()));
    let state = Arc::new(webfind::api::ApiState::new(indexer, None, None, data_dir.clone()));
    let app = webfind::api::app(state, None);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let api_addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let seed_app = Router::new()
        .route(
            "/",
            get(|| async {
                Html(
                    r#"<html>
<head><title>Test Research Seed</title></head>
<body>
  <h1>Rust programming language</h1>
  <p>Rust is a systems programming language that guarantees memory safety and thread safety without relying on a garbage collector. It achieves this through an ownership model enforced at compile time by the borrow checker.</p>
  <p>The language is designed for performance and reliability, making it ideal for operating systems, web servers, embedded devices, and command line tools where safety matters.</p>
  <a href="/page2">Page two</a>
</body>
</html>"#,
                )
            }),
        )
        .route(
            "/page2",
            get(|| async {
                Html(
                    r#"<html>
<head><title>Page Two</title></head>
<body>
  <h1>Rust concurrency</h1>
  <p>Rust concurrency is powered by ownership and the borrow checker. Because the compiler tracks which thread owns each piece of data, many data races are caught before the program ever runs.</p>
  <p>Channels, mutexes, and atomic types in the standard library provide safe patterns for communicating between threads without introducing undefined behavior.</p>
</body>
</html>"#,
                )
            }),
        )
        .route("/robots.txt", get(|| async { "User-agent: *\nAllow: /\n" }));

    let seed_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let seed_addr: SocketAddr = seed_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(seed_listener, seed_app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(200)).await;

    let client = reqwest::Client::new();
    let url = format!(
        "http://{}/research?seed=http://{}/&q=rust+programming&max_pages=5&delay=10&include_graph=true",
        api_addr, seed_addr
    );
    let resp = client
        .get(&url)
        .timeout(Duration::from_secs(30))
        .send()
        .await
        .expect("research request to succeed");

    assert!(
        resp.status().is_success(),
        "research endpoint returned non-success: {:?}",
        resp.status()
    );

    let body_text = resp.text().await.expect("response text");
    let body: serde_json::Value = serde_json::from_str(&body_text).expect("valid JSON response");
    let results = body
        .get("results")
        .and_then(|v| v.as_array())
        .expect("response should contain results array");
    assert!(
        !results.is_empty(),
        "research should return results: {:#?}",
        body
    );

    let text = results[0]
        .get("title")
        .or_else(|| results[0].get("snippet"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    assert!(
        text.contains("rust"),
        "top result should mention rust: {}",
        text
    );

    let graph = body
        .get("graph")
        .expect("response should include graph summary");
    let links = graph
        .get("outbound_links")
        .and_then(|v| v.as_array())
        .or_else(|| graph.get("inbound_links").and_then(|v| v.as_array()))
        .expect("graph should contain links");
    assert!(!links.is_empty(), "graph should contain at least one link");
}
