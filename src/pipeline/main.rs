//! Main Pipeline: Query → Report orchestration.
//!
//! Runs the complete embedded pipeline:
//!   Query → Route → Retrieve → Fetch/Extract → Synthesize → Export
//!
//! Storage is the embedded Turso/libSQL store (`storage::turso_store::TursoStore`),
//! which already provides FTS5 lexical search, DiskANN vector search, the crawl
//! link graph, PageRank, and cache tables. No external servers are required.

use anyhow::Result;
use std::sync::{Arc, Mutex};

use crate::engine::bm25_index::Bm25Index;
use crate::engine::embedder::Embedder;
#[cfg(feature = "embed")]
use crate::engine::embedder::FastembedEmbedder;
use crate::engine::query_router::route_query;
use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::source_tier::SourceTier;
use crate::storage::turso_store::{TursoSearchHit, TursoStore};

/// Pipeline configuration.
#[derive(Debug, Clone)]
pub struct PipelineConfig {
    /// Number of URLs to fetch concurrently when extracting examples.
    pub max_concurrent_fetches: usize,
    /// Per-fetch timeout budget in seconds (advisory).
    pub fetch_timeout_secs: u64,
    /// Run the match-based example extractor.
    pub enable_explanations: bool,
    /// Cap on extracted examples per page.
    pub max_examples_per_type: usize,
    /// Path to the Turso database file.
    pub turso_path: String,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_concurrent_fetches: 10,
            fetch_timeout_secs: 30,
            enable_explanations: true,
            max_examples_per_type: 5,
            turso_path: String::new(),
        }
    }
}

/// Fetch-mode shaping passed through to the report renderer.
#[derive(Debug, Clone)]
pub struct FetchConfig {
    pub timeout_secs: u64,
    pub max_concurrent: usize,
    pub extract_examples: bool,
}

/// A retrieved document with its source tier and extracted examples.
#[derive(Debug, Clone)]
pub struct PipelineHit {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub domain: String,
    pub source_tier: SourceTier,
    pub bm25_score: f64,
    pub vector_score: f64,
    pub graph_score: f64,
    pub final_score: f64,
    pub examples: usize,
}

/// The synthesized result of a pipeline run.
#[derive(Debug, Clone)]
pub struct PipelineReport {
    pub query: String,
    pub intent: String,
    pub strategy: String,
    pub confidence: f32,
    pub hits: Vec<PipelineHit>,
    pub total_examples: usize,
    pub latency_ms: u128,
}

/// Shared pipeline state. Cheap to clone; all heavy handles are `Arc`.
pub struct PipelineState {
    pub store: Arc<TursoStore>,
    pub config: PipelineConfig,
    pub semaphore: Arc<tokio::sync::Semaphore>,
    /// In-process embedding model (EmbeddingGemma2 slot). Lazily initialised.
    pub embedder: Arc<Mutex<Option<Arc<dyn Embedder>>>>,
    /// Corpus-fitted BM25 index backing the sparse lexical channel.
    pub bm25_index: Arc<tokio::sync::Mutex<Option<Arc<Bm25Index>>>>,
}

impl PipelineState {
    pub async fn new(turso_path: &str, config: PipelineConfig) -> Result<Self> {
        let store = TursoStore::new(turso_path).await?;
        // Keep derived indexes current so vector + graph signals are populated.
        let graph_version = store.graph_version().await;
        if store.fts_version().await != graph_version {
            store.rebuild_fts().await?;
        }
        if store.pagerank_version().await != graph_version {
            let _ = store.compute_pagerank(20, 0.85).await?;
        }
        Ok(Self {
            store: Arc::new(store),
            semaphore: Arc::new(tokio::sync::Semaphore::new(config.max_concurrent_fetches)),
            config,
            embedder: Arc::new(Mutex::new(None)),
            bm25_index: Arc::new(tokio::sync::Mutex::new(None)),
        })
    }
}

/// Main pipeline execution.
pub struct Pipeline {
    state: Arc<PipelineState>,
}

impl Pipeline {
    pub fn new(state: Arc<PipelineState>) -> Self {
        Self { state }
    }

    /// Execute the complete pipeline: Query → Report.
    pub async fn execute(&self, query: &str) -> Result<PipelineReport> {
        let started = std::time::Instant::now();

        // 1. Route the query (deterministic → embedding → language → fallback).
        let embedding = self.get_query_embedding(query).await?;
        let route = route_query(query, embedding.as_deref());

        // 2. Retrieve from the embedded store (BM25 + vector + PageRank).
        let hits = self.retrieve(query, embedding.as_deref(), route.confidence).await?;

        // 3. Match-based example extraction (only when examples are enabled).
        let total_examples = if self.state.config.enable_explanations {
            self.fetch_and_extract(&hits).await?
        } else {
            0
        };

        Ok(PipelineReport {
            query: query.to_string(),
            intent: format!("{:?}", route.intent),
            strategy: format!("{:?}", route.strategy),
            confidence: route.confidence,
            hits,
            total_examples,
            latency_ms: started.elapsed().as_millis(),
        })
    }

