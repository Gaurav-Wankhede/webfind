use ahash::AHashMap;
use std::collections::HashMap;
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
    /// Discovered via the site's curated LLM index (llms.txt / llm.txt).
    LlmsTxt,
    /// Discovered by following an internal link while crawling.
    LinkCrawl,
    /// Discovered by following an external (cross-domain) link.
    ExternalLink,
    /// Surfaced by a live multi-engine search (`webfind search --live`).
    ///
    /// Live hits are enqueued for background enrichment so the graph learns
    /// from live search instead of discarding the results.
    LiveSearch,
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
///
/// Edges are **typed and weighted**. A hyperlink, a keyword co-occurrence, a
/// citation, and a parse-tree parent relation are different relationships with
/// different traversal semantics, so collapsing them into one untyped edge
/// makes a dependency graph impossible to reason about.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkEdge {
    pub from: String,
    pub to: String,
    pub anchor_text: Option<String>,
    /// What kind of relationship this edge expresses.
    #[serde(default)]
    pub edge_type: EdgeType,
    /// Strength of the relationship. `1.0` for a plain hyperlink; a
    /// co-occurrence edge carries its observed co-occurrence count.
    #[serde(default = "default_edge_weight")]
    pub weight: f64,
    /// When the edge was recorded. `None` for rows migrated from a schema
    /// that predates this field.
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
}

/// Default weight for a hyperlink edge.
fn default_edge_weight() -> f64 {
    1.0
}

impl Default for LinkEdge {
    fn default() -> Self {
        Self {
            from: String::new(),
            to: String::new(),
            anchor_text: None,
            edge_type: EdgeType::Hyperlink,
            weight: default_edge_weight(),
            created_at: None,
        }
    }
}

/// The kind of relationship a [`LinkEdge`] expresses.
///
/// Serialized as a lowercase snake_case string so the SQL `edge_type` column
/// stays readable and stable across releases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EdgeType {
    /// An ordinary `<a href>` link between pages.
    #[default]
    Hyperlink,
    /// Two keywords observed together within a sliding window.
    Cooccurrence,
    /// A page contains a keyword.
    Contains,
    /// A page references another page in a citation-shaped way.
    References,
    /// A parse-tree node is the parent of another parse-tree node.
    ParentOf,
    /// A page cites an external authoritative source.
    Cites,
}

impl EdgeType {
    /// The stable string form stored in the `edge_type` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            EdgeType::Hyperlink => "hyperlink",
            EdgeType::Cooccurrence => "cooccurrence",
            EdgeType::Contains => "contains",
            EdgeType::References => "references",
            EdgeType::ParentOf => "parent_of",
            EdgeType::Cites => "cites",
        }
    }

    /// Parse the stored string form, defaulting to [`EdgeType::Hyperlink`].
    ///
    /// An unknown value defaults rather than erroring so a database written by
    /// a newer release stays readable by an older one.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "cooccurrence" => EdgeType::Cooccurrence,
            "contains" => EdgeType::Contains,
            "references" => EdgeType::References,
            "parent_of" => EdgeType::ParentOf,
            "cites" => EdgeType::Cites,
            _ => EdgeType::Hyperlink,
        }
    }
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

    /// Near-duplicate check at ingest: returns the canonical URL of an
    /// already-indexed page whose fingerprint is within threshold, or `None`.
    ///
    /// Default is a no-op (stores that cannot fingerprint never reject).
    /// The word floor, cross-domain allowance, and first-seen canonical rule
    /// are the implementor's responsibility; see `TursoStore` for the
    /// production policy.
    async fn find_duplicate(&self, url: &str, content_text: &str) -> Result<Option<String>> {
        let _ = (url, content_text);
        Ok(None)
    }

    /// Traverse the graph from `start` up to `max_depth` hops in `direction`,
    /// returning each reachable URL along with its shortest traversal depth from `start`.
    async fn traverse_with_depth(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<TraversalNode> {
        let mut depth_map: HashMap<String, u32> = HashMap::new();
        let mut current = vec![start.to_string()];
        let _ = depth_map.insert(start.to_string(), 0);

        for d in 1..=max_depth {
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
                    if !depth_map.contains_key(&neighbor) {
                        let _ = depth_map.insert(neighbor.clone(), d);
                        next.push(neighbor);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            current = next;
        }

        let mut nodes: Vec<TraversalNode> = depth_map
            .into_iter()
            .map(|(url, depth)| TraversalNode { url, depth })
            .collect();
        nodes.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.url.cmp(&b.url)));
        nodes
    }

    /// Traverse the graph from `start` up to `max_depth` hops in `direction`,
    /// returning the set of reachable URLs (including `start`).
    async fn traverse(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<String> {
        self.traverse_with_depth(start, max_depth, direction)
            .await
            .into_iter()
            .map(|n| n.url)
            .collect()
    }
}

