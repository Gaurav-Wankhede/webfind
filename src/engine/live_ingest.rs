//! Ingest live search results into the durable Turso graph.
//!
//! # The defect this fixes
//!
//! `webfind search --live` fans out to 15 search engines, RRF-fuses the hits
//! into [`FusedHit`]s, returns them — and **drops them**. [`FusedHit`] carries
//! only `url`, `title`, `snippet`, `published_at`, `score`, `engine_count`, and
//! `engines`; there is no content and no persistence call anywhere in the live
//! path. So:
//!
//! - live hits never became `url_nodes`
//! - their links never became `link_edges`
//! - no embeddings were computed for them
//! - the graph never compounded from live search
//!
//! The README's "graph memory compounds" claim was therefore false for the
//! `--live` path: only `crawl` persisted anything.
//!
//! # What this module does
//!
//! After fusion, every live hit above a score threshold is recorded as a
//! [`UrlNode`] with [`DiscoverySource::LiveSearch`] and enqueued as a crawl job.
//! A background worker then fetches the page, extracts content, and computes
//! embeddings — so a live search progressively enriches the graph.
//!
//! Ingestion is **non-blocking**: the search response is not delayed waiting for
//! fetches. Enqueue and return.

use std::collections::HashSet;

use anyhow::Result;

use crate::engine::crawl_graph::{CrawlGraphStore, DiscoverySource, UrlNode};
use crate::engine::util::extract_domain;
use crate::engine::web_index::{FusedHit, canonical_url};

/// Minimum fused score for a live hit to be worth persisting.
///
/// **This operates on RAW RRF scores, not normalized ones.** `outcome.fused`
/// from the live index carries raw Reciprocal Rank Fusion fractions
/// (`weight / (k + rank)` with k=60), which top out around 0.033 for a single
/// engine at rank 0 and grow with multi-engine consensus. `normalize_rrf_scores`
/// is applied later, inside `fuse_index_and_live`, and is not visible here.
///
/// So the floor is expressed in raw-RRF units: a hit must have been ranked
/// highly by at least one engine (`1/61 ≈ 0.0164`) or appeared in several.
/// A threshold in the normalized `[0.40, 1.0]` range would reject every hit.
pub const MIN_INGEST_SCORE: f64 = 0.015;

/// Maximum live hits ingested per search, regardless of how many were returned.
///
/// A hard cap keeps a single broad query from flooding the crawl queue.
pub const MAX_INGEST_PER_SEARCH: usize = 25;

/// What an ingest pass did, for logging and tests.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct IngestReport {
    /// Live hits considered (after the score floor).
    pub candidates: usize,
    /// URLs newly recorded as graph nodes.
    pub nodes_recorded: usize,
    /// URLs skipped because they were already in the graph.
    pub already_present: usize,
    /// URLs skipped because their canonical form duplicated an earlier hit.
    pub duplicates: usize,
    /// Crawl jobs enqueued for background enrichment.
    pub jobs_enqueued: usize,
    /// URLs that could not be ingested (bad URL, store error).
    pub failed: usize,
}

impl IngestReport {
    /// Total live hits that entered the graph as new nodes.
    #[must_use]
    pub fn ingested(&self) -> usize {
        self.nodes_recorded
    }
}