    /// Embed the query with the in-process model, caching the result in Turso.
    ///
    /// Returns `None` when the neural backend is unavailable — either because
    /// the `embed` feature was not compiled in, or because the model failed to
    /// load. Callers treat `None` as "no vector signal", not as an error: the
    /// ranker falls back to lexical/BM25, which is exactly what it does on a
    /// cold index anyway.
    async fn get_query_embedding(&self, query: &str) -> Result<Option<Vec<f32>>> {
        #[cfg(not(feature = "embed"))]
        {
            use std::sync::Once;
            static WARNED: Once = Once::new();
            WARNED.call_once(|| {
                tracing::warn!(
                    "the `embed` feature is not enabled: dense vector search is \
                     unavailable, so ranking falls back to lexical/BM25 signals. \
                     Rebuild with `--features embed` to enable it."
                );
            });
            let _ = query;
            Ok(None)
        }

        #[cfg(feature = "embed")]
        {
            self.init_embedder().await;
            let embedding = {
                let guard = self.state.embedder.lock().unwrap();
                match guard.as_ref() {
                    Some(m) => match m.embed(&[query]) {
                        Ok(v) => v.into_iter().next(),
                        Err(e) => {
                            tracing::warn!("embedding failed: {e}");
                            None
                        }
                    },
                    None => None,
                }
            };
            if let Some(e) = &embedding {
                self.state.store.cache_embedding(query, e, "embeddinggemma2").await?;
            }
            Ok(embedding)
        }
    }

    /// Ensure the in-process embedder is loaded (embed feature only).
    #[cfg(feature = "embed")]
    async fn init_embedder(&self) {
        let needs_init = self
            .state
            .embedder
            .lock()
            .map(|g| g.is_none())
            .unwrap_or(true);
        if !needs_init {
            return;
        }
        let loaded = match FastembedEmbedder::new() {
            Ok(e) => Some(Arc::new(e)),
            Err(e) => {
                tracing::warn!("embedding model unavailable: {e}");
                None
            }
        };
        if let Ok(mut guard) = self.state.embedder.lock() {
            if guard.is_none() {
                *guard = loaded;
            }
        }
    }