/// A node visited during graph traversal with its hop depth from the root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraversalNode {
    pub url: String,
    pub depth: u32,
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

/// Breadth-first traversal of the crawl graph up to `max_depth` with node depths.
pub async fn traverse_graph_with_depth(
    store: Arc<dyn CrawlGraphStore>,
    start: &str,
    max_depth: u32,
    direction: TraversalDirection,
) -> Vec<TraversalNode> {
    store.traverse_with_depth(start, max_depth, direction).await
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

    let mut out_degree: AHashMap<String, usize> = AHashMap::new();
    let mut incoming: AHashMap<String, Vec<String>> = AHashMap::new();
    for edge in edges {
        *out_degree.entry(edge.from.clone()).or_insert(0) += 1;
        incoming.entry(edge.to.clone()).or_default().push(edge.from);
    }

    let base = (1.0 - damping) / n as f64;
    // Nodes without incoming edges receive no mass and stay at `base` for
    // every iteration, so only nodes with incoming edges need to be iterated.
    // On sparse graphs (few edges relative to nodes) this is a large constant
    // factor: 373K nodes with 9K edges → ~40x fewer hash-map ops per round.
    // Initializing every node to `base` (rather than `1/n`) is the correct
    // starting point: inactive nodes are already at their fixed point, and
    // active nodes converge to the same PageRank fixed point regardless of
    // initialization.
    let mut scores: AHashMap<String, f64> = nodes.iter().map(|u| (u.clone(), base)).collect();
    let active: Vec<&String> = nodes.iter().filter(|u| incoming.contains_key(*u)).collect();

    for _ in 0..iterations.max(1) {
        let mut new_scores = AHashMap::with_capacity(active.len());
        for url in &active {
            let mut rank = base;
            if let Some(in_nodes) = incoming.get(*url) {
                for in_url in in_nodes {
                    if let Some(s) = scores.get(in_url) {
                        let out_deg = out_degree.get(in_url).copied().unwrap_or(1).max(1);
                        rank += damping * s / out_deg as f64;
                    }
                }
            }
            let _ = new_scores.insert((*url).clone(), rank);
        }
        for (url, rank) in new_scores {
            let _ = scores.insert(url, rank);
        }
    }

    scores.into_iter().collect()
}

/// Incremental PageRank update: only recompute scores for nodes whose
/// incoming edges changed, plus their transitive dependents up to
/// `max_propagation_depth`.
///
/// This is a performance optimization over [`compute_pagerank`] for the
/// common case where a crawl adds a few new edges to an existing graph.
/// Instead of recomputing all N nodes × I iterations, we:
/// 1. Seed the changed nodes with their new incoming-edge contributions.
/// 2. Propagate score deltas through the graph for a bounded number of hops.
/// 3. Merge the deltas into the existing scores.
///
/// The result is approximate but converges to the same fixed point as
/// full recomputation when `max_propagation_depth` is large enough.
/// For small graphs or when precision matters, use [`compute_pagerank`].
pub async fn compute_pagerank_incremental(
    store: &dyn CrawlGraphStore,
    previous_scores: &HashMap<String, f64>,
    changed_urls: &[String],
    iterations: usize,
    damping: f64,
    max_propagation_depth: usize,
) -> HashMap<String, f64> {
    let nodes: Vec<String> = store.get_urls().await.into_iter().map(|n| n.url).collect();
    let edges = store.get_all_links().await;
    let n = nodes.len();
    if n == 0 {
        return HashMap::new();
    }

    let mut out_degree: AHashMap<String, usize> = AHashMap::new();
    let mut incoming: AHashMap<String, Vec<String>> = AHashMap::new();
    for edge in edges {
        *out_degree.entry(edge.from.clone()).or_insert(0) += 1;
        incoming.entry(edge.to.clone()).or_default().push(edge.from);
    }

    let base = (1.0 - damping) / n as f64;
    let mut scores = previous_scores.clone();

    // Ensure all nodes have a score (new nodes get base).
    for url in &nodes {
        let _ = scores.entry(url.clone()).or_insert(base);
    }

    // BFS from changed nodes to find all affected nodes.
    let mut affected: AHashMap<String, usize> = AHashMap::new();
    let mut queue: std::collections::VecDeque<(String, usize)> = std::collections::VecDeque::new();
    for url in changed_urls {
        if !affected.contains_key(url) {
            let _ = affected.insert(url.clone(), 0);
            queue.push_back((url.clone(), 0));
        }
    }
    while let Some((url, depth)) = queue.pop_front() {
        if depth >= max_propagation_depth {
            continue;
        }
        if let Some(out_nodes) = incoming.get(&url) {
            for in_url in out_nodes {
                if !affected.contains_key(in_url) {
                    let _ = affected.insert(in_url.clone(), depth + 1);
                    queue.push_back((in_url.clone(), depth + 1));
                }
            }
        }
    }

    // Recompute scores for affected nodes only.
    let active: Vec<&String> = affected.keys().collect();
    for _ in 0..iterations.max(1) {
        let mut new_scores = AHashMap::with_capacity(active.len());
        for url in &active {
            let mut rank = base;
            if let Some(in_nodes) = incoming.get(*url) {
                for in_url in in_nodes {
                    if let Some(s) = scores.get(in_url) {
                        let out_deg = out_degree.get(in_url).copied().unwrap_or(1).max(1);
                        rank += damping * s / out_deg as f64;
                    }
                }
            }
            let _ = new_scores.insert((*url).clone(), rank);
        }
        for (url, rank) in new_scores {
            let _ = scores.insert(url, rank);
        }
    }

    scores
}

