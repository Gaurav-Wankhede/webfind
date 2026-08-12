use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};

use crate::schema::content::PageContentRecord;

/// Where a discovered URL came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoverySource {
    /// User-supplied starting point.
    Seed,
    /// Discovered via sitemap.xml (or sitemap index).
    Sitemap,
    /// Discovered by following an internal link while crawling.
    LinkCrawl,
    /// Discovered by following an external (cross-domain) link.
    ExternalLink,
}

/// A URL node in the crawl graph.
/// This maps directly to a Turso/libSQL `url_nodes` record for vector + link
/// analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UrlNode {
    pub url: String,
    pub domain: String,
    pub source: DiscoverySource,
    /// Hop count from the seed URL (0 = seed itself).
    pub depth: u32,
    pub priority: f32,
    pub lastmod: Option<DateTime<Utc>>,
    pub changefreq: Option<String>,
    pub discovered_at: DateTime<Utc>,
    pub crawled: bool,
}

/// A directed edge from one URL to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkEdge {
    pub from: String,
    pub to: String,
    pub anchor_text: Option<String>,
}

/// Durable background job for persisting crawled content + embeddings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlJob {
    pub id: String,
    pub url: String,
    pub status: String,
    pub attempts: i32,
    pub error: Option<String>,
}

/// Trait for persistable crawl-graph backends.
/// Implement this for Turso (`storage::turso_store`) or an in-memory store to
/// power graph-aware ranking and vector retrieval.
#[async_trait]
pub trait CrawlGraphStore: Send + Sync {
    async fn record_url(&self, node: UrlNode);
    async fn record_link(&self, edge: LinkEdge);
    async fn get_url(&self, url: &str) -> Option<UrlNode>;
    async fn get_urls(&self) -> Vec<UrlNode>;
    async fn get_links_from(&self, url: &str) -> Vec<LinkEdge>;
    async fn get_links_to(&self, url: &str) -> Vec<LinkEdge>;
    /// Return every link edge in the graph.
    async fn get_all_links(&self) -> Vec<LinkEdge>;
    /// Return a version token that changes whenever the graph is mutated.
    async fn graph_version(&self) -> String;

    /// Persist full page content into the knowledge graph.
    async fn record_page_content(&self, content: PageContentRecord) -> Result<()>;
    /// Persist a dense embedding for semantic search.
    async fn record_embedding(&self, url: &str, embedding: Vec<f32>) -> Result<()>;
    /// Enqueue a URL for background content + embedding persistence.
    async fn enqueue_crawl_job(&self, url: &str) -> Result<String>;
    /// Dequeue up to `limit` pending crawl jobs.
    async fn dequeue_crawl_jobs(&self, limit: usize) -> Result<Vec<CrawlJob>>;
    /// Update the status of a crawl job.
    async fn mark_crawl_job_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()>;

