use std::collections::HashMap;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;

use webfind::cli::GraphStoreArg;
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::engine::embedder::{DummyEmbedder, Embedder, FastembedEmbedder};
use webfind::engine::ranker::Ranker;
use webfind::engine::web_index::{
    EngineOptions, FusedHit, Hit, LiveIndex, canonical_url, normalize_rrf_scores,
    positional_relevance, reciprocal_rank_fusion,
};
use webfind::report::format_response;
use webfind::schema::request::{OutputFormat, SearchDepth};
use webfind::schema::response::{
    ContentBlock, IndexFreshness, ScoreBreakdown, SearchMetadata, SearchResponse, SearchResult,
};
use webfind::storage::turso_store::{TursoSearchHit, TursoStore};

/// Convert a store hit into a renderable result.
fn store_hit_to_result(
    hit: &TursoSearchHit,
    rank: u32,
    score: f64,
    include_content: bool,
) -> SearchResult {
    let domain = url::Url::parse(&hit.url)
        .map(|u| u.host_str().unwrap_or("").to_string())
        .unwrap_or_default();
    SearchResult {
        rank,
        url: hit.url.clone(),
        title: hit.title.clone(),
        snippet: hit.excerpt.clone(),
        domain,
        published_at: None,
        modified_at: None,
        crawled_at: Utc::now(),
        author: None,
        site_name: None,
        score,
        scores: ScoreBreakdown {
            bm25: hit.bm25_score,
            vector: hit.vector_score,
            graph: hit.graph_score,
            freshness: None,
            quality: None,
            final_score: score,
        },
        content: include_content.then(|| {
            let wc = hit.excerpt.split_whitespace().count() as u32;
            ContentBlock {
                text: hit.excerpt.clone(),
                excerpt: hit.excerpt.clone(),
                word_count: wc,
                reading_time_seconds: webfind::engine::fetcher::estimate_reading_time(wc),
                html: None,
                markdown: None,
            }
        }),
        keywords: None,
        metrics: None,
        favicon: None,
        thumbnail: None,
        language: "en".to_string(),
        content_type: "text/html".to_string(),
    }
}

/// Convert a live-only fused hit into a renderable result.
///
/// Freshness is derived from `published_at` when available (falls back to
/// "just crawled"). Quality is estimated from snippet word density. Both
/// signals are surfaced in `ScoreBreakdown` so the report doesn't show dashes.
///
/// **Score contract** — only signals with real evidence are emitted:
/// - `freshness`: set iff `published_at` is `Some`; `None` for unknown-date pages.
/// - `quality`: `None` for live-only hits (we have only a snippet, not the full page).
/// - `final_score`: weighted average over *present* signals only; the normalized
///   RRF rank-score (always ∈ [0,1]) is the anchor, everything else is additive
///   when evidence exists. Defaulting absent signals to 1.0 is forbidden.
fn live_fused_to_result(fused: &FusedHit, rank: u32, include_content: bool) -> SearchResult {
    let domain = url::Url::parse(&fused.url)
        .map(|u| u.host_str().unwrap_or("").to_string())
        .unwrap_or_default();

    // Freshness: ONLY computed when the engine surfaced a publication date.
    // An unknown-date page must not receive a freshness bonus — None propagates
    // through the score formula so the weight is simply not applied.
    let freshness: Option<f64> = fused.published_at.map(Ranker::freshness_score);

    // Quality: None for live-only hits — we have only a snippet, not the fetched
    // body, so we cannot compute readability metrics honestly. Do not fake a score.
    let quality: Option<f64> = None;

    // Engine fusion credibility bonus: 0..0.10 added on top of rank-score.
    // Capped so even a 4-engine result adds only +0.10.
    let fusion_bonus = (fused.engine_count.saturating_sub(1) as f64 / 3.0).min(1.0) * 0.10;

    // Weighted average of present signals.
    // Weights: rank-score 0.75 (always), freshness 0.25 (when known).
    // fusion_bonus is additive, then clamped.
    let final_score = match freshness {
        Some(f) => fused.score * 0.75 + f * 0.25 + fusion_bonus,
        None => fused.score + fusion_bonus,
    }
    .clamp(0.0, 1.0);

    SearchResult {
        rank,
        url: fused.url.clone(),
        title: fused.title.clone(),
        snippet: fused.snippet.clone(),
        domain,
        published_at: fused.published_at,
        modified_at: None,
        crawled_at: Utc::now(),
        author: None,
        site_name: None,
        score: final_score,
        scores: ScoreBreakdown {
            bm25: None,
            vector: None,
            graph: None,
            freshness,
            quality,
            final_score,
        },
        content: include_content.then(|| {
            let wc = fused.snippet.split_whitespace().count() as u32;
            ContentBlock {
                text: fused.snippet.clone(),
                excerpt: fused.snippet.clone(),
                word_count: wc,
                reading_time_seconds: webfind::engine::fetcher::estimate_reading_time(wc),
                html: None,
                markdown: None,
            }
        }),
        keywords: None,
        metrics: None,
        favicon: None,
        thumbnail: None,
        language: "en".to_string(),
        content_type: "text/html".to_string(),
    }
}

