use super::*;
use anyhow::{Context, Result};
use libsql::params;
use libsql::TransactionBehavior;

impl TursoStore {
pub async fn compute_pagerank(
        &self,
        iterations: usize,
        damping: f64,
    ) -> Result<std::collections::HashMap<String, f64>> {
        // Try incremental update: if we have cached scores from the same graph
        // version, only recompute affected nodes. Otherwise fall back to full.
        let scores = match self.pagerank_scores().await {
            Ok(cached) if !cached.is_empty() => {
                let prev: std::collections::HashMap<String, f64> =
                    cached.into_iter().collect();
                // Find nodes whose incoming edges changed since last compute.
                // For now, use all nodes with edges as potentially affected.
                let changed: Vec<String> = prev.keys().cloned().collect();
                crate::engine::crawl_graph::compute_pagerank_incremental(
                    self, &prev, &changed, iterations, damping, 5,
                ).await
            }
            _ => crate::engine::crawl_graph::compute_pagerank(self, iterations, damping).await,
        };

        // Rewrite the cache atomically. The DELETE + all INSERTs run under one
        // transaction: on a large graph (hundreds of thousands of nodes) each
        // auto-committed INSERT would fsync individually and take minutes.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .context("begin pagerank cache transaction")?;
        let _ = tx.execute("DELETE FROM pagerank", ())
            .await
            .context("clear pagerank cache")?;
        for (url, score) in &scores {
            let _ = tx.execute(
                "INSERT INTO pagerank (url, score) VALUES (?1, ?2)",
                params![url.as_str(), *score],
            )
            .await
            .with_context(|| format!("cache pagerank for `{url}`"))?;
        }
        // Record the graph version this cache was computed from. Search skips
        // recomputation while `pagerank_version == graph_version`. This is a
        // derived-cache write, NOT a graph mutation, so it must not bump the
        // graph version (bumping here would invalidate the cache on every
        // compute and force a full recompute per search).
        let version = self.graph_version().await;
        let _ = tx.execute(
            "INSERT INTO graph_meta (key, value) VALUES ('pagerank_version', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![version],
        )
        .await
        .context("record pagerank version")?;
        tx.commit().await.context("commit pagerank cache")?;
        Ok(scores)
    }

    /// Graph version the `pagerank` cache was computed from, or `"0"` when
    /// never computed. Search compares this against `graph_version()` to skip
    /// recomputation on unchanged graphs.
    pub async fn pagerank_version(&self) -> String {
        let mut rows = match self
            .conn
            .query(
                "SELECT value FROM graph_meta WHERE key = 'pagerank_version'",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to read pagerank version: {}", e);
                return "0".to_string();
            }
        };
        match rows.next().await {
            Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0).to_string(),
            _ => "0".to_string(),
        }
    }

    /// Read the cached PageRank scores as `(url, score)` ordered descending.
    pub async fn pagerank_scores(&self) -> Result<Vec<(String, f64)>> {
        let mut rows = self
            .conn
            .query("SELECT url, score FROM pagerank ORDER BY score DESC", ())
            .await
            .context("read pagerank cache")?;
        let mut out = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let score: f64 = row.get(1).unwrap_or(0.0);
                    out.push((url, score));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate pagerank cache"),
            }
        }
        Ok(out)
    }

    /// Read only the top-`limit` cached PageRank scores, ordered descending.
    ///
    /// RRF fusion only needs the highest-centrality URLs, so reading the whole
    /// table on every hybrid search is wasteful — it dominates latency at 100K
    /// docs. The `LIMIT` keeps the read proportional to the result size rather
    /// than the index size.
    pub async fn pagerank_scores_top(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        let limit = limit.clamp(1, 500) as i64;
        let mut rows = self
            .conn
            .query(
                "SELECT url, score FROM pagerank ORDER BY score DESC LIMIT ?1",
                params![limit],
            )
            .await
            .context("read top pagerank cache")?;
        let mut out = Vec::with_capacity(limit as usize);
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = row.get(0).context("read url from pagerank cache")?;
                    let score: f64 = row.get(1).context("read score from pagerank cache")?;
                    out.push((url, score));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate top pagerank cache"),
            }
        }
        Ok(out)
    }

    /// Read the top-`limit` URLs by freshness (most recently discovered first),
    /// returning `(url, freshness_score)` where freshness_score is a normalized
    /// value in [0, 1] (1.0 = most recent, 0.0 = oldest in the result set).
    pub async fn freshness_scores_top(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        let limit = limit.clamp(1, 500) as i64;
        let mut rows = self
            .conn
            .query(
                "SELECT url, discovered_at FROM url_nodes WHERE discovered_at IS NOT NULL ORDER BY discovered_at DESC LIMIT ?1",
                params![limit],
            )
            .await
            .context("read top freshness")?;
        let mut out = Vec::with_capacity(limit as usize);
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = row.get(0).context("read url from freshness")?;
                    let discovered_at: String = row.get(1).context("read discovered_at from freshness")?;
                    out.push((url, discovered_at));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate top freshness"),
            }
        }
        if out.is_empty() {
            return Ok(Vec::new());
        }
        // Normalize to [0, 1]: rank 0 (most recent) = 1.0, rank n-1 (oldest) = 0.0.
        let n = out.len();
        let normalized: Vec<(String, f64)> = out
            .into_iter()
            .enumerate()
            .map(|(rank, (url, _))| {
                let score = if n <= 1 {
                    1.0
                } else {
                    1.0 - (rank as f64 / (n - 1) as f64)
                };
                (url, score)
            })
            .collect();
        Ok(normalized)
    }
}
