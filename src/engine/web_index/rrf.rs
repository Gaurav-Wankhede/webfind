//! Reciprocal Rank Fusion: merge per-engine ranked lists into one score map.
//!
//! Each URL scores `sum(weight / (k + rank))` across every list it appears in,
//! so a result that several engines rank highly wins over a result one engine
//! ranks first. The constant `k` dampens the influence of top ranks; 60 is the
//! conventional default. A per-list `weight` lets callers favor one source over
//! another (e.g. fresh live results over a stale local index).

use ahash::AHashMap;
use chrono::{DateTime, Utc};

use super::Hit;
use super::util::{aggregation_key, canonical_url, extract_keywords, keyword_match_count};

/// A URL fused across engines, carrying the best-ranked title and snippet.
#[derive(Debug, Clone)]
pub struct FusedHit {
    /// Destination URL of the highest-ranked occurrence.
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub published_at: Option<DateTime<Utc>>,
    /// Fused RRF score.
    pub score: f64,
    /// Number of engines that returned this URL.
    pub engine_count: usize,
    /// Names of the engines that returned this URL.
    pub engines: Vec<&'static str>,
}

/// Fuse ranked engine lists with RRF.
///
/// Each entry is `(list, weight)`; a list's contribution is
/// `weight / (k + rank)`. The first (highest-ranked) occurrence of a URL
/// supplies its title, snippet, and publication date; later occurrences only
/// contribute score and engine membership. Dedup uses [`canonical_url`], so
/// tracking-parameter variants of the same page fuse into one result.
///
/// Ties (identical fused scores) break deterministically by URL so repeated
/// queries return the same ordering instead of a HashMap-iteration coin flip.
#[must_use]
pub fn reciprocal_rank_fusion(lists: &[(&[Hit], f64)], k: u32) -> Vec<FusedHit> {
    let k = k.max(1);
    let mut by_url: AHashMap<String, FusedHit> = AHashMap::new();

    for (list, weight) in lists {
        let weight = weight.max(0.0);
        for (rank, hit) in list.iter().enumerate() {
            let rank = u32::try_from(rank).unwrap_or(u32::MAX);
            let contribution = weight / (f64::from(k) + f64::from(rank.saturating_add(1)));
            let key = canonical_url(&hit.url);
            match by_url.get_mut(&key) {
                Some(acc) => {
                    acc.score += contribution;
                    acc.engine_count += 1;
                    acc.engines.push(hit.engine);
                }
                None => {
                    by_url.insert(
                        key,
                        FusedHit {
                            url: hit.url.clone(),
                            title: hit.title.clone(),
                            snippet: hit.snippet.clone(),
                            published_at: hit.published_at,
                            score: contribution,
                            engine_count: 1,
                            engines: vec![hit.engine],
                        },
                    );
                }
            }
        }
    }

    let mut fused: Vec<FusedHit> = by_url.into_values().collect();
    fused.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.url.cmp(&b.url)));
    fused
}

/// Normalize raw RRF scores in-place to the `[0.0, 1.0]` range via min-max scaling.
///
/// Raw RRF fractions top out at `weight / (k + 1)` ≈ 0.033 for k=60, which
/// looks artificially low to users. This maps the best hit to 1.0 and the
/// worst to 0.0. When all scores are identical (or a single hit exists), every
/// score is set to 1.0.
#[must_use]
pub fn normalize_rrf_scores(mut hits: Vec<FusedHit>) -> Vec<FusedHit> {
    if hits.is_empty() {
        return hits;
    }
    let max = hits.iter().map(|h| h.score).fold(f64::NEG_INFINITY, f64::max);
    let min = hits.iter().map(|h| h.score).fold(f64::INFINITY, f64::min);
    let range = max - min;
    for h in &mut hits {
        h.score = if range < f64::EPSILON {
            1.0
        } else {
            (h.score - min) / range
        };
    }
    hits
}

