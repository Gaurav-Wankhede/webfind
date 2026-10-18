//! Crawl graph schema for storing link relationships and PageRank
//!
//! This module defines the crawl graph schema for storing link relationships
//! between documents, enabling graph-based ranking and traversal.

use serde::{Deserialize, Serialize};
use chrono::{DateTime, Utc};

/// Crawl graph node (document/URL)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlNode {
    pub id: i64,
    pub url: String,
    pub title: String,
    pub domain: String,
    pub source_tier: String,
    pub pagerank: f64,
    pub fetched_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

/// Crawl graph edge (link from one page to another)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlEdge {
    pub id: i64,
    pub source_url: String,
    pub target_url: String,
    pub anchor_text: Option<String>,
    pub weight: f32,
    pub edge_type: EdgeType,
    pub discovered_at: DateTime<Utc>,
}

/// Edge type classification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum EdgeType {
    Internal,    // Link within same domain
    External,    // Link to different domain
    Navigation,  // Navigation/header/footer link
    Content,     // In-content link
    Redirect,    // HTTP redirect
}

impl Default for EdgeType {
    fn default() -> Self {
        EdgeType::Content
    }
}

/// SQL statements for creating crawl graph tables
pub mod sql {
    /// Create crawl nodes table
    pub const CREATE_CRAWL_NODES: &str = r#"
        CREATE TABLE IF NOT EXISTS crawl_nodes (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            url TEXT NOT NULL UNIQUE,
            title TEXT NOT NULL DEFAULT '',
            domain TEXT NOT NULL,
            source_tier TEXT NOT NULL DEFAULT 'unknown',
            pagerank REAL NOT NULL DEFAULT 1.0,
            fetched_at DATETIME NOT NULL,
            created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
        );
    "#;

    /// Create crawl edges table
    pub const CREATE_CRAWL_EDGES: &str = r#"
        CREATE TABLE IF NOT EXISTS crawl_edges (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            source_url TEXT NOT NULL,
            target_url TEXT NOT NULL,
            anchor_text TEXT,
            weight REAL NOT NULL DEFAULT 1.0,
            edge_type TEXT NOT NULL DEFAULT 'content',
            discovered_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (source_url, target_url)
        );
    "#;

    /// Create indexes for efficient graph traversal
    pub const CREATE_INDEXES: &str = r#"
        CREATE INDEX IF NOT EXISTS idx_crawl_edges_source ON crawl_edges(source_url);
        CREATE INDEX IF NOT EXISTS idx_crawl_edges_target ON crawl_edges(target_url);
        CREATE INDEX IF NOT EXISTS idx_crawl_nodes_domain ON crawl_nodes(domain);
        CREATE INDEX IF NOT EXISTS idx_crawl_nodes_pagerank ON crawl_nodes(pagerank DESC);
    "#;

    /// Insert or update a node
    pub const UPSERT_NODE: &str = r#"
        INSERT INTO crawl_nodes (url, title, domain, source_tier, fetched_at)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(url) DO UPDATE SET
            title = excluded.title,
            domain = excluded.domain,
            source_tier = excluded.source_tier,
            fetched_at = excluded.fetched_at;
    "#;

    /// Insert edge
    pub const INSERT_EDGE: &str = r#"
        INSERT INTO crawl_edges (source_url, target_url, anchor_text, weight, edge_type)
        VALUES (?, ?, ?, ?, ?)
        ON CONFLICT(source_url, target_url) DO UPDATE SET
            anchor_text = excluded.anchor_text,
            weight = excluded.weight,
            edge_type = excluded.edge_type;
    "#;

    /// Get outgoing edges for a node
    pub const GET_OUTGOING_EDGES: &str = r#"
        SELECT target_url, anchor_text, weight, edge_type
        FROM crawl_edges
        WHERE source_url = ?
        ORDER BY weight DESC;
    "#;

    /// Get incoming edges for a node
    pub const GET_INCOMING_EDGES: &str = r#"
        SELECT source_url, anchor_text, weight, edge_type
        FROM crawl_edges
        WHERE target_url = ?
        ORDER BY weight DESC;
    "#;

    /// Compute PageRank (simplified single iteration)
    pub const COMPUTE_PAGERANK: &str = r#"
        WITH ranked AS (
            SELECT 
                n.url,
                0.15 + 0.85 * COALESCE(SUM(e.weight * n2.pagerank / NULLIF(n2.out_degree, 0)), 0) as new_pagerank
            FROM crawl_nodes n
            LEFT JOIN crawl_edges e ON n.url = e.source_url
            LEFT JOIN crawl_nodes n2 ON e.target_url = n2.url
            LEFT JOIN (
                SELECT source_url, COUNT(*) as out_degree
                FROM crawl_edges
                GROUP BY source_url
            ) n2 ON n2.source_url = n.url
            GROUP BY n.url
        )
        UPDATE crawl_nodes
        SET pagerank = r.new_pagerank
        FROM ranked r
        WHERE crawl_nodes.url = r.url;
    "#;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sql_constants_exist() {
        assert!(!sql::CREATE_CRAWL_NODES.is_empty());
        assert!(!sql::CREATE_CRAWL_EDGES.is_empty());
        assert!(!sql::CREATE_INDEXES.is_empty());
    }
}