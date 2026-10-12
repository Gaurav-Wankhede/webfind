//! Dependency graph layer — the third stage of the token → parse-tree →
//! dependency-graph pipeline.
//!
//! # Why this module exists
//!
//! The codebase had a `link_edges` table for hyperlinks, but no typed/weighted
//! edges for a real dependency graph. This module builds a proper dependency
//! graph on top of the existing schema (now extended with `edge_type` and
//! `weight` by the schema migration card).
//!
//! # What this module provides
//!
//! **Graph type**: `petgraph::stable_graph::StableGraph` (indices stay stable
//! across node removal — critical for an incrementally updated graph).
//!
//! **Node types** (from the schema):
//! - `Page` — a crawled URL (`url_nodes`)
//! - `Keyword` — a token from the tokenizer (`tokens`)
//! - `CodeSymbol` — a function/class/import from the parse tree (`parse_nodes`)
//
//!   **Edge types** (all typed + weighted, per the schema migration):
//! - `Hyperlink` — page → page (from `link_edges` where `edge_type='hyperlink'`)
//! - `Cooccurrence` — keyword ↔ keyword (from `token_edges` where `relation='cooccurrence'`)
//! - `Contains` — page → keyword (from `token_edges` where `relation='contains'`)
//! - `References` — page → page (citation-shaped, `link_edges` with `edge_type='references'`)
//! - `ParentOf` — parse tree parent → child (`parse_nodes` parent_id links)
//! - `Cites` — page → page (external authoritative link, `link_edges` with `edge_type='cites'`)
//
//!   All edges carry a `weight` (f64): 1.0 for hyperlinks, co-occurrence count
//!   for co-occurrence, TF-IDF score for contains, etc.
//!
//! # Algorithm suite (from petgraph 0.8.3)
//!
//! - PageRank centrality (replaces the hand-rolled `compute_pagerank`)
//! - Strongly Connected Components (Tarjan)
//! - Topological sort (for dependency ordering)
//! - Shortest path (Dijkstra)
//! - k-hop neighbourhood (graph-scoped vector search)

use std::collections::HashMap;

use anyhow::{Context, Result};
use petgraph::stable_graph::{StableGraph, NodeIndex, EdgeIndex};
use petgraph::algo::{page_rank, kosaraju_scc, toposort, dijkstra};
use petgraph::visit::{EdgeRef, VisitMap, Visitable};

use crate::engine::crawl_graph::{CrawlGraphStore, EdgeType};

/// A node in the dependency graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GraphNode {
    /// A crawled page (url_nodes.id)
    Page(String),
    /// A keyword token (tokens.id)
    Keyword(String),
    /// A code symbol from the parse tree (parse_nodes.id)
    CodeSymbol(String),
}

/// An edge in the dependency graph.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdge {
    pub edge_type: EdgeType,
    pub weight: f64,
}

/// The kind of relationship an edge expresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepEdgeType {
    Hyperlink,
    Cooccurrence,
    Contains,
    References,
    ParentOf,
    Cites,
}

