use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::engine::crawl_graph::{CrawlGraphStore, compute_pagerank};

/// Persisted PageRank cache entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedPageRank {
    graph_version: String,
    computed_at: DateTime<Utc>,
    scores: HashMap<String, f64>,
}

/// On-disk cache for graph centrality scores.
///
/// PageRank is expensive to recompute on every search request. This cache
/// stores the last computed scores keyed by the graph backend's version token.
/// When the graph is mutated, `graph_version` changes and the cache is
/// invalidated and refreshed.
pub struct PageRankCache {
    path: PathBuf,
}

impl PageRankCache {
    pub fn new(data_dir: impl AsRef<Path>) -> Self {
        Self {
            path: data_dir.as_ref().join("pagerank_cache.json"),
        }
    }

    /// Return cached scores if the graph version matches, otherwise recompute
    /// and persist.
    pub async fn get_or_compute(
        &self,
        store: &dyn CrawlGraphStore,
        iterations: usize,
        damping: f64,
    ) -> Result<HashMap<String, f64>> {
        let current_version = store.graph_version().await;

        if let Some(cached) = self.load()? {
            if cached.graph_version == current_version {
                return Ok(cached.scores);
            }
        }

        let scores = compute_pagerank(store, iterations, damping).await;
        self.save(&CachedPageRank {
            graph_version: current_version,
            computed_at: Utc::now(),
            scores: scores.clone(),
        })?;
        Ok(scores)
    }

    fn load(&self) -> Result<Option<CachedPageRank>> {
        if !self.path.exists() {
            return Ok(None);
        }
        let contents = std::fs::read_to_string(&self.path)
            .with_context(|| format!("read PageRank cache {}", self.path.display()))?;
        let cached: CachedPageRank = serde_json::from_str(&contents)
            .with_context(|| format!("parse PageRank cache {}", self.path.display()))?;
        Ok(Some(cached))
    }

    fn save(&self, cached: &CachedPageRank) -> Result<()> {
        let tmp = self.path.with_extension("tmp");
        let contents = serde_json::to_string_pretty(cached).context("serialize PageRank cache")?;
        std::fs::write(&tmp, contents)
            .with_context(|| format!("write PageRank cache tmp {}", tmp.display()))?;
        std::fs::rename(&tmp, &self.path)
            .with_context(|| format!("rename PageRank cache to {}", self.path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::crawl_graph::{DiscoverySource, InMemoryCrawlGraph, LinkEdge, UrlNode};

    #[tokio::test]
    async fn test_cache_invalidates_on_version_change() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PageRankCache::new(dir.path());
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

        let first = cache.get_or_compute(&graph, 10, 0.85).await.unwrap();
        assert!(!first.is_empty());
        assert!(cache.path.exists());

        // Recompute should return cached scores without recomputing.
        let second = cache.get_or_compute(&graph, 10, 0.85).await.unwrap();
        assert_eq!(first, second);

        // Mutate graph -> version changes -> cache invalidated and recomputed.
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
                from: "https://example.com/a".to_string(),
                to: "https://example.com/".to_string(),
                anchor_text: None,
            })
            .await;
        let third = cache.get_or_compute(&graph, 10, 0.85).await.unwrap();
        assert!(third.contains_key("https://example.com/a"));
    }
}