/// Persist live fused hits into the graph and enqueue them for enrichment.
///
/// Returns an [`IngestReport`] describing what happened. Errors on individual
/// URLs are counted in `failed` rather than aborting the pass — one malformed
/// URL must not lose the whole result set.
///
/// This function does **not** fetch anything; it records nodes and enqueues
/// jobs. The background worker performs the network work.
pub async fn ingest_fused_hits(
    store: &dyn CrawlGraphStore,
    hits: &[FusedHit],
) -> Result<IngestReport> {
    let mut report = IngestReport::default();
    let mut seen: HashSet<String> = HashSet::new();

    for hit in hits {
        // Score floor: skip low-confidence long-tail results.
        if hit.score < MIN_INGEST_SCORE {
            continue;
        }
        if report.candidates >= MAX_INGEST_PER_SEARCH {
            break;
        }

        // Dedupe within this batch by canonical URL, so tracking-parameter
        // variants of the same page do not each become a node.
        let canonical = canonical_url(&hit.url);
        if !seen.insert(canonical.clone()) {
            report.duplicates += 1;
            continue;
        }

        // Only http(s) URLs are fetchable.
        let Ok(parsed) = url::Url::parse(&hit.url) else {
            report.failed += 1;
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https") {
            report.failed += 1;
            continue;
        }

        report.candidates += 1;

        // Already in the graph? Then there is nothing new to record, but the
        // hit still proved its relevance — refresh the priority so a page that
        // keeps surfacing ranks higher.
        if let Some(mut existing) = store.get_url(&hit.url).await {
            existing.priority = existing.priority.max(hit.score as f32);
            store.record_url(existing).await;
            report.already_present += 1;
            continue;
        }

        let node = UrlNode {
            url: hit.url.clone(),
            domain: extract_domain(&hit.url).unwrap_or_default(),
            source: DiscoverySource::LiveSearch,
            depth: 0,
            priority: hit.score as f32,
            lastmod: hit.published_at,
            changefreq: None,
            discovered_at: chrono::Utc::now(),
            crawled: false,
        };
        store.record_url(node).await;
        report.nodes_recorded += 1;

        // Enqueue for background fetch + extract + embed. A failure here does
        // not invalidate the node we just recorded.
        match store.enqueue_crawl_job(&hit.url).await {
            Ok(_) => report.jobs_enqueued += 1,
            Err(e) => {
                tracing::debug!("failed to enqueue live hit {}: {}", hit.url, e);
                report.failed += 1;
            }
        }
    }

    if report.nodes_recorded > 0 || report.jobs_enqueued > 0 {
        tracing::info!(
            candidates = report.candidates,
            nodes = report.nodes_recorded,
            enqueued = report.jobs_enqueued,
            already_present = report.already_present,
            duplicates = report.duplicates,
            "ingested live search hits into the graph"
        );
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::crawl_graph::InMemoryCrawlGraph;
    use std::sync::Arc;

    fn hit(url: &str, score: f64) -> FusedHit {
        FusedHit {
            url: url.to_string(),
            title: format!("Title for {url}"),
            snippet: "snippet".to_string(),
            published_at: None,
            score,
            engine_count: 1,
            engines: vec!["ddg"],
        }
    }

    #[tokio::test]
    async fn ingests_hits_above_the_score_floor() {
        let store = InMemoryCrawlGraph::new();
        let hits = vec![
            hit("https://example.com/a", 0.033),
            hit("https://example.com/b", 0.0164),
        ];
        let report = ingest_fused_hits(&store, &hits).await.unwrap();

        assert_eq!(report.nodes_recorded, 2);
        assert_eq!(report.jobs_enqueued, 2);
        assert_eq!(store.get_urls().await.len(), 2);

        let node = store.get_url("https://example.com/a").await.unwrap();
        assert_eq!(node.source, DiscoverySource::LiveSearch);
    }

    /// The floor must be expressed in RAW RRF units.
    ///
    /// Regression guard: an earlier version used 0.55, which is a *normalized*
    /// scale. Because `outcome.fused` carries raw fractions (~0.033 max), that
    /// threshold silently rejected every hit and live search never persisted
    /// anything. This pins the scale so the bug cannot return.
    #[tokio::test]
    async fn score_floor_accepts_a_top_ranked_raw_rrf_hit() {
        let store = InMemoryCrawlGraph::new();
        // A single engine ranking a URL first yields exactly 1/(60+1).
        let top_ranked = 1.0 / 61.0;
        let report = ingest_fused_hits(&store, &[hit("https://example.com/top", top_ranked)])
            .await
            .unwrap();
        assert_eq!(
            report.nodes_recorded, 1,
            "a rank-0 hit from one engine (raw RRF {top_ranked}) must be ingested"
        );
    }

    #[tokio::test]
    async fn skips_hits_below_the_score_floor() {
        let store = InMemoryCrawlGraph::new();
        // Below a single engine's rank-0 contribution: deep long-tail noise.
        let hits = vec![hit("https://example.com/low", 0.001)];
        let report = ingest_fused_hits(&store, &hits).await.unwrap();

        assert_eq!(report.candidates, 0);
        assert_eq!(report.nodes_recorded, 0);
        assert!(store.get_urls().await.is_empty());
    }

    #[tokio::test]
    async fn dedupes_tracking_parameter_variants() {
        let store = InMemoryCrawlGraph::new();
        let hits = vec![
            hit("https://example.com/page", 0.033),
            hit("https://example.com/page?utm_source=x", 0.02),
        ];
        let report = ingest_fused_hits(&store, &hits).await.unwrap();

        assert_eq!(report.nodes_recorded, 1, "canonical dedupe must collapse these");
        assert_eq!(report.duplicates, 1);
        assert_eq!(store.get_urls().await.len(), 1);
    }

    #[tokio::test]
    async fn second_pass_marks_hits_as_already_present() {
        let store = InMemoryCrawlGraph::new();
        let hits = vec![hit("https://example.com/a", 0.033)];

        let first = ingest_fused_hits(&store, &hits).await.unwrap();
        assert_eq!(first.nodes_recorded, 1);
        assert_eq!(first.already_present, 0);

        let second = ingest_fused_hits(&store, &hits).await.unwrap();
        assert_eq!(second.nodes_recorded, 0, "must not duplicate the node");
        assert_eq!(second.already_present, 1);
        assert_eq!(store.get_urls().await.len(), 1);
    }

    #[tokio::test]
    async fn rejects_non_http_urls() {
        let store = InMemoryCrawlGraph::new();
        let hits = vec![
            hit("ftp://example.com/file", 0.033),
            hit("not a url at all", 0.033),
        ];
        let report = ingest_fused_hits(&store, &hits).await.unwrap();

        assert_eq!(report.nodes_recorded, 0);
        assert_eq!(report.failed, 2);
        assert!(store.get_urls().await.is_empty());
    }

    #[tokio::test]
    async fn caps_the_number_of_hits_per_search() {
        let store = InMemoryCrawlGraph::new();
        let hits: Vec<FusedHit> = (0..100)
            .map(|i| hit(&format!("https://example.com/p{i}"), 0.033))
            .collect();
        let report = ingest_fused_hits(&store, &hits).await.unwrap();

        assert_eq!(
            report.nodes_recorded, MAX_INGEST_PER_SEARCH,
            "a broad query must not flood the crawl queue"
        );
    }

    #[tokio::test]
    async fn refreshes_priority_for_a_recurring_hit() {
        let store = InMemoryCrawlGraph::new();
        let _ = ingest_fused_hits(&store, &[hit("https://example.com/a", 0.02)])
            .await
            .unwrap();
        let _ = ingest_fused_hits(&store, &[hit("https://example.com/a", 0.05)])
            .await
            .unwrap();

        let node = store.get_url("https://example.com/a").await.unwrap();
        assert!(
            (node.priority - 0.05).abs() < 1e-6,
            "a page that keeps surfacing must rank higher, got {}",
            node.priority
        );
    }

    #[tokio::test]
    async fn empty_input_is_a_noop() {
        let store = InMemoryCrawlGraph::new();
        let report = ingest_fused_hits(&store, &[]).await.unwrap();
        assert_eq!(report, IngestReport::default());
    }

    #[tokio::test]
    async fn works_through_a_trait_object() {
        // The production path passes `&dyn CrawlGraphStore`, so exercise it.
        let store: Arc<dyn CrawlGraphStore> = Arc::new(InMemoryCrawlGraph::new());
        let report = ingest_fused_hits(store.as_ref(), &[hit("https://example.com/a", 0.033)])
            .await
            .unwrap();
        assert_eq!(report.nodes_recorded, 1);
    }
}