/// Fuse store hits with live fused hits into a single ranked list.
///
/// The store list and the live fused list are each treated as one ranked list;
/// [`reciprocal_rank_fusion`] scores every URL by `sum(weight / (k + rank))`
/// across both, so a URL both lists rank highly wins over a URL only one list
/// ranks first. The live list carries a 2x weight: `--live` explicitly asks
/// for fresh results, and the store's hybrid ranking can surface stale
/// pagerank-driven pages (BM25=0) that would otherwise tie with relevant live
/// hits. Each fused entry carries the matching store hit when the URL already
/// exists in the store, so callers can enrich it with stored content.
fn fuse_index_and_live<'a>(
    store_hits: &'a [TursoSearchHit],
    live_fused: &[FusedHit],
) -> Vec<(FusedHit, Option<&'a TursoSearchHit>)> {
    let index_list: Vec<Hit> = store_hits
        .iter()
        .enumerate()
        .map(|(i, h)| Hit {
            url: h.url.clone(),
            title: h.title.clone(),
            snippet: h.excerpt.clone(),
            published_at: None,
            relevance_score: positional_relevance(i, store_hits.len()),
            engine: "index",
        })
        .collect();
    let live_list: Vec<Hit> = live_fused
        .iter()
        .enumerate()
        .map(|(i, f)| Hit {
            url: f.url.clone(),
            title: f.title.clone(),
            snippet: f.snippet.clone(),
            published_at: f.published_at,
            relevance_score: positional_relevance(i, live_fused.len()),
            engine: "live",
        })
        .collect();

    let raw = reciprocal_rank_fusion(&[(&index_list, 1.0), (&live_list, LIVE_FUSION_WEIGHT)], 60);
    // Normalize to [0.0, 1.0] so displayed scores are intuitive (top hit → ~1.0).
    let fused = normalize_rrf_scores(raw);

    let store_by_url: HashMap<String, &TursoSearchHit> = store_hits
        .iter()
        .map(|h| (canonical_url(&h.url), h))
        .collect();

    fused
        .into_iter()
        .map(|f| {
            let store_hit = store_by_url.get(&canonical_url(&f.url)).copied();
            (f, store_hit)
        })
        .collect()
}

/// Weight applied to the live list in the store-vs-live fusion.
///
/// `--live` explicitly asks for fresh results; the store's hybrid ranking can
/// surface stale pagerank-driven pages (BM25=0) that would otherwise tie with
/// relevant live hits. 2x keeps the hierarchy shared > live-only > store-only
/// while still letting a URL present in both lists win decisively.
const LIVE_FUSION_WEIGHT: f64 = 2.0;

