use std::collections::HashMap;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;

use webfind::cli::GraphStoreArg;
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::engine::embedder::{DummyEmbedder, Embedder, FastembedEmbedder};
use webfind::engine::web_index::{
    EngineOptions, FusedHit, Hit, LiveIndex, canonical_url, positional_relevance,
    reciprocal_rank_fusion,
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
    let has_vec = hit.signals.iter().any(|s| s == "vector");
    let has_graph = hit.signals.iter().any(|s| s == "graph");
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
            bm25: 0.0,
            vector: has_vec.then_some(score),
            graph: has_graph.then_some(score),
            freshness: None,
            quality: None,
            final_score: score,
        },
        content: include_content.then(|| ContentBlock {
            text: hit.excerpt.clone(),
            excerpt: hit.excerpt.clone(),
            word_count: 0,
            reading_time_seconds: 0,
            html: None,
            markdown: None,
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
/// Live results carry title/snippet from the engines but no stored content, so
/// `content` is the snippet when requested.
fn live_fused_to_result(fused: &FusedHit, rank: u32, include_content: bool) -> SearchResult {
    let domain = url::Url::parse(&fused.url)
        .map(|u| u.host_str().unwrap_or("").to_string())
        .unwrap_or_default();
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
        score: fused.score,
        scores: ScoreBreakdown {
            bm25: 0.0,
            vector: None,
            graph: None,
            freshness: None,
            quality: None,
            final_score: fused.score,
        },
        content: include_content.then(|| ContentBlock {
            text: fused.snippet.clone(),
            excerpt: fused.snippet.clone(),
            word_count: 0,
            reading_time_seconds: 0,
            html: None,
            markdown: None,
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

    let fused = reciprocal_rank_fusion(&[(&index_list, 1.0), (&live_list, LIVE_FUSION_WEIGHT)], 60);

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
/// URLs already in the store keep their stored content and signal breakdown;
/// live-only URLs carry title/snippet from the engines. Appends `"live"` to
/// `signals` when live results contributed.
async fn merge_live_results(
    query: &str,
    store_hits: &[TursoSearchHit],
    limit: u32,
    include_content: bool,
    signals: &mut Vec<String>,
) -> anyhow::Result<Vec<SearchResult>> {
    let live_index = LiveIndex::new().context("build live search index")?;
    let opts = EngineOptions {
        max_results: limit.max(1) as usize,
        ..EngineOptions::default()
    };
    let outcome = live_index.search(query, &opts).await;
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
    if outcome.fused.is_empty() {
        return Ok(store_hits
            .iter()
            .enumerate()
            .map(|(i, h)| store_hit_to_result(h, (i + 1) as u32, h.score, include_content))
            .collect());
    }
    tracing::debug!(
        "live fused: {}",
        outcome
            .fused
            .iter()
            .map(|f| format!("{}={:.3}", f.url, f.score))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let fused = fuse_index_and_live(store_hits, &outcome.fused[..limit.max(1) as usize]);
    let mut results = Vec::with_capacity(fused.len());
    for (i, (f, store_hit)) in fused.iter().enumerate() {
        let rank = (i + 1) as u32;
        if let Some(hit) = store_hit {
            results.push(store_hit_to_result(hit, rank, f.score, include_content));
        } else {
            results.push(live_fused_to_result(f, rank, include_content));
        }
    }
    results.truncate(limit.max(1) as usize);

    if !signals.iter().any(|s| s == "live") {
        signals.push("live".to_string());
    }
    Ok(results)
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
    store.rebuild_fts().await.context("rebuild FTS index")?;
    // PageRank is expensive on large graphs; recompute only when the graph
    // changed since the last computation (the `pagerank` table records the
    // graph version it was computed from).
    if store.pagerank_version().await != store.graph_version().await {
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
        hits.iter()
            .enumerate()
            .map(|(i, h)| store_hit_to_result(h, (i + 1) as u32, h.score, include_content))
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
}