/// In-memory graph store for tests and local crawls.
///
/// Uses `DashMap` for lock-free reads and fine-grained writes
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
        let _ = self.version.fetch_add(1, Ordering::SeqCst);
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
        let _ = self.urls.insert(node.url.clone(), node);
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
        let _ = self.page_content.insert(content.url_node.clone(), content);
        Ok(())
    }

    async fn record_embedding(&self, url: &str, embedding: Vec<f32>) -> Result<()> {
        let _ = self.embeddings.insert(url.to_string(), embedding);
        Ok(())
    }

    async fn enqueue_crawl_job(&self, url: &str) -> Result<String> {
        let id = format!("job-{}", self.jobs.len() + 1);
        let _ = self.jobs.insert(
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
            .record_link(hyperlink("https://example.com/", "https://example.com/a", Some("link")))
            .await;

        assert_eq!(graph.get_urls().await.len(), 2);
        assert_eq!(graph.get_links_from("https://example.com/").await.len(), 1);
        assert_eq!(graph.get_links_to("https://example.com/a").await.len(), 1);
    }

    /// Build a plain hyperlink edge — the common case in these tests.
    fn hyperlink(from: &str, to: &str, anchor: Option<&str>) -> LinkEdge {
        LinkEdge {
            from: from.to_string(),
            to: to.to_string(),
            anchor_text: anchor.map(str::to_string),
            ..Default::default()
        }
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
            .record_link(hyperlink("https://example.com/", "https://example.com/a", None))
            .await;
        graph
            .record_link(hyperlink("https://example.com/a", "https://example.com/b", None))
            .await;
        graph
            .record_link(hyperlink("https://example.com/", "https://example.com/c", None))
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
            .record_link(hyperlink("https://example.com/", "https://example.com/a", None))
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
            .record_link(hyperlink("https://example.com/a", "https://example.com/", None))
            .await;
        graph
            .record_link(hyperlink("https://example.com/b", "https://example.com/", None))
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

    #[test]
    fn edge_type_round_trips_through_its_string_form() {
        for edge_type in [
            EdgeType::Hyperlink,
            EdgeType::Cooccurrence,
            EdgeType::Contains,
            EdgeType::References,
            EdgeType::ParentOf,
            EdgeType::Cites,
        ] {
            assert_eq!(
                EdgeType::parse(edge_type.as_str()),
                edge_type,
                "{edge_type:?} must survive as_str -> parse"
            );
        }
    }

    #[test]
    fn edge_type_defaults_to_hyperlink() {
        // A row written before the column existed reads back as a hyperlink,
        // which is the only relationship the old schema could express.
        assert_eq!(EdgeType::default(), EdgeType::Hyperlink);
        assert_eq!(EdgeType::parse("hyperlink"), EdgeType::Hyperlink);
        // An unknown value from a newer release must not panic.
        assert_eq!(EdgeType::parse("some_future_relation"), EdgeType::Hyperlink);
        assert_eq!(EdgeType::parse(""), EdgeType::Hyperlink);
    }

    #[test]
    fn link_edge_default_is_a_weight_one_hyperlink() {
        let edge = LinkEdge::default();
        assert_eq!(edge.edge_type, EdgeType::Hyperlink);
        assert!((edge.weight - 1.0).abs() < f64::EPSILON);
        assert!(edge.created_at.is_none());
    }

    #[tokio::test]
    async fn typed_weighted_edges_preserve_type_and_weight() {
        let graph = InMemoryCrawlGraph::new();
        for url in ["https://example.com/a", "https://example.com/b"] {
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
                to: "https://example.com/b".to_string(),
                anchor_text: None,
                edge_type: EdgeType::Cooccurrence,
                weight: 5.0,
                created_at: None,
            })
            .await;

        let edges = graph.get_links_from("https://example.com/a").await;
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].edge_type, EdgeType::Cooccurrence);
        assert!((edges[0].weight - 5.0).abs() < f64::EPSILON);
    }
}
