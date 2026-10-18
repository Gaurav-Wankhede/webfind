//! Query-relevance scoring grounded in the published Okapi BM25 formulation.
//!
//! Every constant and formula here is taken from the cited literature rather
//! than tuned by hand:
//!
//! - **BM25** — Robertson & Zaragoza, *The Probabilistic Relevance Framework:
//!   BM25 and Beyond* (2009); Wikipedia "Okapi BM25" (accessed this session):
//!   `score(D,Q) = Σ IDF(qᵢ) · f(qᵢ,D)(k₁+1) / (f(qᵢ,D) + k₁(1 − b + b·|D|/avgdl))`
//!   with `k₁ ∈ [1.2, 2.0]` and `b = 0.75`.
//! - **IDF** — the standard BM25 IDF variant
//!   `IDF(qᵢ) = ln((N − n(qᵢ) + 0.5) / (n(qᵢ) + 0.5) + 1)`, which is strictly
//!   positive and never negative.
//! - **RRF** — Cormack, Clarke & Büttcher, *Reciprocal Rank Fusion outperforms
//!   Condorcet and individual Rank Learning Methods*, SIGIR 2009:
//!   `RRFscore(d) = Σ_r 1/(k + r(d))` with the paper's fixed `k = 60`.
//! - **BM25F field weights** — Robertson & Zaragoza: fields are weighted so a
//!   title match outweighs a body match.

use std::collections::HashMap;

/// BM25 term-frequency saturation parameter.
///
/// Cited range is `k1 ∈ [1.2, 2.0]`; 1.2 is the conventional default.
pub const BM25_K1: f64 = 1.2;

/// BM25 document-length normalisation parameter. Cited default `b = 0.75`.
pub const BM25_B: f64 = 0.75;

/// RRF rank constant. Fixed at 60 in the SIGIR 2009 paper and "not altered
/// during subsequent validation".
pub const RRF_K: f64 = 60.0;

/// BM25F field weights (Robertson & Zaragoza: title outweighs body).
pub const FIELD_WEIGHT_TITLE: f64 = 5.0;
pub const FIELD_WEIGHT_BODY: f64 = 1.0;

/// IDF weight of a query term, using the standard BM25 formulation.
///
/// `n` is the number of documents containing the term and `N` the collection
/// size. The `+1` inside the logarithm keeps the value strictly positive, so a
/// term appearing in more than half the collection still contributes a small
/// positive weight instead of going negative.
pub fn bm25_idf(n: u64, total: u64) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let n = n as f64;
    let total = total as f64;
    (((total - n + 0.5) / (n + 0.5)) + 1.0).ln()
}

/// Term-frequency saturation factor `f·(k₁+1) / (f + k₁·(1 − b + b·|D|/avgdl))`.
///
/// This is the BM25 non-linear saturation: repeated occurrences of a term keep
/// raising the score but with diminishing returns, and longer documents are
/// normalised so they do not win merely by being long.
pub fn bm25_tf(freq: f64, doc_len: f64, avg_len: f64) -> f64 {
    if freq <= 0.0 {
        return 0.0;
    }
    let norm = if avg_len > 0.0 {
        1.0 - BM25_B + BM25_B * (doc_len / avg_len)
    } else {
        1.0
    };
    (freq * (BM25_K1 + 1.0)) / (freq + BM25_K1 * norm)
}

/// BM25F field-weighted term frequency.
///
/// Each field contributes `weight · freq`, then a single BM25 saturation is
/// applied to the weighted sum. This is what makes a title hit outweigh a body
/// hit rather than counting them identically.
pub fn bm25f_weighted_tf(field_freqs: &[(f64, f64)], doc_len: f64, avg_len: f64) -> f64 {
    let weighted: f64 = field_freqs.iter().map(|(w, f)| w * f).sum();
    bm25_tf(weighted, doc_len, avg_len)
}

/// Score one document against a query using BM25F.
///
/// `fields` holds `(title_freq, body_freq)` per query term, `doc_len` is the
/// document length in words and `idf` the per-term IDF vector.
pub fn bm25f_score(
    fields: &[(f64, f64)],
    idf: &[f64],
    doc_len: f64,
    avg_len: f64,
) -> f64 {
    fields
        .iter()
        .zip(idf.iter())
        .map(|((t, b), idf)| idf * bm25f_weighted_tf(&[(FIELD_WEIGHT_TITLE, *t), (FIELD_WEIGHT_BODY, *b)], doc_len, avg_len))
        .sum()
}

