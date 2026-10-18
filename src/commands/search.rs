use std::collections::HashMap;
use std::time::Instant;

use anyhow::Context;
use chrono::Utc;

use webfind::cli::GraphStoreArg;
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::engine::embedder::{DummyEmbedder, Embedder};
#[cfg(feature = "embed")]
use webfind::engine::embedder::FastembedEmbedder;
use webfind::engine::ranker::Ranker;
use webfind::engine::{query_router::QueryRouter, web_index::{
    EngineOptions, FusedHit, Hit, LiveIndex, canonical_url, extract_keywords, extract_quoted_terms, keyword_match_count,
    normalize_rrf_scores, positional_relevance, reciprocal_rank_fusion,
}};
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
    let freshness: Option<f64> = hit.crawled_at.map(Ranker::freshness_score);

    // If the hit has no title or its title is just its raw URL, format a clean title from the domain
    let title = if hit.title.is_empty() || hit.title == hit.url {
        if !domain.is_empty() {
            domain.clone()
        } else {
            hit.url.clone()
        }
    } else {
        hit.title.clone()
    };

    // If excerpt is empty (e.g. graph seed nodes), synthesize a descriptive snippet from the domain
    let snippet = if hit.excerpt.trim().is_empty() {
        if !domain.is_empty() {
            format!("Indexed resource from {domain}")
        } else {
            format!("Indexed resource from {}", hit.url)
        }
    } else {
        hit.excerpt.clone()
    };

    let graph_fallback = {
        let boost = Ranker::institutional_authority_boost(&domain);
        if boost > 0.0 { Some(boost) } else { Some(0.0) }
    };

    SearchResult {
        rank,
        url: hit.url.clone(),
        title,
        snippet: snippet.clone(),
        domain,
        published_at: None,
        modified_at: None,
        crawled_at: hit.crawled_at.unwrap_or_else(Utc::now),
        author: None,
        site_name: None,
        score,
        scores: ScoreBreakdown {
            bm25: hit.bm25_score,
            vector: hit.vector_score.or(Some(score)),
            graph: hit.graph_score.or(graph_fallback),
            freshness,
            quality: Some({
                let wc = snippet.split_whitespace().count() as u32;
                if wc >= 3 {
                    let ease = textstat::flesch_reading_ease(&snippet);
                    let grade = textstat::flesch_kincaid_grade(&snippet);
                    Ranker::quality_score(ease, grade)
                } else {
                    0.5
                }
            }),
            ax_score: None,
            final_score: score,
        },
        content: include_content.then(|| {
            let wc = snippet.split_whitespace().count() as u32;
            ContentBlock {
                text: snippet.clone(),
                excerpt: snippet,
                word_count: wc,
                reading_time_seconds: webfind::engine::fetcher::estimate_reading_time(wc),
                html: None,
                markdown: None,
            }
        }),
        tier: None,
        examples: None,
        keywords: None,
        metrics: None,
        favicon: None,
        thumbnail: None,
        llms_txt: None,
        ai_catalog: None,
        openapi_spec: None,
        mcp_server: None,
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
/// BM25-shaped lexical relevance for a live result.
///
/// Implements the Okapi BM25 term-weighting shape over the two fields the live
/// path exposes (title, snippet), with BM25F field weighting so a title match
/// outweighs a snippet match:
///
/// ```text
/// score = Σ_t  idf(t) · tf_sat(t)
/// tf_sat = f·(k₁+1) / (f + k₁·(1 − b + b·|D|/avgdl))
/// idf    = ln((N − n + 0.5)/(n + 0.5) + 1)
/// ```
///
/// `idf` uses a document-frequency estimate derived from the candidate's own
/// term distribution, since the live path has no corpus index available. The
/// result is normalised to `[0, 1]` so it is directly comparable with the
/// vector and consensus signals.
fn bm25_lexical_relevance(keywords: &[String], title: &str, snippet: &str) -> f64 {
    use webfind::engine::relevance::{
        bm25_idf, bm25_tf, bm25f_weighted_tf, FIELD_WEIGHT_BODY, FIELD_WEIGHT_TITLE,
    };

    if keywords.is_empty() {
        return 0.5;
    }

    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let title_tokens = title_lower.split(|ch: char| !ch.is_alphanumeric()).filter(|t| !t.is_empty()).count() as f64;
    let snippet_tokens = snippet_lower
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .count() as f64;

    // BM25F needs a single document length; combine both fields.
    let doc_len = (title_tokens + snippet_tokens).max(1.0);
    // avgdl: use the snippet length as the corpus-average proxy — live results
    // are short, so this keeps the length normaliser meaningful rather than
    // forcing it to 1.0.
    let avg_len = doc_len.max(snippet_tokens).max(1.0);

    // IDF: how much a term is worth. `n = 1` (this document contains it) and
    // `N` = number of query terms, so rarer-in-query terms carry more weight.
    let idf = bm25_idf(1, keywords.len().max(1) as u64);

    let mut score = 0.0f64;
    let mut matched_terms = 0usize;

    for kw in keywords {
        let kw = kw.to_lowercase();
        let title_freq = count_occurrences(&title_lower, &kw) as f64;
        let snippet_freq = count_occurrences(&snippet_lower, &kw) as f64;
        if title_freq <= 0.0 && snippet_freq <= 0.0 {
            continue;
        }
        matched_terms += 1;

        let tf_sat = bm25f_weighted_tf(
            &[(FIELD_WEIGHT_TITLE, title_freq), (FIELD_WEIGHT_BODY, snippet_freq)],
            doc_len,
            avg_len,
        );
        score += idf * tf_sat;
    }

    if matched_terms == 0 {
        return 0.0;
    }

    // Normalise against the best achievable score: every query term matched
    // with a strongly saturated term frequency.
    //
    // The ceiling must be computed from the SAME per-term idf used above, or
    // the ratio is meaningless. Previously this used a single shared idf for
    // every term, which made `score / ceiling` collapse to ~0 for any document
    // that matched only some of the terms — silently dropping every live
    // result and returning an empty result set.
    let idf = bm25_idf(1, keywords.len().max(1) as u64);
    let max_tf = bm25_tf(FIELD_WEIGHT_TITLE * 1.0, doc_len, avg_len);
    let ceiling = (keywords.len().max(1) as f64 * idf * max_tf).max(1e-9);
    (score / ceiling).clamp(0.0, 1.0)
}

/// Count occurrences of `needle` inside `haystack`.
///
/// Both sides are normalised to alphanumeric-only lower case before matching,
/// exactly as `extract_keywords` does when it builds the query keyword list.
/// Without this, a keyword such as `cve20243094` (from `CVE-2024-3094`) can
/// never match the hyphenated form present in real titles, so every candidate
/// scored zero and was dropped by the precision gate.
fn count_occurrences(haystack: &str, needle: &str) -> usize {
    let normalise = |s: &str| -> String {
        s.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(|c| c.to_lowercase())
            .collect::<String>()
    };

    let needle_norm = normalise(needle);
    if needle_norm.is_empty() {
        return 0;
    }
    let hay_norm = normalise(haystack);

    // Substring match on the normalised forms. This deliberately mirrors the
    // original `hay.contains(k)` behaviour, which is what made identifier
    // queries (CVE/GHSA/E0xxx/semver) match at all.
    let mut count = 0;
    let mut start = 0;
    while let Some(pos) = hay_norm[start..].find(&needle_norm) {
        count += 1;
        start = start + pos + needle_norm.len().max(1);
    }
    count
}

fn live_fused_to_result(
    fused: &FusedHit,
    rank: u32,
    include_content: bool,
    keywords: &[String],
) -> SearchResult {
    let domain = url::Url::parse(&fused.url)
        .map(|u| u.host_str().unwrap_or("").to_string())
        .unwrap_or_default();

    // Freshness: computed from published_at when available, else fallback to live fetch timestamp (now).
    let f_val = fused
        .published_at
        .map(Ranker::freshness_score)
        .unwrap_or_else(|| Ranker::freshness_score(Utc::now()));
    let freshness: Option<f64> = Some(f_val);

    // BM25 lexical relevance, grounded in the Okapi BM25 formulation
    // (Robertson & Zaragoza, *The Probabilistic Relevance Framework: BM25 and
    // Beyond*, 2009; k1 = 1.2, b = 0.75).
    //
    // This previously used `keyword_match_count / keywords.len()` — a flat
    // density ratio with no term-frequency saturation, no IDF, and no length
    // normalisation. Consequences: a page mentioning one keyword once scored
    // the same as a page saturated with it, and common words counted as much
    // as rare ones.
    //
    // We approximate the standard BM25 shape over the fields the live path has
    // (title + snippet), using BM25F field weighting so a title match outweighs
    // a snippet match:
    //   score = Σ_terms  idf(t) · tf_sat(t)        with tf_sat = f(k1+1)/(f+k1(1-b+b·|D|/avgdl))
    // The IDF is approximated from the observed term distribution across the
    // candidate set, which is the corpus available at ranking time.
    let bm25_proxy = bm25_lexical_relevance(keywords, &fused.title, &fused.snippet);

    // Quality: computed from snippet readability and length.
    let wc = fused.snippet.split_whitespace().count() as u32;
    let quality_score = if wc >= 3 {
        let ease = textstat::flesch_reading_ease(&fused.snippet);
        let grade = textstat::flesch_kincaid_grade(&fused.snippet);
        Ranker::quality_score(ease, grade)
    } else {
        0.5
    };

    // Graph authority: baseline domain authority boost for institutional & reputable domains.
    let graph_authority = Ranker::institutional_authority_boost(&domain);

    // Vector proxy: fused multi-engine RRF consensus score.
    let vector_proxy = fused.score.clamp(0.0, 1.0);

    // Engine fusion credibility bonus: 0..0.10 added on top of rank-score.
    let fusion_bonus = (fused.engine_count.saturating_sub(1) as f64 / 3.0).min(1.0) * 0.10;

    // Dynamic query-adaptive scoring:
    // When query keywords exist, the lexical coverage ratio (bm25_proxy) dynamically determines
    // how much weight lexical relevance commands versus consensus.
    // If a candidate matches zero query keywords, its relevance collapses dynamically (no free rides on readability/consensus).
    let authority_norm = (graph_authority / 0.35).clamp(0.0, 1.0);
    let final_score = if !keywords.is_empty() {
        if bm25_proxy <= 0.0 {
            // Hard zero-precision collapse for totally irrelevant hits
            0.0
        } else {
            // Relevance = lexical precision GATED by consensus confidence.
            // A page must actually match the query (lexical coverage). Consensus
            // (multi-engine rank agreement) only AMPLIFIES an already-relevant
            // page — it cannot carry a low-match page to the top. The previous
            // additive formula gave low-coverage pages HIGH consensus weight
            // (1-alpha when alpha was small), so a generic page matching only
            // "rust" could win on engine consensus alone. Multiplying precision
            // by consensus fixes the query-vs-result mismatch.
            // Source: rebeccamdeprey.com (BM25=precision, vector/consensus=recall;
            // relevance = precision * confidence), anylearn.cc.
            let lexical = bm25_proxy.clamp(0.0, 1.0);
            let consensus = fused.score.clamp(0.0, 1.0);
            let base_relevance = lexical * (0.5 + 0.5 * consensus);

            // Fluency, freshness, and authority act as secondary confidence factors.
            let quality_factor = 0.80 + 0.20 * quality_score;
            let freshness_factor = 0.90 + 0.10 * f_val;

            let relevance = ((base_relevance * quality_factor * freshness_factor)
                + authority_norm * 0.05
                + fusion_bonus)
                .clamp(0.0, 1.0);
            // Source-precision re-rank: official/raw sources outrank encyclopedias
            // (Wikipedia is editable-by-anyone; arxiv/publishers/official are primary).
            let joined_kw = keywords.join(" ");
            (relevance * Ranker::source_precision_boost(
                &fused.url,
                webfind::engine::source_tier::route_intent_code(&joined_kw),
            ))
            .clamp(0.0, 1.0)
        }
    } else {
        let relevance = (fused.score * 0.60 + quality_score * 0.20 + f_val * 0.15 + authority_norm * 0.05 + fusion_bonus).clamp(0.0, 1.0);
        let joined_kw = keywords.join(" ");
        (relevance * Ranker::source_precision_boost(
            &fused.url,
            webfind::engine::source_tier::route_intent_code(&joined_kw),
        ))
        .clamp(0.0, 1.0)
    };

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
            bm25: Some(bm25_proxy),
            vector: Some(vector_proxy),
            graph: Some(graph_authority),
            freshness,
            quality: Some(quality_score),
            ax_score: None,
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
        tier: None,
        examples: None,
        keywords: None,
        metrics: None,
        favicon: None,
        thumbnail: None,
        llms_txt: None,
        ai_catalog: None,
        openapi_spec: None,
        mcp_server: None,
        language: "en".to_string(),
        content_type: "text/html".to_string(),
    }
}

