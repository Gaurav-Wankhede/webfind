use std::collections::{HashMap, HashSet};

use chrono::Utc;

use crate::engine::crawl_graph::{CrawlGraphStore, LinkEdge};
use crate::engine::util;
use crate::schema::response::{DomainAuthority, DomainNode, GraphEdge, GraphSummary, SearchResult};

/// Build a graph summary around a slice of search results.
///
/// Returns `None` if no graph store is available. The summary includes
/// inbound/outbound links for the result URLs, plus per-domain statistics.
pub async fn build_graph_summary(
    store: &dyn CrawlGraphStore,
    results: &[SearchResult],
) -> Option<GraphSummary> {
    let result_urls: Vec<String> = results.iter().map(|r| r.url.clone()).collect();
    let mut result_domains = HashSet::new();
    for url in &result_urls {
        if let Some(domain) = util::extract_domain(url) {
            result_domains.insert(domain);
        }
    }

    let mut inbound_links = Vec::new();
    let mut outbound_links = Vec::new();
    for url in &result_urls {
        for edge in store.get_links_to(url).await {
            inbound_links.push(to_graph_edge(&edge));
        }
        for edge in store.get_links_from(url).await {
            outbound_links.push(to_graph_edge(&edge));
        }
    }

    let all_nodes = store.get_urls().await;
    let mut domain_pages: HashMap<String, u32> = HashMap::new();
    for node in all_nodes {
        if result_domains.contains(&node.domain) {
            *domain_pages.entry(node.domain).or_insert(0) += 1;
        }
    }
    let related_domains: Vec<DomainNode> = domain_pages
        .into_iter()
        .map(|(domain, page_count)| DomainNode {
            domain,
            page_count,
            avg_authority: 0.0,
        })
        .collect();

    let all_edges = store.get_all_links().await;
    let mut inbound_counts: HashMap<String, u32> = HashMap::new();
    let mut outbound_counts: HashMap<String, u32> = HashMap::new();
    for edge in all_edges {
        if let Some(from) = util::extract_domain(&edge.from) {
            *outbound_counts.entry(from).or_insert(0) += 1;
        }
        if let Some(to) = util::extract_domain(&edge.to) {
            *inbound_counts.entry(to).or_insert(0) += 1;
        }
    }
    let domain_authority: Vec<DomainAuthority> = result_domains
        .into_iter()
        .map(|domain| DomainAuthority {
            domain: domain.clone(),
            authority_score: 0.0,
            inbound_links: inbound_counts.get(&domain).copied().unwrap_or(0),
            outbound_links: outbound_counts.get(&domain).copied().unwrap_or(0),
        })
        .collect();

    Some(GraphSummary {
        inbound_links,
        outbound_links,
        related_domains,
        domain_authority,
    })
}

fn to_graph_edge(edge: &LinkEdge) -> GraphEdge {
    GraphEdge {
        source_url: edge.from.clone(),
        target_url: edge.to.clone(),
        anchor_text: edge.anchor_text.clone(),
        crawled_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::crawl_graph::{DiscoverySource, InMemoryCrawlGraph, UrlNode};

    #[tokio::test]
    async fn test_build_summary_for_results() {
        let graph = InMemoryCrawlGraph::new();
        graph
            .record_url(UrlNode {
                url: "https://example.com/".to_string(),
                domain: "example.com".to_string(),
                source: DiscoverySource::Seed,
                depth: 0,
                priority: 1.0,
                lastmod: None,
                changefreq: None,
                discovered_at: Utc::now(),
                crawled: false,
            })
            .await;
        graph
            .record_url(UrlNode {
                url: "https://example.com/a".to_string(),
                domain: "example.com".to_string(),
                source: DiscoverySource::LinkCrawl,
                depth: 1,
                priority: 0.5,
                lastmod: None,
                changefreq: None,
                discovered_at: Utc::now(),
                crawled: false,
            })
            .await;
        graph
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: Some("a".to_string()),
            })
            .await;

        let results = vec![SearchResult {
            rank: 1,
            url: "https://example.com/a".to_string(),
            title: "A".to_string(),
            snippet: "".to_string(),
            domain: "example.com".to_string(),
            published_at: None,
            modified_at: None,
            crawled_at: Utc::now(),
            author: None,
            site_name: None,
            score: 0.0,
            scores: crate::schema::response::ScoreBreakdown {
                bm25: None,
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                ax_score: None,
                final_score: 0.0,
            },
            content: None,
            keywords: None,
            metrics: None,
            favicon: None,
            thumbnail: None,
            llms_txt: None,
            ai_catalog: None,
            openapi_spec: None,
            mcp_server: None,
            language: "en".to_string(),
            content_type: "text".to_string(),
        }];

        let summary = build_graph_summary(&graph, &results).await.unwrap();
        assert_eq!(summary.inbound_links.len(), 1);
        assert_eq!(summary.inbound_links[0].source_url, "https://example.com/");
        assert_eq!(summary.related_domains.len(), 1);
        assert_eq!(summary.related_domains[0].page_count, 2);
        assert_eq!(summary.domain_authority[0].inbound_links, 1);
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            util::extract_domain("https://Example.com/foo"),
            Some("example.com".to_string())
        );
        assert_eq!(
            util::extract_domain("http://localhost:8080/"),
            Some("localhost".to_string())
        );
        assert_eq!(util::extract_domain("not-a-url"), None);
    }
}