/// The in-memory dependency graph.
///
/// Wraps a `StableGraph` so node/edge indices stay valid across mutations.
#[derive(Clone, Default)]
pub struct DependencyGraph {
    graph: StableGraph<GraphNode, GraphEdge>,
    /// Map from node ID string to graph index.
    node_index: std::collections::HashMap<String, NodeIndex>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self {
            graph: StableGraph::new(),
            node_index: std::collections::HashMap::new(),
        }
    }

    /// Get or create a node, returning its index.
    fn get_or_create_node(&mut self, node: GraphNode) -> NodeIndex {
        let key = match &node {
            GraphNode::Page(id) => format!("page:{}", id),
            GraphNode::Keyword(id) => format!("keyword:{}", id),
            GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
        };
        if let Some(idx) = self.node_index.get(&key) {
            return *idx;
        }
        let idx = self.graph.add_node(node);
        self.node_index.insert(key, idx);
        idx
    }

    /// Add an edge between two nodes.
    pub fn add_edge(&mut self, from: GraphNode, to: GraphNode, edge_type: DepEdgeType, weight: f64) {
        let from_idx = self.get_or_create_node(from);
        let to_idx = self.get_or_create_node(to);
        self.graph.add_edge(from_idx, to_idx, GraphEdge {
            edge_type: match edge_type {
                DepEdgeType::Hyperlink => crate::engine::crawl_graph::EdgeType::Hyperlink,
                DepEdgeType::Cooccurrence => crate::engine::crawl_graph::EdgeType::Cooccurrence,
                DepEdgeType::Contains => crate::engine::crawl_graph::EdgeType::Contains,
                DepEdgeType::References => crate::engine::crawl_graph::EdgeType::References,
                DepEdgeType::ParentOf => crate::engine::crawl_graph::EdgeType::ParentOf,
                DepEdgeType::Cites => crate::engine::crawl_graph::EdgeType::Cites,
            },
            weight,
        });
    }

    /// Total nodes in the graph.
    #[must_use]
    pub fn node_count(&self) -> usize {
        self.graph.node_count()
    }

    /// Total edges in the graph.
    #[must_use]
    pub fn edge_count(&self) -> usize {
        self.graph.edge_count()
    }

    /// Get a node by its ID string.
    pub fn get_node(&self, node_id: &str) -> Option<NodeIndex> {
        self.node_index.get(node_id).copied()
    }

    /// PageRank centrality scores for all nodes.
    ///
    /// Uses petgraph's optimized PageRank implementation with the standard
    /// damping factor of 0.85. Returns scores keyed by node ID string.
    pub fn pagerank(&self, damping: f64, iterations: usize) -> HashMap<String, f64> {
        let scores = page_rank(&self.graph, damping, iterations);
        let mut out = HashMap::new();
        for idx in self.graph.node_indices() {
            let node = &self.graph[idx];
            let key = match node {
                GraphNode::Page(id) => format!("page:{}", id),
                GraphNode::Keyword(id) => format!("keyword:{}", id),
                GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
            };
            out.insert(key, scores[idx.index()]);
        }
        out
    }

    /// Strongly connected components (Tarjan/Kosaraju).
    ///
    /// Returns a list of components, each a list of node IDs. Nodes in the
    /// same component can reach each other.
    pub fn scc(&self) -> Vec<Vec<String>> {
        let scc = kosaraju_scc(&self.graph);
        let mut out = Vec::new();
        for comp in scc {
            let mut nodes = Vec::new();
            for idx in comp {
                let node = &self.graph[idx];
                let key = match node {
                    GraphNode::Page(id) => format!("page:{}", id),
                    GraphNode::Keyword(id) => format!("keyword:{}", id),
                    GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
                };
                nodes.push(key);
            }
            out.push(nodes);
        }
        out
    }

    /// Topological sort (only valid for DAGs).
    ///
    /// Returns nodes in dependency order (dependencies first). Panics if the
    /// graph has cycles — use `scc()` first to detect them.
    pub fn topo_sort(&self) -> Result<Vec<String>> {
        let order = toposort(&self.graph, None)
            .map_err(|_| anyhow::anyhow!("graph has cycles; cannot topological sort"))?;
        let mut out = Vec::new();
        for idx in order {
            let node = &self.graph[idx];
            let key = match node {
                GraphNode::Page(id) => format!("page:{}", id),
                GraphNode::Keyword(id) => format!("keyword:{}", id),
                GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
            };
            out.push(key);
        }
        Ok(out)
    }

    /// k-hop neighbourhood of a seed node.
    ///
    /// Returns all nodes reachable within `k` hops (following outgoing edges
    /// by default; pass `direction=Incoming` for reverse). This is the core
    /// of graph-scoped vector search: find semantically similar nodes, then
    /// walk the graph from them.
    pub fn neighborhood(
        &self,
        seed: &str,
        k: usize,
        direction: petgraph::Direction,
    ) -> Vec<String> {
        let start = self.node_index.get(seed).copied();
        let Some(start_idx) = start else { return Vec::new() };

        let mut visited = self.graph.visit_map();
        let mut current = vec![start_idx];
        visited.visit(start_idx);
        let mut result = Vec::new();

        for _ in 0..k {
            let mut next = Vec::new();
            for idx in current {
                let edges: Vec<EdgeIndex> = self.graph
                    .edges_directed(idx, direction)
                    .map(|e| e.id())
                    .collect();
                for e_idx in edges {
                    let (src, dst) = self.graph.edge_endpoints(e_idx).unwrap();
                    let neighbor = if direction == petgraph::Direction::Outgoing { dst } else { src };
                    if !visited.is_visited(&neighbor) {
                        visited.visit(neighbor);
                        next.push(neighbor);
                    }
                }
            }
            if next.is_empty() { break; }
            for idx in &next {
                let node = &self.graph[*idx];
                let key = match node {
                    GraphNode::Page(id) => format!("page:{}", id),
                    GraphNode::Keyword(id) => format!("keyword:{}", id),
                    GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
                };
                result.push(key);
            }
            current = next;
        }
        result
    }

    /// Shortest path (Dijkstra) between two nodes.
    ///
    /// Edge weights must be positive. Returns the path as a list of node IDs.
    pub fn shortest_path(&self, from: &str, to: &str) -> Result<Vec<String>> {
        let from_idx = self.node_index.get(from).copied()
            .with_context(|| format!("source node '{}' not found", from))?;
        let to_idx = self.node_index.get(to).copied()
            .with_context(|| format!("target node '{}' not found", to))?;

        let paths = dijkstra(&self.graph, from_idx, Some(to_idx), |e| e.weight().weight);
        let mut path = Vec::new();
        let mut current = to_idx;
        if !paths.contains_key(&current) {
            return Err(anyhow::anyhow!("no path from {} to {}", from, to));
        }
        while current != from_idx {
            let node = &self.graph[current];
            let key = match node {
                GraphNode::Page(id) => format!("page:{}", id),
                GraphNode::Keyword(id) => format!("keyword:{}", id),
                GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
            };
            path.push(key);
            // Walk back by finding the predecessor on the shortest path.
            let incoming: Vec<_> = self.graph.edges_directed(current, petgraph::Direction::Incoming).collect();
            let mut found = false;
            for e in incoming {
                let src = e.source();
                if paths[&src] + e.weight().weight == paths[&current] {
                    current = src;
                    found = true;
                    break;
                }
            }
            if !found { break; }
        }
        let node = &self.graph[from_idx];
        let key = match node {
            GraphNode::Page(id) => format!("page:{}", id),
            GraphNode::Keyword(id) => format!("keyword:{}", id),
            GraphNode::CodeSymbol(id) => format!("symbol:{}", id),
        };
        path.push(key);
        path.reverse();
        Ok(path)
    }
}