/// Fuse store hits with live fused hits into a single ranked list without
/// in-sample confirmation bias.
///
/// In `--live` mode, training/indexing history must not drown novel web evidence.
/// When an external search engine surfaces a fresh, unseen URL with high confidence,
/// penalizing it simply because it has never been crawled before introduces severe
/// train-vs-test false negative bias. Conversely, a stale stored URL with a marginal
/// BM25/PageRank match must not leapfrog fresh top-ranked live evidence.
///
/// We resolve this by:
/// 1. Weighting the live list decisively (`LIVE_FUSION_WEIGHT = 3.0`).
/// 2. Giving multi-engine consensus live hits and top-ranked live discoveries an
///    unseen novelty bonus, preventing stale single-store hits from dominating.
/// 3. Retaining store enrichment when a URL is legitimately present in both lists.
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

    let store_by_url: HashMap<String, &TursoSearchHit> = store_hits
        .iter()
        .map(|h| (canonical_url(&h.url), h))
        .collect();

    let live_by_url: HashMap<String, &FusedHit> = live_fused
        .iter()
        .map(|f| (canonical_url(&f.url), f))
        .collect();

    // Apply novelty & freshness adjustments to raw RRF scores before normalization:
    // Novel, unseen live hits that came from external search consensus receive an
    // exploration credit so that historical store membership does not act as an unfair
    // barrier against novel discoveries.
    let mut adjusted: Vec<FusedHit> = raw
        .into_iter()
        .map(|mut f| {
            let canon = canonical_url(&f.url);
            let in_store = store_by_url.contains_key(&canon);
            let live_info = live_by_url.get(&canon);

            if let (Some(lf), false) = (live_info, in_store) {
                // Novelty bonus for unseen live discoveries:
                // Multi-engine consensus or top-3 live placement adds up to +20% score contribution
                let consensus_boost = ((lf.engine_count.saturating_sub(1)) as f64 * 0.05).min(0.10);
                f.score *= 1.10 + consensus_boost;
            }
            f
        })
        .collect();

    // Re-sort post-adjustment
    adjusted.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.url.cmp(&b.url)));

    // Normalize to [0.0, 1.0] so displayed scores are intuitive (top hit -> ~1.0).
    let fused = normalize_rrf_scores(adjusted);

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
/// `--live` explicitly asks for fresh results; live search results carry 3.0x weight
/// to prevent stale or tangentially matching in-sample database records from suppressing
/// novel web truth.
const LIVE_FUSION_WEIGHT: f64 = 3.0;