/// Merge store hits with live search-engine results via RRF.
///
/// Handles three edge cases that previously produced empty responses:
///
/// 1. **Zero results** (engines blocked or too-niche query): retry with a
///    progressively relaxed query (drop trailing tokens) until at least one
///    engine returns a hit, or all retries are exhausted.
/// 2. **Sparse results** (< `limit`): fill up to `limit` with the best
///    store-only hits not already in the fused set.
/// 3. **Score display**: raw RRF fractions are normalized to [0,1] by
///    `fuse_index_and_live`; `live_fused_to_result` attaches freshness +
///    quality signals so the breakdown never shows placeholder dashes.
async fn merge_live_results(
    query: &str,
    store_hits: &[TursoSearchHit],
    limit: u32,
    include_content: bool,
    signals: &mut Vec<String>,
) -> anyhow::Result<Vec<SearchResult>> {
    let live_index = LiveIndex::new().context("build live search index")?;
    // Ask engines for 2× limit so we have room after dedup/fusing.
    let fetch = (limit.max(1) * 2) as usize;
    let opts = EngineOptions {
        max_results: fetch,
        timeout_ms: 3_500,
        ..EngineOptions::default()
    };

    // --- Pass 1: full query ---
    let mut outcome = live_index.search(query, &opts).await;
    log_engine_failures(&outcome);

    // --- Pass 2: relaxed query (drop last token) ---
    if outcome.fused.is_empty() {
        let relaxed = relax_query(query, 1);
        if !relaxed.is_empty() && relaxed != query {
            tracing::warn!("live: 0 results for full query, retrying with \"{}\"", relaxed);
            outcome = live_index.search(&relaxed, &opts).await;
            log_engine_failures(&outcome);
        }
    }

    // --- Pass 3: broadest 3-word query ---
    if outcome.fused.is_empty() {
        let broad = relax_query(query, query.split_whitespace().count().saturating_sub(3));
        if !broad.is_empty() && broad != query {
            tracing::warn!("live: still 0 results, retrying with \"{}\"", broad);
            outcome = live_index.search(&broad, &opts).await;
            log_engine_failures(&outcome);
        }
    }

    // If all passes yield nothing, return whatever the store has.
    if outcome.fused.is_empty() {
        tracing::warn!("live: all query passes returned 0 results, falling back to store-only");
        let fallback: Vec<&webfind::storage::turso_store::TursoSearchHit> = store_hits
            .iter()
            .take(limit.max(1) as usize)
            .collect();
        // Min-max normalize store scores to [0,1] so the display scale matches
        // live-fused results (store raw RRF fracs are ≈0.016-0.05, not [0,1]).
        let max_s = fallback.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = fallback.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = (max_s - min_s).max(f64::EPSILON);
        return Ok(fallback
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let normalized = (h.score - min_s) / range;
                store_hit_to_result(h, (i + 1) as u32, normalized, include_content)
            })
            .collect());
    }

    tracing::debug!(
        "live fused ({} hits): {}",
        outcome.fused.len(),
        outcome
            .fused
            .iter()
            .take(5)
            .map(|f| format!("{}={:.3}", f.url, f.score))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let live_slice = &outcome.fused[..outcome.fused.len().min(fetch)];
    let fused = fuse_index_and_live(store_hits, live_slice);

    // Build results from fused list (scores already normalized to [0,1]).
    let mut results = Vec::with_capacity(fused.len());
    let mut seen_urls: std::collections::HashSet<String> =
        std::collections::HashSet::with_capacity(fused.len());
    for (f, store_hit) in &fused {
        if results.len() >= limit.max(1) as usize {
            break;
        }
        let rank = (results.len() + 1) as u32;
        seen_urls.insert(canonical_url(&f.url));
        if let Some(hit) = store_hit {
            results.push(store_hit_to_result(hit, rank, f.score, include_content));
        } else {
            results.push(live_fused_to_result(f, rank, include_content));
        }
    }

    // --- Sparse fill: pad with store-only hits not already in fused list ---
    if results.len() < limit.max(1) as usize {
        let store_unseen: Vec<&TursoSearchHit> = store_hits
            .iter()
            .filter(|h| !seen_urls.contains(&canonical_url(&h.url)))
            .collect();
        let max_s = store_unseen.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = store_unseen.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = (max_s - min_s).max(f64::EPSILON);

        for h in store_unseen {
            if results.len() >= limit.max(1) as usize {
                break;
            }
            let rank = (results.len() + 1) as u32;
            let norm_score = if range < f64::EPSILON {
                1.0
            } else {
                ((h.score - min_s) / range).clamp(0.0, 1.0)
            };
            results.push(store_hit_to_result(h, rank, norm_score, include_content));
        }
    }

    if !signals.iter().any(|s| s == "live") {
        signals.push("live".to_string());
    }
    Ok(results)
}

