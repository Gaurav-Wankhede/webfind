//! Live web index: query public search engines, fuse their ranked lists with
//! reciprocal rank fusion, and return ranked evidence the crawler can fetch
//! and persist into the graph store.
//!
//! The orchestrator never fails as a whole: engines are fallible individually,
//! and every failure is reported in [`Outcome::reports`] so callers can
//! surface degradation honestly instead of silently dropping results.

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

pub mod client;
pub mod engines;
pub mod rrf;
pub mod util;

pub use client::Client;
pub use engines::{
    arxiv, bing, bing_news, crates_io, ddg, devdocs, github_code, hn, lobsters, marginalia, mdn,
    mojeek, semantic_scholar, stackoverflow, wikipedia,
};
pub use rrf::{FusedHit, aggregate_fused, normalize_rrf_scores, reciprocal_rank_fusion};
pub use util::{
    aggregation_key, canonical_url, decode_bing_tracker_url, decode_ddg_redirect_url,
    extract_keywords, keyword_match_count, normalize_result_url, parse_date_from_snippet,
    positional_relevance,
};

/// Errors produced by a single engine request.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The engine answered with a non-success status other than 403/429.
    #[error("engine returned HTTP {0}")]
    HttpStatus(u16),
    /// The engine answered with an anti-bot challenge (403/429 or a challenge body).
    #[error("request blocked by an anti-bot challenge")]
    Blocked,
    /// The request exceeded its deadline.
    #[error("request timed out after {0} ms")]
    Timeout(u64),
    /// A transport-level failure (DNS, TLS, connection, body read).
    #[error("transport error: {0}")]
    Transport(#[from] reqwest::Error),
    /// The engine returned a body that could not be parsed.
    #[error("failed to parse engine response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Per-request options shared by every engine.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Maximum results requested from each engine.
    pub max_results: usize,
    /// Per-request HTTP timeout in milliseconds.
    pub timeout_ms: u64,
    /// Preferred result language (e.g. "en"); engines map it to their own params.
    pub language: Option<String>,
    /// Preferred result country (e.g. "us"); engines map it to their own params.
    pub country: Option<String>,
    /// Maximum number of engines queried concurrently.
    pub max_concurrency: usize,
    /// RRF constant; larger values dampen the influence of top ranks.
    pub rrf_k: u32,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            max_results: 10,
            timeout_ms: 10_000,
            language: None,
            country: None,
            max_concurrency: 12,
            rrf_k: 60,
        }
    }
}

/// One raw result returned by a single engine.
#[derive(Debug, Clone)]
pub struct Hit {
    /// Destination URL (engine redirects already resolved).
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub published_at: Option<DateTime<Utc>>,
    /// Positional relevance within the engine's own list: 1.0 (best) → 0.0.
    pub relevance_score: f64,
    /// Name of the engine that produced this hit.
    pub engine: &'static str,
}

/// Outcome of one engine request: either hits or a labeled failure.
#[derive(Debug)]
pub struct EngineReport {
    pub engine: &'static str,
    pub hits: Vec<Hit>,
    pub error: Option<Error>,
    pub latency_ms: u64,
}

/// Aggregate result of a live search across all engines.
#[derive(Debug)]
pub struct Outcome {
    pub query: String,
    /// RRF-fused results, aggregated into concept groups, best first.
    pub fused: Vec<FusedHit>,
    /// Per-engine outcomes, including failures.
    pub reports: Vec<EngineReport>,
    pub total_engines: usize,
    pub engines_ok: usize,
    pub engines_failed: usize,
    pub latency_ms: u64,
}

/// A search-engine adapter.
///
/// Implementations are stateless: user-agent rotation and retry-on-block live
/// in [`Client`], so a single `Arc<dyn Engine>` is safe to share across
/// concurrent searches.
#[async_trait]
pub trait Engine: Send + Sync {
    /// Stable engine identifier, surfaced in hits and reports.
    fn name(&self) -> &'static str;

    /// Whether this engine should be queried for the given query. Defaults to `true`.
    fn should_query(&self, _query: &str) -> bool {
        true
    }