/// Aggregate RRF-fused hits into concept groups and rank them.
///
/// Engines return different URLs for the same concept (tokio.rs/,
/// tokio.rs/tokio/tutorial/async, docs.rs/tokio), so per-URL RRF scores stay
/// flat (~1/(k+1) each) and the URL tie-break decides arbitrarily. Grouping by
/// [`aggregation_key`] (host + first path segment; homepages merge into the
/// same-host section group) accumulates evidence across URL variants, and a
/// keyword-aware tie-break prefers hits whose title/snippet matches the query.
///
/// The representative of each group carries the group's accumulated score and
/// engine membership; its own title/snippet is the member with the most query
/// keyword matches (ties: lexicographically smallest URL).
#[must_use]
pub fn aggregate_fused(fused: Vec<FusedHit>, query: &str) -> Vec<FusedHit> {
    let keywords = extract_keywords(query);
    let mut by_key: AHashMap<String, FusedHit> = AHashMap::new();

    for hit in fused {
        let key = aggregation_key(&hit.url);
        match by_key.get_mut(&key) {
            Some(acc) => absorb(acc, &hit, &keywords),
            None => {
                by_key.insert(key, hit);
            }
        }
    }

    // Merge homepage (host-only) groups into the strongest same-host section
    // group so tokio.rs/ + tokio.rs/tokio/... accumulate as one concept.
    let homepages: Vec<(String, FusedHit)> = by_key
        .iter()
        .filter(|(key, _)| !key.contains('/'))
        .map(|(key, hit)| (key.clone(), hit.clone()))
        .collect();
    for (host, homepage) in homepages {
        let best_path_key = by_key
            .keys()
            .filter(|key| key.starts_with(&format!("{host}/")))
            .max_by(|a, b| by_key[*a].score.total_cmp(&by_key[*b].score))
            .cloned();
        if let Some(path_key) = best_path_key
            && let Some(target) = by_key.get_mut(&path_key)
        {
            absorb(target, &homepage, &keywords);
            by_key.remove(&host);
        }
    }

    let mut aggregated: Vec<FusedHit> = by_key.into_values().collect();
    aggregated.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| {
                keyword_match_count(&keywords, &b.title, &b.snippet)
                    .cmp(&keyword_match_count(&keywords, &a.title, &a.snippet))
            })
            .then_with(|| a.url.cmp(&b.url))
    });
    aggregated
}