/// Drop the last `n` whitespace-separated tokens from a query string.
fn relax_query(query: &str, drop: usize) -> String {
    let tokens: Vec<&str> = query.split_whitespace().collect();
    tokens
        .iter()
        .take(tokens.len().saturating_sub(drop))
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Emit warn/debug lines for any engine failures in an outcome.
fn log_engine_failures(outcome: &webfind::engine::web_index::Outcome) {
    if outcome.engines_failed > 0 {
        tracing::warn!(
            "live search: {}/{} engines failed",
            outcome.engines_failed,
            outcome.total_engines
        );
        for report in &outcome.reports {
            if let Some(error) = &report.error {
                tracing::debug!("engine {} failed: {error:?}", report.engine);
            }
        }
    }
}


/// Search the embedded Turso graph store with hybrid (BM25 + vector + graph)
/// RRF fusion, optionally merged with live search-engine results. Used when
/// `--graph-store turso` is selected; requires no `index_data` directory.
#[allow(clippy::too_many_arguments)]
async fn turso_search(
    cfg: &webfind::config::WebfindConfig,
    query: &str,
    depth: webfind::cli::DepthArg,
    limit: u32,
    output: webfind::cli::OutputArg,
    include_content: bool,
    turso_path: Option<String>,
    hybrid: bool,
    live: bool,
) -> anyhow::Result<()> {
    let start = Instant::now();
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
    let store = TursoStore::new(&turso_path)
        .await
        .with_context(|| format!("open Turso graph store at `{turso_path}`"))?;
    // Ensure the derived indexes are current so search returns full signals.
    // Rebuild only when the graph has changed since the last indexation.
    let current_graph_ver = store.graph_version().await;
    if store.fts_version().await != current_graph_ver {
        store.rebuild_fts().await.context("rebuild FTS index")?;
    }
    // PageRank is expensive on large graphs; recompute only when the graph
    // changed since the last computation (the `pagerank` table records the
    // graph version it was computed from).
    if store.pagerank_version().await != current_graph_ver {
        store
            .compute_pagerank(20, 0.85)
            .await
            .context("compute PageRank")?;
    }

    // Embed the query for the vector signal (fall back to the deterministic
    // dummy embedder when the ONNX model is unavailable).
    let embedding: Option<Vec<f32>> = if hybrid {
        match FastembedEmbedder::new() {
            Ok(e) => e.embed(&[query]).ok().and_then(|v| v.into_iter().next()),
            Err(e) => {
                tracing::warn!("fastembed unavailable, using dummy embedder: {e}");
                DummyEmbedder
                    .embed(&[query])
                    .ok()
                    .and_then(|v| v.into_iter().next())
            }
        }
    } else {
        None
    };

    // Fetch extra store hits when merging with live results so the fusion has
    // material beyond the final limit.
    let store_limit = if live {
        (limit.max(1) * 2) as usize
    } else {
        limit.max(1) as usize
    };
    let hits = store
        .search(query, embedding.as_deref(), store_limit)
        .await
        .context("hybrid search in Turso store")?;

    let mut signals: Vec<String> = Vec::new();
    for h in &hits {
        for s in &h.signals {
            if !signals.iter().any(|known| known == s) {
                signals.push(s.clone());
            }
        }
    }
    if signals.is_empty() {
        signals.push("bm25".to_string());
    }

    let results = if live {
        merge_live_results(query, &hits, limit, include_content, &mut signals).await?
    } else {
        let max_s = hits.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = hits.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = (max_s - min_s).max(f64::EPSILON);
        hits.iter()
            .enumerate()
            .map(|(i, h)| {
                let norm_score = if range < f64::EPSILON {
                    1.0
                } else {
                    ((h.score - min_s) / range).clamp(0.0, 1.0)
                };
                store_hit_to_result(h, (i + 1) as u32, norm_score, include_content)
            })
            .collect()
    };

    let response = SearchResponse {
        request_id: uuid::Uuid::new_v4().to_string(),
        query: query.to_string(),
        depth: SearchDepth::from(depth),
        total_results: results.len() as u64,
        returned: results.len() as u32,
        latency_ms: start.elapsed().as_millis() as u64,
        results,
        suggestions: vec![],
        related: vec![],
        graph: None,
        metadata: SearchMetadata {
            index_version: "turso".to_string(),
            index_size: store.get_urls().await.len() as u64,
            engine_version: "webfind-turso".to_string(),
            searched_at: Utc::now(),
            signals_used: signals,
            index_freshness: IndexFreshness {
                oldest_page: None,
                newest_page: None,
                avg_age_days: 0.0,
            },
        },
    };

    let formatted = format_response(&response, &OutputFormat::from(output));
    print!("{}", formatted);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    query: String,
    depth: webfind::cli::DepthArg,
    limit: u32,
    output: webfind::cli::OutputArg,
    _language: Option<String>,
    _domains: Option<String>,
    include_content: bool,
    _include_graph: bool,
    _include_keywords: bool,
    _include_metrics: bool,
    graph_store: Option<GraphStoreArg>,
    turso_path: Option<String>,
    hybrid: bool,
    live: bool,
) -> anyhow::Result<()> {
    let graph_store = webfind::config::resolve_graph_store(cfg, graph_store);
    if graph_store == GraphStoreArg::Memory {
        eprintln!(
            "note: '--graph-store memory' has no persistent index; searching the Turso store instead."
        );
    }
    turso_search(
        cfg,
        &query,
        depth,
        limit,
        output,
        include_content,
        turso_path,
        hybrid,
        live,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_hit(url: &str, title: &str) -> TursoSearchHit {
        TursoSearchHit {
            url: url.to_string(),
            title: title.to_string(),
            excerpt: format!("excerpt for {title}"),
            score: 1.0,
            signals: vec!["bm25".to_string()],
            bm25_score: Some(0.016),
            vector_score: None,
            graph_score: None,
        }
    }

    fn live_fused(url: &str, title: &str, score: f64) -> FusedHit {
        FusedHit {
            url: url.to_string(),
            title: title.to_string(),
            snippet: format!("snippet for {title}"),
            published_at: None,
            score,
            engine_count: 1,
            engines: vec!["ddg"],
        }
    }

    #[test]
    fn fusion_ranks_shared_urls_above_single_list_urls() {
        let store = vec![
            store_hit("https://example.com/old", "Old page"),
            store_hit("https://example.com/other", "Other page"),
        ];
        let live = vec![
            live_fused("https://fresh.example/new", "Fresh page", 0.9),
            live_fused("https://example.com/old", "Old page", 0.8),
        ];
        let fused = fuse_index_and_live(&store, &live);

        // The URL present in both lists outranks URLs present in only one.
        assert_eq!(fused[0].0.url, "https://example.com/old");
        assert!(fused[0].1.is_some(), "shared URL enriched with store hit");
        // The live-only URL (live rank 1) outranks the store-only URL (store rank 2).
        assert_eq!(fused[1].0.url, "https://fresh.example/new");
        assert!(fused[1].1.is_none());
        assert_eq!(fused[2].0.url, "https://example.com/other");
    }

    #[test]
    fn fusion_keeps_store_content_for_shared_urls() {
        let store = vec![store_hit("https://example.com/a", "Stored A")];
        let live = vec![live_fused("https://example.com/a", "Live A", 0.9)];
        let fused = fuse_index_and_live(&store, &live);

        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].0.url, "https://example.com/a");
        assert_eq!(fused[0].1.map(|h| h.title.as_str()), Some("Stored A"));
    }

    #[test]
    fn fusion_dedups_tracking_variants_of_same_url() {
        let store = vec![store_hit("https://example.com/doc?utm_source=x", "Stored")];
        let live = vec![live_fused("https://example.com/doc", "Live", 0.9)];
        let fused = fuse_index_and_live(&store, &live);

        assert_eq!(fused.len(), 1, "tracking variants fuse into one result");
        assert_eq!(fused[0].0.url, "https://example.com/doc?utm_source=x");
        assert!(fused[0].1.is_some());
    }

    #[test]
    fn relax_query_drops_trailing_tokens() {
        assert_eq!(relax_query("a b c d", 1), "a b c");
        assert_eq!(relax_query("a b c d", 3), "a");
        assert_eq!(relax_query("hello", 1), "");
        assert_eq!(relax_query("", 1), "");
    }

    #[test]
    fn relax_query_broadest_3_word() {
        let q = "how the best creators design thumbnails 2025 2026 Paddy Galloway 1of10";
        let words: Vec<&str> = q.split_whitespace().collect();
        let drop = words.len().saturating_sub(3);
        let broad = relax_query(q, drop);
        assert_eq!(broad, "how the best");
    }

    #[test]
    fn fused_scores_normalized_after_fusion() {
        let store = vec![
            store_hit("https://a.example", "A"),
            store_hit("https://b.example", "B"),
        ];
        let live = vec![
            live_fused("https://a.example", "A", 0.9),
            live_fused("https://c.example", "C", 0.5),
        ];
        let fused = fuse_index_and_live(&store, &live);
        // After normalization the top hit must score 1.0.
        assert!(
            (fused[0].0.score - 1.0).abs() < 1e-9,
            "top fused score should be 1.0 after normalization, got {}",
            fused[0].0.score
        );
        // All scores must be in [0, 1].
        for (f, _) in &fused {
            assert!(f.score >= 0.0 && f.score <= 1.0, "score out of range: {}", f.score);
        }
    }

    #[test]
    fn live_result_no_date_emits_no_freshness_or_quality() {
        let hit = live_fused("https://example.com/page", "A Page", 0.80);
        let result = live_fused_to_result(&hit, 1, false);
        assert!(result.scores.freshness.is_none(), "no published_at → freshness must be None");
        assert!(result.scores.quality.is_none(), "no fetched body → quality must be None");
        // engine_count=1 → fusion_bonus = (1-1)/3 * 0.10 = 0.0; final = 0.80 + 0.0
        let expected = 0.80_f64.clamp(0.0, 1.0);
        assert!(
            (result.score - expected).abs() < 1e-9,
            "score={} expected={expected}",
            result.score
        );
    }

    #[test]
    fn live_result_with_date_applies_freshness_weight() {
        let mut hit = live_fused("https://example.com/page", "A Page", 0.80);
        hit.published_at = Some(chrono::Utc::now());
        let result = live_fused_to_result(&hit, 1, false);
        let f = result.scores.freshness.expect("known date → freshness must be Some");
        assert!(f > 0.95, "just-published freshness should be > 0.95, got {f}");
        assert!(result.scores.quality.is_none(), "still no body → quality must be None");
        // final = 0.80*0.75 + f*0.25 + 0.0 (engine_count=1)
        let expected = (0.80 * 0.75 + f * 0.25).clamp(0.0, 1.0);
        assert!(
            (result.score - expected).abs() < 1e-9,
            "score={} expected={expected}",
            result.score
        );
    }

    #[test]
    fn live_result_multi_engine_adds_fusion_bonus() {
        let mut hit = live_fused("https://example.com/page", "A Page", 0.70);
        hit.engine_count = 4;
        let result = live_fused_to_result(&hit, 1, false);
        // fusion_bonus = (4-1)/3 * 0.10 = 0.10; final = (0.70 + 0.10).clamp(0,1)
        let expected = (0.70 + 0.10_f64).clamp(0.0, 1.0);
        assert!(
            (result.score - expected).abs() < 1e-9,
            "score={} expected={expected}",
            result.score
        );
    }
}