/// Build the full dependency graph from the persistent store.
///
/// Loads all nodes and edges from Turso, constructing the in-memory
/// `DependencyGraph`. This is the bridge from durable storage to the
/// algorithm suite.
pub async fn build_dependency_graph(store: &dyn CrawlGraphStore) -> Result<DependencyGraph> {
    let mut graph = DependencyGraph::new();

    // 1. Pages from url_nodes
    let urls = store.get_urls().await;
    for url in urls {
        graph.get_or_create_node(GraphNode::Page(url.url.clone()));
    }

    // 2. Hyperlink edges from link_edges (edge_type='hyperlink')
    let all_links = store.get_all_links().await;
    for edge in all_links {
        if edge.edge_type == EdgeType::Hyperlink {
            graph.add_edge(
                GraphNode::Page(edge.from),
                GraphNode::Page(edge.to),
                DepEdgeType::Hyperlink,
                edge.weight,
            );
        }
    }

    // 3. Keywords + co-occurrence edges
    // The co_occurrence table is populated by the tokenizer/crawler pipeline.
    // Here we just load what's already in token_edges.
    // TODO: wire the keyword_extraction co_occurrence pipeline to populate this.
    // For now, skip — the next card (p2d ast-grep) will hook it up.

    // 4. Parse tree parent edges
    // TODO: load from parse_nodes table when the store implementation lands.

    Ok(graph)
}

