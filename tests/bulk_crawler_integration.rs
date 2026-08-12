use std::sync::Arc;

use webfind::engine::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use webfind::engine::crawl_graph::{CrawlGraphStore, DiscoverySource};
use webfind::engine::proxy_pool::ProxyPool;

mod common;

#[tokio::test]
async fn test_bulk_crawler_discovers_linked_pages() {
    let (_server, base_url) = common::start_test_server().await;

    let crawler = BulkDomainCrawler::new(
        ProxyPool::new(),
        10,
        60,
        50,
        10,
        10,
        FollowExternalLinks::Ignore,
        0,
        5,
    )
    .with_respect_robots(RespectRobots::No);

    let results = crawler
        .crawl(&base_url)
        .await
        .expect("crawl should succeed");

    assert!(
        results.len() >= 4,
        "expected at least 4 pages, got {}",
        results.len()
    );

    let titles: Vec<String> = results.iter().map(|r| r.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "Home"), "missing Home");
    assert!(titles.iter().any(|t| t == "Page One"), "missing Page One");
    assert!(titles.iter().any(|t| t == "Page Two"), "missing Page Two");
    let urls: Vec<String> = results.iter().map(|r| r.url.clone()).collect();
    assert!(
        urls.iter().any(|u| u.contains("/private/secret")),
        "missing Secret link"
    );

    for r in &results {
        assert!(r.is_valid_content, "{} should be valid content", r.url);
    }
}

#[tokio::test]
async fn test_bulk_crawler_respects_max_pages() {
    let (_server, base_url) = common::start_test_server().await;

    let crawler = BulkDomainCrawler::new(
        ProxyPool::new(),
        10,
        60,
        50,
        10,
        2,
        FollowExternalLinks::Ignore,
        0,
        5,
    )
    .with_respect_robots(RespectRobots::No);

    let results = crawler
        .crawl(&base_url)
        .await
        .expect("crawl should succeed");
    assert!(
        results.len() <= 2,
        "expected at most 2 pages, got {}",
        results.len()
    );
}

#[tokio::test]
async fn test_bulk_crawler_uses_sitemap_and_respects_robots() {
    let (_server, base_url) = common::start_test_server().await;

    let graph = webfind::engine::crawl_graph::InMemoryCrawlGraph::new();
    let graph = Arc::new(graph);

    let crawler = BulkDomainCrawler::new(
        ProxyPool::new(),
        10,
        60,
        10,
        100,
        10,
        FollowExternalLinks::Ignore,
        0,
        5,
    )
    .with_respect_robots(RespectRobots::Yes)
    .with_graph_store(graph.clone());

    let results = crawler
        .crawl(&base_url)
        .await
        .expect("crawl should succeed");

    // Should discover home + page1 + page2, but NOT /private/secret.
    let titles: Vec<String> = results.iter().map(|r| r.title.clone()).collect();
    assert!(titles.iter().any(|t| t == "Home"), "missing Home");
    assert!(titles.iter().any(|t| t == "Page One"), "missing Page One");
    assert!(titles.iter().any(|t| t == "Page Two"), "missing Page Two");
    assert!(
        !titles.iter().any(|t| t == "Secret"),
        "robots.txt disallow /private/ was not respected"
    );

    // Graph should contain sitemap-discovered URLs and seed.
    let urls = graph.get_urls().await;
    let sitemap_count = urls
        .iter()
        .filter(|u| u.source == DiscoverySource::Sitemap)
        .count();
    assert!(
        sitemap_count >= 2,
        "expected sitemap sources in graph, found {}",
        sitemap_count
    );

    // Sitemap entries should be seeded before the crawl reaches them via links.
    // We verify the graph recorded them.
    let page1 = urls.iter().find(|u| u.url.contains("/page1"));
    assert!(page1.is_some(), "page1 not recorded in graph");
    assert_eq!(page1.unwrap().source, DiscoverySource::Sitemap);
}