    /// Retrieve documents from the embedded store and score them with the
    /// ranker's source-tier precision boost.
    async fn retrieve(
        &self,
        query: &str,
        embedding: Option<&[f32]>,
        _confidence: f32,
    ) -> Result<Vec<PipelineHit>> {
        let limit = self.state.config.max_concurrent_fetches.max(1) * 2;
        let store_hits: Vec<TursoSearchHit> = self.state.store.search(query, embedding, limit).await?;

        let mut out = Vec::with_capacity(store_hits.len());
        for h in store_hits {
            let tier = crate::engine::source_tier::classify(&h.url);
            let domain = url::Url::parse(&h.url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_default();
            let boost = tier.precision();
            let bm25 = h.bm25_score.unwrap_or(0.0);
            let vector = h.vector_score.unwrap_or(0.0);
            let graph = h.graph_score.unwrap_or(0.0);
            // Lexical relevance modulated by source precision, mirroring the
            // ranker's source_precision_boost so official sources win ties.
            let base = bm25.max(vector) * (0.5 + 0.5 * (h.score / h.score.max(1.0)));
            let final_score = (base * (0.6 + 0.4 * graph)) * boost;
            out.push(PipelineHit {
                url: h.url,
                title: h.title,
                snippet: h.excerpt,
                domain,
                source_tier: tier,
                bm25_score: bm25,
                vector_score: vector,
                graph_score: graph,
                final_score,
                examples: 0,
            });
        }
        out.sort_by(|a, b| b.final_score.partial_cmp(&a.final_score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(out)
    }

    /// Fetch each retrieved URL and run the match-based extractor.
    async fn fetch_and_extract(&self, hits: &[PipelineHit]) -> Result<usize> {
        let urls: Vec<String> = hits
            .iter()
            .map(|h| h.url.clone())
            .filter(|u| !u.trim().is_empty())
            .take(self.state.config.max_concurrent_fetches)
            .collect();
        if urls.is_empty() {
            return Ok(0);
        }

        let mut handles = Vec::with_capacity(urls.len());
        for url in urls {
            let permit = self.state.semaphore.clone().acquire_owned().await?;
            let budget = self.state.config.max_examples_per_type.max(1);
            handles.push(tokio::spawn(async move {
                let _permit = permit;
                let result = match crate::engine::fetcher::Fetcher::new() {
                    Ok(f) => f.fetch_url(&url).await,
                    Err(e) => Err(anyhow::anyhow!("fetcher init failed: {e}")),
                };
                (url, result, budget)
            }));
        }

        let mut total = 0usize;
        for handle in handles {
            let (url, outcome, budget) = match handle.await {
                Ok(v) => v,
                Err(_) => continue,
            };
            let content = match outcome {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!("fetch failed for {url}: {e}");
                    continue;
                }
            };
            let mut examples = crate::pipeline::example_extract::extract_examples(&content);
            for ex in &mut examples {
                ex.source_tier = crate::engine::source_tier::classify(&url);
            }
            total += examples.len().min(budget);
        }
        Ok(total)
    }
}

/// Fetch pipeline: batched URL fetch with validity summarisation.
pub struct FetchPipeline;

impl FetchPipeline {
    /// Fetch many URLs concurrently and return the successfully extracted ones.
    pub async fn fetch_all(urls: &[String]) -> Vec<crate::schema::content::StructuredContent> {
        let mut out = Vec::new();
        for url in urls {
            match crate::engine::fetcher::Fetcher::new() {
                Ok(f) => {
                    if let Ok(c) = f.fetch_url(url).await {
                        if c.is_valid_content {
                            out.push(c);
                        }
                    }
                }
                Err(e) => tracing::debug!("fetcher init failed for {url}: {e}"),
            }
        }
        out
    }

    /// Keep only the fetches that produced usable content.
    pub fn filter_valid(results: Vec<crate::schema::content::StructuredContent>) -> Vec<crate::schema::content::StructuredContent> {
        results.into_iter().filter(|c| c.is_valid_content).collect()
    }

    /// Summarise a batch: (success, failure, per-URL errors).
    pub fn summarize(
        results: &[crate::schema::content::StructuredContent],
    ) -> (usize, usize, Vec<(String, String)>) {
        let success = results.iter().filter(|c| c.is_valid_content).count();
        let failed: Vec<_> = results.iter().filter(|c| !c.is_valid_content).collect();
        let errors: Vec<(String, String)> = failed
            .iter()
            .map(|c| (c.url.clone(), "fetch failed or no usable content".to_string()))
            .collect();
        (success, failed.len(), errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_config_defaults_are_sane() {
        let c = PipelineConfig::default();
        assert!(c.max_concurrent_fetches > 0);
        assert!(c.max_examples_per_type > 0);
        assert!(c.enable_explanations);
    }

    #[test]
    fn fetch_pipeline_summarize_counts_validity() {
        let mk = |valid: bool| crate::schema::content::StructuredContent {
            url: "https://x/".into(),
            is_valid_content: valid,
            ..tests_support::minimal()
        };
        let results = vec![mk(true), mk(true), mk(false)];
        let (ok, bad, errors) = FetchPipeline::summarize(&results);
        assert_eq!(ok, 2);
        assert_eq!(bad, 1);
        assert_eq!(errors.len(), 1);
    }
}

#[cfg(test)]
mod tests_support {
    use crate::schema::content::StructuredContent;

    /// Minimal valid `StructuredContent` for tests (no `Default` impl exists).
    pub fn minimal() -> StructuredContent {
        StructuredContent {
            url: String::new(),
            final_url: String::new(),
            status_code: 200,
            title: String::new(),
            description: None,
            canonical_url: None,
            language: "en".into(),
            language_confidence: 1.0,
            published_at: None,
            modified_at: None,
            author: None,
            site_name: None,
            content_text: String::new(),
            content_html: String::new(),
            content_markdown: String::new(),
            excerpt: String::new(),
            word_count: 0,
            char_count: 0,
            sentence_count: 0,
            reading_time_seconds: 0,
            reading_ease: 0.0,
            grade_level: 0.0,
            keywords: Vec::new(),
            open_graph: None,
            twitter_card: None,
            json_ld: Vec::new(),
            schema_type: None,
            images: Vec::new(),
            internal_links: Vec::new(),
            external_links: Vec::new(),
            favicon: None,
            rss_url: None,
            normalized_text: String::new(),
            fetched_at: chrono::DateTime::UNIX_EPOCH,
            fetch_duration_ms: 0,
            html_size_bytes: 0,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            content_type: "text/html".into(),
            content_type_header: "text/html".into(),
            is_paywalled: false,
            is_valid_content: false,
            entities: Default::default(),
        }
    }
}