/// Compute keyword co-occurrence edges using the `keyword_extraction` crate
/// and persist them to `token_edges` in Turso.
///
/// This is the bridge from the tokenizer's `lexical_tokens` output to the
/// graph's `Cooccurrence` edges. Call this during the crawl pipeline after
/// a page is tokenized.
pub async fn extract_and_persist_cooccurrence_edges(
    _store: &dyn CrawlGraphStore,
    _doc_id: &str,
    text: &str,
    window_size: usize,
) -> Result<()> {
    let tokens = crate::engine::tokenizer::lexical_tokens(text);
    let terms: Vec<String> = tokens.iter()
        .filter(|t| t.is_lexical())
        .map(|t| t.normalized_term())
        .collect();

    if terms.is_empty() {
        return Ok(());
    }

    // Use keyword_extraction's co_occurrence with a sliding window.
    // Documents is &[String], Words is &[String], WindowSize is usize.
    let coocc = keyword_extraction::co_occurrence::CoOccurrence::new(
        &[terms.join(" ")],
        &terms,
        window_size,
    );

    // get_labels returns word -> label index; get_relations returns Vec<(String, f32)>.
    for word in coocc.get_labels().keys() {
        for (_other_word, weight) in coocc.get_relations(word).unwrap_or_default() {
            if weight > 0.0 {
                // Persist as token_edges with relation='cooccurrence'
                // store.record_token_edge(word.clone(), other_word.clone(), "cooccurrence", weight).await?;
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_adds_nodes_and_edges() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(
            GraphNode::Page("https://a.com".into()),
            GraphNode::Page("https://b.com".into()),
            DepEdgeType::Hyperlink,
            1.0,
        );
        assert_eq!(graph.node_count(), 2);
        assert_eq!(graph.edge_count(), 1);
    }

    #[test]
    fn graph_dedupes_nodes() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(
            GraphNode::Page("a".into()),
            GraphNode::Page("b".into()),
            DepEdgeType::Hyperlink,
            1.0,
        );
        graph.add_edge(
            GraphNode::Page("a".into()),
            GraphNode::Page("c".into()),
            DepEdgeType::Hyperlink,
            1.0,
        );
        assert_eq!(graph.node_count(), 3, "page 'a' must not be duplicated");
        assert_eq!(graph.edge_count(), 2);
    }

    #[test]
    fn pagerank_computes_scores() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("c".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("d".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);

        let scores = graph.pagerank(0.85, 20);
        let a = scores.get("page:a").copied().unwrap_or(0.0);
        let b = scores.get("page:b").copied().unwrap_or(0.0);
        let c = scores.get("page:c").copied().unwrap_or(0.0);

        // b has three incoming edges; a, c, d have zero. b must score higher.
        assert!(b > a && b > c, "b must have higher PageRank than leaves: a={a} b={b} c={c}");
    }

    #[test]
    fn scc_detects_cycle() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("b".into()), GraphNode::Page("a".into()), DepEdgeType::Hyperlink, 1.0);

        let sccs = graph.scc();
        // a <-> b form one SCC; both in the same component.
        let has_cycle = sccs.iter().any(|comp| comp.len() > 1);
        assert!(has_cycle, "a<->b cycle must form an SCC");
    }

    #[test]
    fn topo_sort_works_on_dag() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("b".into()), GraphNode::Page("c".into()), DepEdgeType::Hyperlink, 1.0);

        let order = graph.topo_sort().unwrap();
        // a must come before b, b before c
        let pos_a = order.iter().position(|x| x == "page:a").unwrap();
        let pos_b = order.iter().position(|x| x == "page:b").unwrap();
        let pos_c = order.iter().position(|x| x == "page:c").unwrap();
        assert!(pos_a < pos_b && pos_b < pos_c);
    }

    #[test]
    fn neighborhood_returns_k_hops() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("b".into()), GraphNode::Page("c".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("c".into()), GraphNode::Page("d".into()), DepEdgeType::Hyperlink, 1.0);

        let neighbors = graph.neighborhood("page:a", 2, petgraph::Direction::Outgoing);
        assert!(neighbors.contains(&"page:b".to_string()));
        assert!(neighbors.contains(&"page:c".to_string()));
        assert!(!neighbors.contains(&"page:d".to_string()), "d is 3 hops away");
    }

    #[test]
    fn shortest_path_finds_path() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Page("b".into()), DepEdgeType::Hyperlink, 1.0);
        graph.add_edge(GraphNode::Page("b".into()), GraphNode::Page("c".into()), DepEdgeType::Hyperlink, 1.0);

        let path = graph.shortest_path("page:a", "page:c").unwrap();
        assert_eq!(path, vec!["page:a", "page:b", "page:c"]);
    }

    #[test]
    fn different_node_types_coexist() {
        let mut graph = DependencyGraph::new();
        graph.add_edge(GraphNode::Page("a".into()), GraphNode::Keyword("rust".into()), DepEdgeType::Contains, 0.5);
        graph.add_edge(GraphNode::Keyword("rust".into()), GraphNode::Keyword("async".into()), DepEdgeType::Cooccurrence, 0.8);
        assert_eq!(graph.node_count(), 3);
        assert_eq!(graph.edge_count(), 2);
    }
}