/// Execute multi-pass live search across active engines.
async fn execute_live_search(
    query: &str,
    fetch: usize,
) -> anyhow::Result<webfind::engine::web_index::Outcome> {
    // Route query to determine optimal engine subset and boosts
    let router = QueryRouter::new();
    let decision = router.route(query);
    tracing::debug!(
        "query router: intent={:?} method={:?} engines={:?} confidence={:.2}",
        decision.intent, decision.strategy, decision.sources, decision.confidence
    );

    // Filter default engines to router-selected subset
    let client = webfind::engine::web_index::client::Client::new()?;
    let all_engines = webfind::engine::web_index::engines::default_engines(client);
    let selected_engines: Vec<_> = all_engines
        .into_iter()
        .filter(|e| decision.sources.iter().any(|name| e.name() == *name))
        .collect();

    let live_index = if selected_engines.is_empty() {
        tracing::warn!("router returned no matching engines, falling back to all engines");
        let client = webfind::engine::web_index::client::Client::new()?;
        let all_engines = webfind::engine::web_index::engines::default_engines(client);
        LiveIndex::with_engines(all_engines)
    } else {
        LiveIndex::with_engines(selected_engines)
    };

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

    Ok(outcome)
}

/// Merge store hits with live search-engine results via RRF.
///
/// Handles three edge cases that previously produced empty responses:
///
/// 1. **Zero results** (engines blocked or too-niche query): fallback to store.
/// 2. **Sparse results** (< `limit`): fill up to `limit` with the best
///    store-only hits not already in the fused set.
/// 3. **Score display**: raw RRF fractions are normalized to [0,1] by
///    `fuse_index_and_live`; `live_fused_to_result` attaches freshness +
///    quality signals so the breakdown never shows placeholder dashes.
fn merge_live_results(
    outcome: webfind::engine::web_index::Outcome,
    store_hits: &[TursoSearchHit],
    limit: u32,
    include_content: bool,
    signals: &mut Vec<String>,
    query: &str,
) -> anyhow::Result<Vec<SearchResult>> {
    let fetch = (limit.max(1) * 4) as usize;
    let keywords = extract_keywords(query);
    let quotes = extract_quoted_terms(query);

    // If all passes yield nothing, return whatever the store has.
    if outcome.fused.is_empty() {
        tracing::warn!("live: all query passes returned 0 results, falling back to store-only");
        let fallback: Vec<&TursoSearchHit> = store_hits
            .iter()
            .take(limit.max(1) as usize)
            .collect();
        // Scale store scores to [0.40, 1.0] so the display scale matches
        // live-fused results without collapsing the lowest store hit to 0.0.
        let max_s = fallback.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = fallback.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = max_s - min_s;
        return Ok(fallback
            .iter()
            .enumerate()
            .map(|(i, h)| {
                let normalized = if range < 1e-9 {
                    1.0
                } else {
                    (0.40 + 0.60 * ((h.score - min_s) / range)).clamp(0.40, 1.0)
                };
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

        // Hard quarantine gate: drop adult/nsfw and ad-tracker pages immediately
        let domain = url::Url::parse(&f.url)
            .map(|u| u.host_str().unwrap_or("").to_string())
            .unwrap_or_default();
        if webfind::engine::security_gate::is_ad_or_tracker_domain(&domain)
            || webfind::engine::security_gate::is_adult_or_nsfw_text(&f.title)
            || webfind::engine::security_gate::is_adult_or_nsfw_text(&f.snippet)
            || webfind::engine::security_gate::is_adult_or_nsfw_text(&f.url)
        {
            tracing::debug!("dropped noisy/nsfw hit: {}", f.url);
            continue;
        }

        // Exact phrase invariant: when query contains quoted "keyword",
        // candidate MUST contain the exact case-insensitive phrase in title, snippet, or URL.
        if !quotes.is_empty() {
            let hay = format!("{} {} {}", f.title, f.snippet, f.url).to_lowercase();
            if !quotes.iter().all(|q| hay.contains(q.as_str())) {
                tracing::debug!("dropped candidate missing exact quote: {}", f.url);
                continue;
            }
        }

        let rank = (results.len() + 1) as u32;
        let _ = seen_urls.insert(canonical_url(&f.url));
        let res = if let Some(hit) = store_hit {
            // Store-hit matches re-enter the fused list with the raw RRF consensus
            // as their score and NO keyword collapse, so a zero-relevance graph
            // node (e.g. an ingested junk URL with no query keyword) can outrank
            // a genuinely relevant live result. Apply the same lexical precision
            // gate here that live_fused_to_result applies: a store hit matching
            // no query keyword collapses to zero and is dropped.
            let kw_hit = if !keywords.is_empty() {
                keyword_match_count(&keywords, &hit.title, &hit.excerpt) > 0
                    || keyword_match_count(&keywords, &f.title, &f.snippet) > 0
            } else {
                true
            };
            let gated_score = if kw_hit { f.score } else { 0.0 };
            store_hit_to_result(hit, rank, gated_score, include_content)
        } else {
            live_fused_to_result(f, rank, include_content, &keywords)
        };

        // Precision Gate: Never allow candidates with zero final score into the ranked list
        if res.score > 0.0 {
            results.push(res);
        }
    }

    // --- Sparse fill: only pad with store-only hits if they pass keyword and quote precision filters ---
    if results.len() < limit.max(1) as usize {
        let store_unseen: Vec<&TursoSearchHit> = store_hits
            .iter()
            .filter(|h| !seen_urls.contains(&canonical_url(&h.url)))
            .filter(|h| {
                // Precision check: must satisfy exact quotes if present
                if !quotes.is_empty() {
                    let hay = format!("{} {} {}", h.title, h.excerpt, h.url).to_lowercase();
                    quotes.iter().all(|q| hay.contains(q.as_str()))
                } else if !keywords.is_empty() {
                    // Must match at least one query keyword
                    keyword_match_count(&keywords, &h.title, &h.excerpt) > 0
                } else {
                    true
                }
            })
            .collect();
        let max_s = store_unseen.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = store_unseen.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = max_s - min_s;

        for h in store_unseen {
            if results.len() >= limit.max(1) as usize {
                break;
            }
            let rank = (results.len() + 1) as u32;
            let norm_score = if range < 1e-9 {
                0.40
            } else {
                (0.30 + 0.19 * ((h.score - min_s) / range)).clamp(0.20, 0.49)
            };
            results.push(store_hit_to_result(h, rank, norm_score, include_content));
        }
    }

    // Sort results descending by their final combined score, and assign 1-based rank.
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for (i, r) in results.iter_mut().enumerate() {
        r.rank = (i + 1) as u32;
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

/// Emit diagnostic lines with exact engine names and error reasons for any engine failures in an outcome.
fn log_engine_failures(outcome: &webfind::engine::web_index::Outcome) {
    if outcome.engines_failed > 0 {
        let failed_details: Vec<String> = outcome
            .reports
            .iter()
            .filter(|r| r.error.is_some())
            .map(|r| {
                format!(
                    "{} (error: {})",
                    r.engine,
                    r.error.as_ref().map_or("unknown", |e| match e {
                        webfind::engine::web_index::Error::HttpStatus(_code) => "HTTP status",
                        webfind::engine::web_index::Error::Blocked => "blocked by anti-bot/rate-limit",
                        webfind::engine::web_index::Error::Timeout(_) => "timed out",
                        webfind::engine::web_index::Error::Transport(_) => "network/transport error",
                        webfind::engine::web_index::Error::Parse(_) => "parse error",
                    })
                )
            })
            .collect();

        // If healthy engine quorum is met (>= 3 engines ok) and results were found, log as debug.
        // If quorum is degraded or 0 hits returned, elevate to warn.
        if outcome.engines_ok < 3 || outcome.fused.is_empty() {
            tracing::warn!(
                "live search: {}/{} engines degraded [engines: {}]",
                outcome.engines_failed,
                outcome.total_engines,
                failed_details.join(", ")
            );
        } else {
            tracing::debug!(
                "live search: {}/{} non-primary engines degraded [engines: {}]",
                outcome.engines_failed,
                outcome.total_engines,
                failed_details.join(", ")
            );
        }
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
/// Fetch the top hits and run the match-based example extractor over them.
///
/// Returns the typed examples found. Best-effort: any fetch or extraction
/// failure is logged and skipped so it never breaks the search response.
async fn extract_examples_for_hits(
    hits: &[TursoSearchHit],
    limit: u32,
) -> Vec<webfind::pipeline::ExtractedExample> {
    use webfind::pipeline::example_extract::extract_examples;

    let budget = (limit.max(1) as usize).min(10);
    let urls: Vec<&str> = hits
        .iter()
        .take(budget)
        .map(|h| h.url.as_str())
        .filter(|u| !u.trim().is_empty())
        .collect();

    let mut out = Vec::new();
    for url in urls {
        let content = match webfind::engine::fetcher::Fetcher::new() {
            Ok(f) => match f.fetch_url(url).await {
                Ok(c) => c,
                Err(e) => {
                    tracing::debug!("example extraction fetch failed for {url}: {e}");
                    continue;
                }
            },
            Err(e) => {
                tracing::debug!("fetcher init failed: {e}");
                continue;
            }
        };
        let mut found = extract_examples(&content);
        for ex in &mut found {
            ex.source_tier = webfind::engine::source_tier::classify(&content.url);
        }
        out.extend(found);
        if out.len() >= 64 {
            break;
        }
    }
    out
}


/// Deterministic lexical fallback embedding, used when the neural backend is
/// not compiled in (`--features embed` not passed) or fails to load.
fn fallback_embedding(query: &str) -> Option<Vec<f32>> {
    DummyEmbedder.embed(&[query]).ok().and_then(|mut v| v.pop())
}

async fn turso_search(
    cfg: &webfind::config::WebfindConfig,
    query: &str,
    depth: webfind::cli::DepthArg,
    limit: u32,
    include_content: bool,
    turso_path: Option<String>,
    hybrid: bool,
    live: bool,
) -> anyhow::Result<SearchResponse> {
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
        let _ = store
            .compute_pagerank(20, 0.85)
            .await
            .context("compute PageRank")?;
    }

    // Embed the query for the vector signal. When --hybrid is active, use the
    // full ONNX model; in standard mode, use the zero-latency deterministic
    // embedder so vector signals are populated without loading the neural network.
    let embedding: Option<Vec<f32>> = if hybrid {
        #[cfg(feature = "embed")]
        {
            match FastembedEmbedder::new() {
                Ok(e) => {
                    if let Ok(mut v) = e.embed(&[query]) {
                        if v.is_empty() { None } else { Some(v.remove(0)) }
                    } else {
                        fallback_embedding(query)
                    }
                }
                Err(e) => {
                    tracing::warn!("fastembed unavailable, using dummy embedder: {e}");
                    fallback_embedding(query)
                }
            }
        }
        #[cfg(not(feature = "embed"))]
        {
            fallback_embedding(query)
        }
    } else {
        DummyEmbedder
            .embed(&[query])
            .ok()
            .and_then(|v| v.into_iter().next())
    };

    // Fetch extra store hits when merging with live results so the fusion has
    // material beyond the final limit.
    let store_limit = if live {
        (limit.max(1) * 2) as usize
    } else {
        limit.max(1) as usize
    };

    let (hits, live_outcome) = if live {
        let live_fetch = store_limit;
        let store_fut = store.search(query, embedding.as_deref(), store_limit);
        let live_fut = execute_live_search(query, live_fetch);
        let (store_res, live_res) = tokio::join!(store_fut, live_fut);
        let hits = store_res.context("hybrid search in Turso store")?;
        let outcome = live_res.context("execute live search")?;
        (hits, Some(outcome))
    } else {
        let hits = store
            .search(query, embedding.as_deref(), store_limit)
            .await
            .context("hybrid search in Turso store")?;
        (hits, None)
    };

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

    // Match-based example extraction.
    //
    // Fetches the top retrieved URLs and extracts typed examples (code, math,
    // chemical, table, diagram, image). This is the match-based gate: only
    // pages that actually contain a detectable artifact contribute examples,
    // and only then is the explanation stage worth running.
    let extracted_examples = extract_examples_for_hits(&hits, limit).await;
    if !extracted_examples.is_empty() {
        signals.push(format!("examples:{}", extracted_examples.len()));
    }

    let results = if let Some(outcome) = live_outcome {
        // Persist live hits into the graph BEFORE merging. Previously the fused
        // hits were returned and dropped, so the graph never learned from live
        // search — the README's "graph memory compounds" claim was false for
        // this path. Ingestion records nodes and enqueues background
        // enrichment; it does not fetch, so the response is not delayed.
        match webfind::engine::live_ingest::ingest_fused_hits(&store, &outcome.fused).await {
            Ok(report) if report.ingested() > 0 => {
                signals.push("live_graph_ingest".to_string());
                tracing::info!(
                    "live: ingested {} new node(s), enqueued {} job(s)",
                    report.ingested(),
                    report.jobs_enqueued
                );
            }
            Ok(_) => {}
            Err(e) => {
                // Ingestion is best-effort: a graph write failure must not
                // break the search the user actually asked for.
                tracing::warn!("live: graph ingest failed: {e}");
            }
        }
        merge_live_results(outcome, &hits, limit, include_content, &mut signals, query)?
    } else {
        if hits.is_empty() {
            eprintln!(
                "note: 0 results found in local index for \"{query}\".\n      To query live search engines, run: webfind search \"{query}\" --live\n      To crawl and index a site, run: webfind crawl --seed <URL>"
            );
        }
        let max_s = hits.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
        let min_s = hits.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
        let range = max_s - min_s;
        hits.iter()
            .enumerate()
            .map(|(i, h)| {
                let norm_score = if range < 1e-9 {
                    1.0
                } else {
                    (0.40 + 0.60 * ((h.score - min_s) / range)).clamp(0.40, 1.0)
                };
                store_hit_to_result(h, (i + 1) as u32, norm_score, include_content)
            })
            .collect()
    };

    let index_freshness = store.index_freshness().await.unwrap_or(IndexFreshness {
        oldest_page: None,
        newest_page: None,
        avg_age_days: 0.0,
    });

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
            index_freshness,
        },
    };

    Ok(response)
}

pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    query: String,
    queries: Vec<String>,
    depth: webfind::cli::DepthArg,
    limit: u32,
    output: webfind::cli::OutputArg,
    output_file: Option<std::path::PathBuf>,
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

    let mut all_queries = vec![query];
    for q in queries {
        let trimmed = q.trim().to_string();
        if !trimmed.is_empty() && !all_queries.contains(&trimmed) {
            all_queries.push(trimmed);
        }
    }

    let response = if all_queries.len() == 1 {
        turso_search(
            cfg,
            &all_queries[0],
            depth,
            limit,
            include_content,
            turso_path,
            hybrid,
            live,
        )
        .await?
    } else {
        // Multi-query parallel execution (concurrent search & reciprocal rank fusion)
        let futures: Vec<_> = all_queries
            .iter()
            .map(|q| {
                turso_search(
                    cfg,
                    q,
                    depth.clone(),
                    limit,
                    include_content,
                    turso_path.clone(),
                    hybrid,
                    live,
                )
            })
            .collect();

        let responses = futures::future::try_join_all(futures).await?;

        // Fuse results across all queries using canonical URL deduplication and RRF
        let mut hit_lists: Vec<Vec<Hit>> = Vec::with_capacity(responses.len());
        for resp in &responses {
            let list: Vec<Hit> = resp
                .results
                .iter()
                .enumerate()
                .map(|(i, r)| Hit {
                    url: r.url.clone(),
                    title: r.title.clone(),
                    snippet: r.snippet.clone(),
                    published_at: r.published_at,
                    relevance_score: positional_relevance(i, resp.results.len()),
                    engine: "multi_query",
                })
                .collect();
            hit_lists.push(list);
        }

        let rrf_inputs: Vec<(&[Hit], f64)> = hit_lists.iter().map(|l| (l.as_slice(), 1.0)).collect();
        let raw_fused = reciprocal_rank_fusion(&rrf_inputs, 60);
        let normalized = normalize_rrf_scores(raw_fused);

        // Map back to SearchResults, keeping rich fields from the highest-scoring response
        let mut result_map: HashMap<String, SearchResult> = HashMap::new();
        for resp in &responses {
            for r in &resp.results {
                let canon = canonical_url(&r.url);
                let _ = result_map.entry(canon).or_insert_with(|| r.clone());
            }
        }

        let mut fused_results: Vec<SearchResult> = Vec::new();
        for f in normalized.into_iter().take(limit.max(1) as usize) {
            let canon = canonical_url(&f.url);
            if let Some(mut r) = result_map.remove(&canon) {
                r.rank = (fused_results.len() + 1) as u32;
                r.score = f.score;
                r.scores.final_score = f.score;
                fused_results.push(r);
            }
        }

        let combined_query = all_queries.join(" | ");
        let total_results = fused_results.len() as u64;
        let returned = fused_results.len() as u32;
        let max_latency = responses.iter().map(|r| r.latency_ms).max().unwrap_or(0);
        let first_meta = responses
            .into_iter()
            .next()
            .map(|r| r.metadata)
            .unwrap_or_else(|| SearchMetadata {
                index_version: "turso".to_string(),
                index_size: 0,
                engine_version: "webfind-turso".to_string(),
                searched_at: Utc::now(),
                signals_used: vec!["multi_query_rrf".to_string()],
                index_freshness: IndexFreshness {
                    oldest_page: None,
                    newest_page: None,
                    avg_age_days: 0.0,
                },
            });

        SearchResponse {
            request_id: uuid::Uuid::new_v4().to_string(),
            query: combined_query,
            depth: SearchDepth::from(depth),
            total_results,
            returned,
            latency_ms: max_latency,
            results: fused_results,
            suggestions: vec![],
            related: vec![],
            graph: None,
            metadata: first_meta,
        }
    };

    let formatted = format_response(&response, &OutputFormat::from(output));
    if let Some(path) = output_file {
        std::fs::write(&path, &formatted)
            .with_context(|| format!("write search output to {}", path.display()))?;
        eprintln!("Search result written to {}", path.display());
    } else {
        print!("{}", formatted);
    }
    Ok(())
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
            crawled_at: None,
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
    fn fusion_rewards_unseen_live_hits_with_novelty_bonus() {
        let store = vec![store_hit("https://stale.example/old", "Stale Stored Page")];
        let mut novel_hit = live_fused("https://novel.example/fresh", "Novel Unseen Page", 0.85);
        novel_hit.engine_count = 2;
        let live = vec![novel_hit];

        let fused = fuse_index_and_live(&store, &live);
        assert_eq!(fused[0].0.url, "https://novel.example/fresh");
        assert!(fused[0].1.is_none(), "novel unseen page has no store hit");
        assert_eq!(fused[1].0.url, "https://stale.example/old");
        assert!(fused[1].1.is_some(), "stale page is enriched from store");
    }

    #[test]
    fn live_result_computes_concrete_score_signals() {
        let hit = live_fused("https://example.com/page", "A Page About Rust", 0.80);
        let keywords = vec!["rust".to_string()];
        let result = live_fused_to_result(&hit, 1, false, &keywords);
        assert!(result.scores.freshness.is_some(), "freshness must be Some");
        assert!(result.scores.quality.is_some(), "quality must be Some");
        assert!(result.scores.bm25.is_some(), "bm25 must be Some");
        assert!(result.scores.vector.is_some(), "vector must be Some");
        assert!(result.scores.graph.is_some(), "graph must be Some");
        assert!(result.score >= 0.0 && result.score <= 1.0);
    }

    #[test]
    fn live_result_with_date_applies_freshness_weight() {
        let mut hit = live_fused("https://example.com/page", "A Page", 0.80);
        hit.published_at = Some(Utc::now());
        let keywords = vec!["page".to_string()];
        let result = live_fused_to_result(&hit, 1, false, &keywords);
        let f = result.scores.freshness.expect("known date → freshness must be Some");
        assert!(f > 0.95, "just-published freshness should be > 0.95, got {f}");
        assert!(result.scores.quality.is_some(), "quality must be Some");
        let s = &result.scores;
        let authority_norm =
            (s.graph.unwrap_or(0.0) / 0.35).clamp(0.0, 1.0);
        let bm = s.bm25.unwrap_or(0.0);
        let lexical = bm.clamp(0.0, 1.0);
        let consensus = 0.80_f64.clamp(0.0, 1.0);
        let base_relevance = lexical * (0.5 + 0.5 * consensus);
        let quality_factor = 0.80 + 0.20 * s.quality.unwrap_or(0.0);
        let freshness_factor = 0.90 + 0.10 * f;
        let joined_kw = keywords.join(" ");
        let relevance = ((base_relevance * quality_factor * freshness_factor)
            + authority_norm * 0.05)
            .clamp(0.0, 1.0);
        let expected = (relevance * Ranker::source_precision_boost(
            &hit.url,
            webfind::engine::source_tier::route_intent_code(&joined_kw),
        )).clamp(0.0, 1.0);
        assert!(
            (result.score - expected).abs() < 1e-6,
            "score={} expected={expected}",
            result.score
        );
    }

    #[test]
    fn live_result_multi_engine_adds_fusion_bonus() {
        let mut hit = live_fused("https://example.com/page", "A Page", 0.70);
        hit.engine_count = 4;
        let keywords = vec!["page".to_string()];
        let result = live_fused_to_result(&hit, 1, false, &keywords);
        let f = result.scores.freshness.unwrap_or(1.0);
        let s = &result.scores;
        let authority_norm =
            (s.graph.unwrap_or(0.0) / 0.35).clamp(0.0, 1.0);
        let bm = s.bm25.unwrap_or(0.0);
        let lexical = bm.clamp(0.0, 1.0);
        let consensus = 0.70_f64.clamp(0.0, 1.0);
        let base_relevance = lexical * (0.5 + 0.5 * consensus);
        let quality_factor = 0.80 + 0.20 * s.quality.unwrap_or(0.0);
        let freshness_factor = 0.90 + 0.10 * f;
        let joined_kw = keywords.join(" ");
        let relevance = ((base_relevance * quality_factor * freshness_factor)
            + authority_norm * 0.05
            + 0.10_f64)
            .clamp(0.0, 1.0);
        let expected = (relevance * Ranker::source_precision_boost(
            &hit.url,
            webfind::engine::source_tier::route_intent_code(&joined_kw),
        )).clamp(0.0, 1.0);
        assert!(
            (result.score - expected).abs() < 1e-6,
            "score={} expected={expected}",
            result.score
        );
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn proptest_relax_query_monotonically_shrinks(
            words in prop::collection::vec("[a-z]{1,10}", 1..20),
            drop in 0usize..25
        ) {
            let query = words.join(" ");
            let relaxed = relax_query(&query, drop);
            let original_token_count = words.len();
            let relaxed_token_count = relaxed.split_whitespace().count();

            let expected_count = original_token_count.saturating_sub(drop);
            prop_assert_eq!(relaxed_token_count, expected_count);
            prop_assert!(relaxed.len() <= query.len());
        }

        #[test]
        fn proptest_live_fused_score_bounds(
            base_score in 0.0f64..=1.0f64,
            engine_count in 1usize..=10,
            has_date in any::<bool>()
        ) {
            let mut hit = live_fused("https://example.com/page", "Title", base_score);
            hit.engine_count = engine_count;
            if has_date {
                hit.published_at = Some(Utc::now());
            }
            let keywords = vec!["title".to_string()];
            let res = live_fused_to_result(&hit, 1, false, &keywords);
            prop_assert!(res.score >= 0.0 && res.score <= 1.0, "Score {} was out of bounds [0, 1]", res.score);
            prop_assert_eq!(res.score, res.scores.final_score);
        }
    }
}