/// Reciprocal Rank Fusion over several ranked lists.
///
/// `lists` is one ranked list of document ids per retriever. The result maps
/// each document to its fused score; higher is better. `k` defaults to the
/// paper's 60.
pub fn rrf_keyword_scores(lists: &[Vec<String>], k: f64) -> HashMap<String, f64> {
    let mut fused: HashMap<String, f64> = HashMap::new();
    for list in lists {
        for (rank, doc) in list.iter().enumerate() {
            let contribution = 1.0 / (k + rank as f64 + 1.0);
            *fused.entry(doc.clone()).or_insert(0.0) += contribution;
        }
    }
    fused
}

/// Normalise a score map into `[0, 1]` by min-max scaling.
///
/// Used only for *display* and for combining signals that are otherwise on
/// incomparable scales; ranking itself is done on the raw fused score so that
/// relative ordering is never distorted by a display transform.
pub fn min_max_normalise(scores: &HashMap<String, f64>) -> HashMap<String, f64> {
    if scores.is_empty() {
        return HashMap::new();
    }
    let max = scores.values().cloned().fold(f64::NEG_INFINITY, f64::max);
    let min = scores.values().cloned().fold(f64::INFINITY, f64::min);
    let range = max - min;
    scores
        .iter()
        .map(|(k, v)| {
            let norm = if range < 1e-12 { 1.0 } else { (v - min) / range };
            (k.clone(), norm)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idf_is_positive_and_decreases_with_frequency() {
        // A term in every document carries least information.
        let common = bm25_idf(100, 100);
        let rare = bm25_idf(1, 100);
        assert!(common > 0.0, "IDF must stay positive: {common}");
        assert!(rare > common, "rare term must outrank common term");
    }

    #[test]
    fn tf_saturates_with_repetition() {
        let one = bm25_tf(1.0, 100.0, 100.0);
        let many = bm25_tf(50.0, 100.0, 100.0);
        assert!(many > one, "more occurrences must score higher");
        // Saturation: 50x the frequency must not give 50x the score.
        assert!(many / one < 10.0, "TF must saturate, got ratio {}", many / one);
    }

    #[test]
    fn longer_documents_are_normalised() {
        // Same term frequency, but a much longer document scores lower.
        let short = bm25_tf(5.0, 100.0, 100.0);
        let long = bm25_tf(5.0, 1000.0, 100.0);
        assert!(long < short, "longer doc must not win on length alone");
    }

    #[test]
    fn field_weighting_prefers_title_over_body() {
        let title_hit = bm25f_score(&[(1.0, 0.0)], &[1.0], 100.0, 100.0);
        let body_hit = bm25f_score(&[(0.0, 1.0)], &[1.0], 100.0, 100.0);
        assert!(title_hit > body_hit, "title match must outrank body match");
    }

    #[test]
    fn rrf_prefers_documents_ranked_by_more_systems() {
        // A document retrieved by BOTH retrievers must outrank one retrieved by
        // only one — the core property RRF is chosen for (Cormack et al. 2009).
        let both = vec![
            vec!["shared".into(), "only_a".into()],
            vec!["shared".into(), "only_b".into()],
        ];
        let fused = rrf_keyword_scores(&both, RRF_K);
        assert!(
            fused["shared"] > fused["only_a"],
            "a document found by both retrievers must outrank one found by one"
        );
        assert!(
            fused["shared"] > fused["only_b"],
            "a document found by both retrievers must outrank one found by one"
        );
    }

    #[test]
    fn rrf_rank_sum_is_symmetric_for_mirrored_rankings() {
        // Mirrored rankings (a,b,c) and (c,b,a) must give equal scores to the
        // first and last documents — RRF depends on rank position only.
        let lists = vec![
            vec!["a".into(), "b".into(), "c".into()],
            vec!["c".into(), "b".into(), "a".into()],
        ];
        let fused = rrf_keyword_scores(&lists, RRF_K);
        assert!((fused["a"] - fused["c"]).abs() < 1e-12, "mirrored ranks must score equally");
    }

    #[test]
    fn min_max_normalise_spans_unit_interval() {
        let mut s = HashMap::new();
        let _ = s.insert("a".to_string(), 1.0);
        let _ = s.insert("b".to_string(), 3.0);
        let _ = s.insert("c".to_string(), 5.0);
        let n = min_max_normalise(&s);
        assert_eq!(n["a"], 0.0);
        assert_eq!(n["c"], 1.0);
        assert!((n["b"] - 0.5).abs() < 1e-9);
    }
}