    /// Traverse the graph from `start` up to `max_depth` hops in `direction`,
    /// returning the set of reachable URLs (including `start`).
    ///
    /// The default implementation walks edges with an in-memory BFS. Backends
    /// that can express this more efficiently — e.g. Turso/libSQL recursive
    /// CTEs — override it for a single-query traversal.
    async fn traverse(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut current = vec![start.to_string()];
        visited.insert(start.to_string());

        for _ in 0..max_depth {
            let mut next = Vec::new();
            for url in &current {
                let edges: Vec<LinkEdge> = match direction {
                    TraversalDirection::Inbound => self.get_links_to(url).await,
                    TraversalDirection::Outbound => self.get_links_from(url).await,
                    TraversalDirection::Both => {
                        let mut e = self.get_links_from(url).await;
                        e.extend(self.get_links_to(url).await);
                        e
                    }
                };
                for edge in edges {
                    let neighbor = match direction {
                        TraversalDirection::Inbound => edge.from,
                        TraversalDirection::Outbound => edge.to,
                        TraversalDirection::Both => {
                            if edge.from == *url {
                                edge.to
                            } else {
                                edge.from
                            }
                        }
                    };
                    if visited.insert(neighbor.clone()) {
                        next.push(neighbor);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            current = next;
        }

        visited.into_iter().collect()
    }
}

/// Direction for graph traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraversalDirection {
    Inbound,
    Outbound,
    Both,
}

/// Breadth-first traversal of the crawl graph up to `max_depth`.
/// Returns the set of URLs reachable from `start` (including the start URL).
///
/// Delegates to [`CrawlGraphStore::traverse`], so backends that provide a
/// native implementation (e.g. a recursive CTE) are used automatically.
pub async fn traverse_graph(
    store: Arc<dyn CrawlGraphStore>,
    start: &str,
    max_depth: u32,
    direction: TraversalDirection,
) -> Vec<String> {
    store.traverse(start, max_depth, direction).await
}

/// Compute PageRank-style centrality scores for every URL in the graph.
///
/// Returns a map from URL to a score in the range (0.0, 1.0]. Nodes with no
/// incoming links receive the base (1-damping)/N score.
pub async fn compute_pagerank(
    store: &dyn CrawlGraphStore,
    iterations: usize,
    damping: f64,
) -> HashMap<String, f64> {
    let nodes: Vec<String> = store.get_urls().await.into_iter().map(|n| n.url).collect();
    let edges = store.get_all_links().await;
    let n = nodes.len();
    if n == 0 {
        return HashMap::new();
    }

    let mut out_degree: HashMap<String, usize> = HashMap::new();
    let mut incoming: HashMap<String, Vec<String>> = HashMap::new();
    for edge in edges {
        *out_degree.entry(edge.from.clone()).or_insert(0) += 1;
        incoming.entry(edge.to.clone()).or_default().push(edge.from);
    }

    let mut scores: HashMap<String, f64> =
        nodes.iter().map(|u| (u.clone(), 1.0 / n as f64)).collect();
    let base = (1.0 - damping) / n as f64;

    for _ in 0..iterations.max(1) {
        let mut new_scores = HashMap::with_capacity(n);
        for url in &nodes {
            let mut rank = base;
            if let Some(in_nodes) = incoming.get(url) {
                for in_url in in_nodes {
                    if let Some(s) = scores.get(in_url) {
                        let out_deg = out_degree.get(in_url).copied().unwrap_or(1).max(1);
                        rank += damping * s / out_deg as f64;
                    }
                }
            }
            new_scores.insert(url.clone(), rank);
        }
        scores = new_scores;
    }

    scores
}

/// In-memory graph store for tests and local crawls.
///
/// Uses `DashMap` for lock-free reads and fine-grained writes so an MCP
/// server can serve search/graph requests while a crawl is still running.
pub struct InMemoryCrawlGraph {
    urls: DashMap<String, UrlNode>,
    edges_from: DashMap<String, Vec<LinkEdge>>,
    edges_to: DashMap<String, Vec<LinkEdge>>,
    page_content: DashMap<String, PageContentRecord>,
    embeddings: DashMap<String, Vec<f32>>,
    jobs: DashMap<String, CrawlJob>,
    version: AtomicU64,
}

impl InMemoryCrawlGraph {
    pub fn new() -> Self {
        Self {
            urls: DashMap::new(),
            edges_from: DashMap::new(),
            edges_to: DashMap::new(),
            page_content: DashMap::new(),
            embeddings: DashMap::new(),
            jobs: DashMap::new(),
            version: AtomicU64::new(1),
        }
    }

    fn bump_version(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
    }
}

impl Default for InMemoryCrawlGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl CrawlGraphStore for InMemoryCrawlGraph {
    async fn record_url(&self, node: UrlNode) {
        self.urls.insert(node.url.clone(), node);
        self.bump_version();
    }

    async fn record_link(&self, edge: LinkEdge) {
        self.edges_from
            .entry(edge.from.clone())
            .or_default()
            .push(edge.clone());
        self.edges_to.entry(edge.to.clone()).or_default().push(edge);
        self.bump_version();
    }

    async fn get_url(&self, url: &str) -> Option<UrlNode> {
        self.urls.get(url).map(|r| r.clone())
    }

    async fn get_urls(&self) -> Vec<UrlNode> {
        self.urls.iter().map(|r| r.value().clone()).collect()
    }

    async fn get_links_from(&self, url: &str) -> Vec<LinkEdge> {
        self.edges_from
            .get(url)
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    async fn get_links_to(&self, url: &str) -> Vec<LinkEdge> {
        self.edges_to
            .get(url)
            .map(|r| r.clone())
            .unwrap_or_default()
    }

    async fn get_all_links(&self) -> Vec<LinkEdge> {
        self.edges_from
            .iter()
            .flat_map(|r| r.value().clone())
            .collect()
    }

    async fn graph_version(&self) -> String {
        self.version.load(Ordering::SeqCst).to_string()
    }

    async fn record_page_content(&self, content: PageContentRecord) -> Result<()> {
        self.page_content.insert(content.url_node.clone(), content);
        Ok(())
    }

    async fn record_embedding(&self, url: &str, embedding: Vec<f32>) -> Result<()> {
        self.embeddings.insert(url.to_string(), embedding);
        Ok(())
    }

    async fn enqueue_crawl_job(&self, url: &str) -> Result<String> {
        let id = format!("job-{}", self.jobs.len() + 1);
        self.jobs.insert(
            id.clone(),
            CrawlJob {
                id: id.clone(),
                url: url.to_string(),
                status: "pending".to_string(),
                attempts: 0,
                error: None,
            },
        );
        Ok(id)
    }

    async fn dequeue_crawl_jobs(&self, _limit: usize) -> Result<Vec<CrawlJob>> {
        Ok(self
            .jobs
            .iter()
            .filter(|j| j.status == "pending")
            .map(|j| j.value().clone())
            .collect())
    }

    async fn mark_crawl_job_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        if let Some(mut job) = self.jobs.get_mut(id) {
            job.status = status.to_string();
            job.error = error.map(|s| s.to_string());
            job.attempts += 1;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_in_memory_graph_records_urls_and_links() {
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
                anchor_text: Some("link".to_string()),
            })
            .await;

        assert_eq!(graph.get_urls().await.len(), 2);
        assert_eq!(graph.get_links_from("https://example.com/").await.len(), 1);
        assert_eq!(graph.get_links_to("https://example.com/a").await.len(), 1);
    }

    #[tokio::test]
    async fn test_traverse_graph_outbound() {
        let graph = Arc::new(InMemoryCrawlGraph::new());
        for url in [
            "https://example.com/",
            "https://example.com/a",
            "https://example.com/b",
            "https://example.com/c",
        ] {
            graph
                .record_url(UrlNode {
                    url: url.to_string(),
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
        }
        graph
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: None,
            })
            .await;
        graph
            .record_link(LinkEdge {
                from: "https://example.com/a".to_string(),
                to: "https://example.com/b".to_string(),
                anchor_text: None,
            })
            .await;
        graph
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/c".to_string(),
                anchor_text: None,
            })
            .await;

        let visited = traverse_graph(
            graph.clone(),
            "https://example.com/",
            2,
            TraversalDirection::Outbound,
        )
        .await;
        let urls: Vec<&str> = visited.iter().map(|s| s.as_str()).collect();
        assert!(urls.contains(&"https://example.com/"));
        assert!(urls.contains(&"https://example.com/a"));
        assert!(urls.contains(&"https://example.com/b"));
        assert!(urls.contains(&"https://example.com/c"));
    }

