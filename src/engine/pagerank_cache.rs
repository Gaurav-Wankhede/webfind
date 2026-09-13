use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

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

/// Process-wide lock serializing the recompute+persist critical section.
///
/// Callers construct a fresh `PageRankCache` per request (parallel search fans
/// out several at once), so the lock cannot live on the instance: without a
/// shared lock, concurrent cache misses race on the shared `.tmp` file —
/// interleaved `std::fs::write`s corrupt it, the losing `rename` errors out,
/// and the corrupted file then fails every subsequent `load()` until it is
/// deleted manually (persistent search failure).
static COMPUTE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn compute_lock() -> &'static Mutex<()> {
    COMPUTE_LOCK.get_or_init(|| Mutex::new(()))
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

        if let Some(cached) = self.load()?
            && cached.graph_version == current_version {
                return Ok(cached.scores);
            }

        // Serialize recompute+save across all cache instances in this process
        // (see `COMPUTE_LOCK`). The double-check after acquiring the lock lets
        // concurrent callers observe the winner's fresh cache instead of each
        // recomputing PageRank.
        let _guard = compute_lock().lock().await;

        if let Some(cached) = self.load()?
            && cached.graph_version == current_version {
                return Ok(cached.scores);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_recompute_does_not_corrupt_cache() {
        let dir = tempfile::tempdir().unwrap();
        let graph = std::sync::Arc::new(InMemoryCrawlGraph::new());
        for i in 0..5 {
            graph
                .record_url(UrlNode {
                    url: format!("https://example.com/{i}"),
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
        }

        // 8 concurrent cache instances all miss (no cache file yet) and race
        // to recompute + persist. Before the process-wide lock this corrupted
        // the shared `.tmp` file and made half the calls fail. All must now
        // succeed and the persisted file must remain parseable.
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.path().to_path_buf();
                let graph = graph.clone();
                tokio::spawn(async move {
                    let cache = PageRankCache::new(&dir);
                    cache.get_or_compute(graph.as_ref(), 10, 0.85).await
                })
            })
            .collect();
        for task in tasks {
            let scores = task
                .await
                .expect("task panicked")
                .expect("recompute failed");
            assert!(!scores.is_empty());
        }

        let cache = PageRankCache::new(dir.path());
        assert!(cache.load().unwrap().is_some());
    }
}