/// Merge `other` into `target`: accumulate score and engine membership, and
/// keep the better representative (more query keyword matches, then smaller
/// URL).
fn absorb(target: &mut FusedHit, other: &FusedHit, keywords: &[String]) {
    target.score += other.score;
    target.engine_count += other.engine_count;
    for engine in &other.engines {
        if !target.engines.contains(engine) {
            target.engines.push(engine);
        }
    }
    let target_matches = keyword_match_count(keywords, &target.title, &target.snippet);
    let other_matches = keyword_match_count(keywords, &other.title, &other.snippet);
    if other_matches > target_matches || (other_matches == target_matches && other.url < target.url)
    {
        target.url = other.url.clone();
        target.title = other.title.clone();
        target.snippet = other.snippet.clone();
        target.published_at = other.published_at;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn fusion_ranks_shared_results_first() {
        let ddg = vec![
            hit("https://a.example", "ddg"),
            hit("https://b.example", "ddg"),
        ];
        let bing = vec![
            hit("https://b.example", "bing"),
            hit("https://c.example", "bing"),
        ];
        let fused = reciprocal_rank_fusion(&[(&ddg, 1.0), (&bing, 1.0)], 60);
        assert_eq!(fused[0].url, "https://b.example");
        assert_eq!(fused[0].engine_count, 2);
        assert_eq!(fused.len(), 3);
    }

    #[test]
    fn fusion_respects_rank_within_single_list() {
        let list = vec![
            hit("https://a.example", "ddg"),
            hit("https://b.example", "ddg"),
        ];
        let fused = reciprocal_rank_fusion(&[(&list, 1.0)], 60);
        assert_eq!(fused[0].url, "https://a.example");
        assert_eq!(fused[1].url, "https://b.example");
    }

    #[test]
    fn fusion_dedups_by_canonical_url() {
        let a = vec![hit("https://a.example/path?utm_source=x", "ddg")];
        let b = vec![hit("https://a.example/path", "bing")];
        let fused = reciprocal_rank_fusion(&[(&a, 1.0), (&b, 1.0)], 60);
        assert_eq!(fused.len(), 1);
        assert_eq!(fused[0].engine_count, 2);
    }

    #[test]
    fn fusion_keeps_best_title_from_first_occurrence() {
        let a = vec![Hit {
            url: "https://a.example".to_string(),
            title: "Best Title".to_string(),
            snippet: "Best snippet".to_string(),
            published_at: None,
            relevance_score: 1.0,
            engine: "ddg",
        }];
        let b = vec![Hit {
            url: "https://a.example".to_string(),
            title: "Worse Title".to_string(),
            snippet: "Worse snippet".to_string(),
            published_at: None,
            relevance_score: 1.0,
            engine: "bing",
        }];
        let fused = reciprocal_rank_fusion(&[(&a, 1.0), (&b, 1.0)], 60);
        assert_eq!(fused[0].title, "Best Title");
        assert_eq!(fused[0].snippet, "Best snippet");
    }

    #[test]
    fn fusion_handles_empty_input() {
        assert!(reciprocal_rank_fusion(&[], 60).is_empty());
        let empty: Vec<Hit> = vec![];
        assert!(reciprocal_rank_fusion(&[(&empty, 1.0)], 60).is_empty());
    }

    #[test]
    fn fusion_k_damps_scores() {
        let list = vec![hit("https://a.example", "ddg")];
        let k1 = reciprocal_rank_fusion(&[(&list, 1.0)], 1);
        let k60 = reciprocal_rank_fusion(&[(&list, 1.0)], 60);
        assert!(k1[0].score > k60[0].score);
    }

    #[test]
    fn fusion_weight_scales_contribution() {
        let list = vec![hit("https://a.example", "ddg")];
        let weighted = reciprocal_rank_fusion(&[(&list, 2.0)], 60);
        let plain = reciprocal_rank_fusion(&[(&list, 1.0)], 60);
        assert!((weighted[0].score - 2.0 * plain[0].score).abs() < 1e-9);
    }

    #[test]
    fn fusion_ties_break_deterministically_by_url() {
        let a = vec![hit("https://z.example", "ddg")];
        let b = vec![hit("https://a.example", "bing")];
        let fused = reciprocal_rank_fusion(&[(&a, 1.0), (&b, 1.0)], 60);
        // Both score 1/61; the URL tie-break must order them stably.
        assert_eq!(fused[0].url, "https://a.example");
        assert_eq!(fused[1].url, "https://z.example");
    }

    fn fused_hit(url: &str, title: &str, score: f64) -> FusedHit {
        FusedHit {
            url: url.to_string(),
            title: title.to_string(),
            snippet: String::new(),
            published_at: None,
            score,
            engine_count: 1,
            engines: vec!["stub"],
        }
    }

    #[test]
    fn aggregation_merges_same_host_path_variants() {
        let fused = vec![
            fused_hit(
                "https://tokio.rs/tokio/tutorial/async",
                "Tokio - An asynchronous Rust runtime",
                0.0164,
            ),
            fused_hit(
                "https://tokio.rs/tokio/tutorial/select",
                "Tokio select",
                0.0164,
            ),
        ];
        let aggregated = aggregate_fused(fused, "rust async runtime tokio");
        assert_eq!(aggregated.len(), 1);
        assert_eq!(aggregated[0].url, "https://tokio.rs/tokio/tutorial/async");
        assert!((aggregated[0].score - 0.0328).abs() < 1e-9);
        assert_eq!(aggregated[0].engine_count, 2);
    }

    #[test]
    fn aggregation_homepage_merges_into_same_host_section() {
        let fused = vec![
            fused_hit("https://tokio.rs/", "Tokio", 0.0164),
            fused_hit(
                "https://tokio.rs/tokio/tutorial/async",
                "Tokio - An asynchronous Rust runtime",
                0.0164,
            ),
        ];
        let aggregated = aggregate_fused(fused, "rust async runtime tokio");
        assert_eq!(aggregated.len(), 1);
        // The section page (4 keyword matches) represents the group, not the
        // bare homepage (1 match).
        assert_eq!(aggregated[0].url, "https://tokio.rs/tokio/tutorial/async");
        assert!((aggregated[0].score - 0.0328).abs() < 1e-9);
    }

    #[test]
    fn aggregation_keeps_distinct_sections_separate() {
        let fused = vec![
            fused_hit("https://arxiv.org/abs/2602.07455", "RustCompCert", 0.0164),
            fused_hit("https://arxiv.org/pdf/2608.20677", "Async/Await", 0.0164),
        ];
        let aggregated = aggregate_fused(fused, "rust async runtime tokio");
        assert_eq!(aggregated.len(), 2);
    }

    #[test]
    fn aggregation_keyword_tie_break_ranks_relevant_first() {
        let fused = vec![
            fused_hit("https://arxiv.org/abs/2602.07455", "RustCompCert", 0.0164),
            fused_hit(
                "https://tokio.rs/tokio/tutorial/async",
                "Tokio - An asynchronous Rust runtime",
                0.0164,
            ),
        ];
        let aggregated = aggregate_fused(fused, "rust async runtime tokio");
        // Equal scores; the tokio page matches all 4 keywords and must win.
        assert_eq!(aggregated[0].url, "https://tokio.rs/tokio/tutorial/async");
        assert_eq!(aggregated[1].url, "https://arxiv.org/abs/2602.07455");
    }

    #[test]
    fn aggregation_is_deterministic() {
        let fused = vec![
            fused_hit("https://z.example/a", "Zed", 0.0164),
            fused_hit("https://a.example/b", "Alpha", 0.0164),
        ];
        let first = aggregate_fused(fused.clone(), "query");
        let second = aggregate_fused(fused, "query");
        assert_eq!(
            first.iter().map(|f| f.url.clone()).collect::<Vec<_>>(),
            second.iter().map(|f| f.url.clone()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn normalize_empty_is_noop() {
        assert!(normalize_rrf_scores(vec![]).is_empty());
    }

    #[test]
    fn normalize_single_hit_becomes_one() {
        let hits = vec![fused_hit("https://a.example", "A", 0.032)];
        let out = normalize_rrf_scores(hits);
        assert!((out[0].score - 1.0).abs() < 1e-9);
    }

    #[test]
    fn normalize_scales_to_unit_interval() {
        let hits = vec![
            fused_hit("https://a.example", "A", 0.064),
            fused_hit("https://b.example", "B", 0.032),
            fused_hit("https://c.example", "C", 0.016),
        ];
        let out = normalize_rrf_scores(hits);
        assert!((out[0].score - 1.0).abs() < 1e-9, "top must be 1.0");
        assert!((out[2].score - 0.0).abs() < 1e-9, "bottom must be 0.0");
        for h in &out {
            assert!(h.score >= 0.0 && h.score <= 1.0, "score out of [0,1]: {}", h.score);
        }
        // Ordering preserved
        assert!(out[0].score > out[1].score && out[1].score > out[2].score);
    }

    #[test]
    fn normalize_identical_scores_all_become_one() {
        let hits = vec![
            fused_hit("https://a.example", "A", 0.032),
            fused_hit("https://b.example", "B", 0.032),
        ];
        let out = normalize_rrf_scores(hits);
        assert!((out[0].score - 1.0).abs() < 1e-9);
        assert!((out[1].score - 1.0).abs() < 1e-9);
    }
}