    #[tokio::test]
    async fn test_traverse_graph_depth_zero() {
        let graph = Arc::new(InMemoryCrawlGraph::new());
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
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: None,
            })
            .await;

        let visited = traverse_graph(
            graph.clone(),
            "https://example.com/",
            0,
            TraversalDirection::Outbound,
        )
        .await;
        assert_eq!(visited, vec!["https://example.com/".to_string()]);
    }

    #[tokio::test]
    async fn test_compute_pagerank_star_graph() {
        let graph = InMemoryCrawlGraph::new();
        for url in [
            "https://example.com/",
            "https://example.com/a",
            "https://example.com/b",
        ] {
            graph
                .record_url(UrlNode {
                    url: url.to_string(),
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
        }
        graph
            .record_link(LinkEdge {
                from: "https://example.com/a".to_string(),
                to: "https://example.com/".to_string(),
                anchor_text: None,
            })
            .await;
        graph
            .record_link(LinkEdge {
                from: "https://example.com/b".to_string(),
                to: "https://example.com/".to_string(),
                anchor_text: None,
            })
            .await;

        let scores = compute_pagerank(&graph, 20, 0.85).await;
        let root = scores.get("https://example.com/").copied().unwrap();
        let leaf = scores.get("https://example.com/a").copied().unwrap();
        assert!(
            root > leaf,
            "root should have higher PageRank than leaves: {} vs {}",
            root,
            leaf
        );
    }

    #[tokio::test]
    async fn test_compute_pagerank_empty_graph() {
        let graph = InMemoryCrawlGraph::new();
        let scores = compute_pagerank(&graph, 10, 0.85).await;
        assert!(scores.is_empty());
    }
}