    /// Search the engine and return its ranked hits.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Blocked`] when the engine answers with an anti-bot
    /// challenge, [`Error::Timeout`] on deadline expiry, and
    /// [`Error::HttpStatus`] for other non-success responses.
    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error>;
}

/// Orchestrator: fans a query out to every engine, fuses the ranked lists with
/// RRF, and reports per-engine success/failure so callers can surface
/// degradation honestly.
pub struct LiveIndex {
    engines: Vec<Arc<dyn Engine>>,
}

impl LiveIndex {
    /// Build an index with the default engine set.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] when the shared HTTP client cannot be built.
    pub fn new() -> Result<Self, Error> {
        let client = Client::new()?;
        let engines = engines::default_engines(client);
        Ok(Self { engines })
    }

    /// Build an index with a caller-supplied engine set.
    #[must_use]
    pub fn with_engines(engines: Vec<Arc<dyn Engine>>) -> Self {
        Self { engines }
    }

    /// Query every engine concurrently and fuse the results.
    ///
    /// # Panics
    ///
    /// Panics if the concurrency semaphore is closed; the semaphore is owned by
    /// this method and never closed, so this is unreachable.
    pub async fn search(&self, query: &str, opts: &EngineOptions) -> Outcome {
        let start = Instant::now();
        let active_engines: Vec<Arc<dyn Engine>> = self
            .engines
            .iter()
            .filter(|e| e.should_query(query))
            .cloned()
            .collect();
        let queried_count = active_engines.len();
        let max_permits = opts.max_concurrency.max(queried_count).max(1);
        let semaphore = Arc::new(Semaphore::new(max_permits));
        let mut futures = Vec::with_capacity(queried_count);

        for engine in active_engines {
            let semaphore = Arc::clone(&semaphore);
            let query = query.to_string();
            let opts = opts.clone();
            futures.push(async move {
                let _permit = semaphore
                    .acquire_owned()
                    .await
                    .expect("semaphore is never closed");
                let engine_start = Instant::now();
                let result = engine.search(&query, &opts).await;
                let latency_ms =
                    u64::try_from(engine_start.elapsed().as_millis()).unwrap_or(u64::MAX);
                match result {
                    Ok(hits) => EngineReport {
                        engine: engine.name(),
                        hits,
                        error: None,
                        latency_ms,
                    },
                    Err(error) => EngineReport {
                        engine: engine.name(),
                        hits: Vec::new(),
                        error: Some(error),
                        latency_ms,
                    },
                }
            });
        }

        let mut unordered = FuturesUnordered::new();
        for f in futures {
            unordered.push(f);
        }

        let mut reports = Vec::with_capacity(self.engines.len());
        let mut total_hits_seen = 0;
        let target_hits = opts.max_results.saturating_mul(2).max(10);
        let min_engines = 3.min(self.engines.len());
        let max_duration = std::time::Duration::from_millis(opts.timeout_ms.max(1000));

        loop {
            if start.elapsed() >= max_duration {
                tracing::debug!(
                    "LiveIndex deadline reached ({}ms), proceeding with collected results",
                    start.elapsed().as_millis()
                );
                break;
            }

            let remaining = max_duration.saturating_sub(start.elapsed());
            match tokio::time::timeout(remaining, unordered.next()).await {
                Ok(Some(report)) => {
                    total_hits_seen += report.hits.len();
                    reports.push(report);

                    // Speculative early return: if we already received responses from key engines
                    // and accumulated ample candidate hits, do not stall on slow/hanging endpoints.
                    if reports.len() >= min_engines
                        && total_hits_seen >= target_hits
                        && start.elapsed().as_millis() >= 750
                    {
                        tracing::debug!(
                            "LiveIndex early completion: got {} hits from {} engines in {}ms",
                            total_hits_seen,
                            reports.len(),
                            start.elapsed().as_millis()
                        );
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => {
                    tracing::debug!(
                        "LiveIndex timeout reached across remaining engines after {}ms",
                        start.elapsed().as_millis()
                    );
                    break;
                }
            }
        }

        let lists: Vec<(&[Hit], f64)> = reports
            .iter()
            .filter(|r| r.error.is_none())
            .map(|r| (r.hits.as_slice(), 1.0))
            .collect();
        let fused = reciprocal_rank_fusion(&lists, opts.rrf_k);
        // Aggregate per-URL scores into concept groups (host + first path
        // segment) so URL variants of the same page accumulate evidence, and
        // break score ties by query-keyword matches instead of URL order.
        let fused = aggregate_fused(fused, query);

        let engines_ok = reports.iter().filter(|r| r.error.is_none()).count();
        let latency_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

        Outcome {
            query: query.to_string(),
            fused,
            reports,
            total_engines: queried_count,
            engines_ok,
            engines_failed: queried_count.saturating_sub(engines_ok),
            latency_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubEngine {
        name: &'static str,
        hits: Vec<Hit>,
        fail: bool,
    }

    #[async_trait]
    impl Engine for StubEngine {
        fn name(&self) -> &'static str {
            self.name
        }

        async fn search(&self, _query: &str, _opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
            if self.fail {
                Err(Error::Blocked)
            } else {
                Ok(self.hits.clone())
            }
        }
    }

    fn hit(url: &str, engine: &'static str) -> Hit {
        Hit {
            url: url.to_string(),
            title: url.to_string(),
            snippet: String::new(),
            published_at: None,
            relevance_score: 1.0,
            engine,
        }
    }

    #[tokio::test]
    async fn fuses_engine_lists_and_reports_failures() {
        let engines: Vec<Arc<dyn Engine>> = vec![
            Arc::new(StubEngine {
                name: "stub-a",
                hits: vec![
                    hit("https://a.example", "stub-a"),
                    hit("https://b.example", "stub-a"),
                ],
                fail: false,
            }),
            Arc::new(StubEngine {
                name: "stub-b",
                hits: vec![hit("https://b.example", "stub-b")],
                fail: false,
            }),
            Arc::new(StubEngine {
                name: "stub-c",
                hits: Vec::new(),
                fail: true,
            }),
        ];
        let index = LiveIndex::with_engines(engines);
        let outcome = index.search("test query", &EngineOptions::default()).await;

        assert_eq!(outcome.total_engines, 3);
        assert_eq!(outcome.engines_ok, 2);
        assert_eq!(outcome.engines_failed, 1);
        assert_eq!(outcome.fused.len(), 2);
        assert_eq!(outcome.fused[0].url, "https://b.example");
        assert_eq!(outcome.fused[0].engine_count, 2);
        assert!(
            outcome
                .reports
                .iter()
                .any(|r| r.engine == "stub-c" && r.error.is_some())
        );
    }

    #[tokio::test]
    async fn empty_engine_set_yields_empty_outcome() {
        let index = LiveIndex::with_engines(Vec::new());
        let outcome = index.search("test query", &EngineOptions::default()).await;
        assert!(outcome.fused.is_empty());
        assert_eq!(outcome.total_engines, 0);
        assert_eq!(outcome.engines_ok, 0);
    }

    #[tokio::test]
    async fn search_aggregates_url_variants_into_concept_groups() {
        let engines: Vec<Arc<dyn Engine>> = vec![
            Arc::new(StubEngine {
                name: "stub-a",
                hits: vec![Hit {
                    url: "https://tokio.rs/tokio/tutorial/async".to_string(),
                    title: "Tokio - An asynchronous Rust runtime".to_string(),
                    snippet: String::new(),
                    published_at: None,
                    relevance_score: 1.0,
                    engine: "stub-a",
                }],
                fail: false,
            }),
            Arc::new(StubEngine {
                name: "stub-b",
                hits: vec![Hit {
                    url: "https://tokio.rs/".to_string(),
                    title: "Tokio".to_string(),
                    snippet: String::new(),
                    published_at: None,
                    relevance_score: 1.0,
                    engine: "stub-b",
                }],
                fail: false,
            }),
        ];
        let index = LiveIndex::with_engines(engines);
        let outcome = index
            .search("rust async runtime tokio", &EngineOptions::default())
            .await;

        // Homepage + section page aggregate into one group; the section page
        // (4 keyword matches) represents it.
        assert_eq!(outcome.fused.len(), 1);
        assert_eq!(
            outcome.fused[0].url,
            "https://tokio.rs/tokio/tutorial/async"
        );
        assert_eq!(outcome.fused[0].engine_count, 2);
    }
}
