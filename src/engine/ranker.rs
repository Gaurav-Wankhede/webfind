use std::collections::HashMap;

use crate::schema::request::SearchRequest;
use crate::schema::response::SearchResult;

/// Ranker applies additional scoring signals on top of lexical and vector scores.
///
/// Default signals: BM25, freshness, quality, graph/PageRank.
/// When vector scores are supplied, vector is added and weights are re-balanced.
/// Default weights: BM25=0.55, freshness=0.20, quality=0.15, graph=0.10
/// Hybrid weights: BM25=0.45, freshness=0.15, quality=0.10, graph=0.10, vector=0.20
pub struct Ranker;

const W_BM25: f64 = 0.55;
const W_FRESH: f64 = 0.20;
const W_QUALITY: f64 = 0.15;
const W_GRAPH: f64 = 0.10;

const H_BM25: f64 = 0.45;
const H_FRESH: f64 = 0.15;
const H_QUALITY: f64 = 0.10;
const H_GRAPH: f64 = 0.10;
const H_VECTOR: f64 = 0.20;

impl Ranker {
    pub fn new() -> Self {
        Self
    }

    /// Re-rank results by combining BM25 with freshness, quality, graph, and optional vector signals.
    pub fn rank(
        &self,
        mut results: Vec<SearchResult>,
        _request: &SearchRequest,
        graph_scores: Option<&HashMap<String, f64>>,
        vector_scores: Option<&HashMap<String, f64>>,
    ) -> Vec<SearchResult> {
        let use_vector = vector_scores.map(|m| !m.is_empty()).unwrap_or(false);
        let (w_bm25, w_fresh, w_quality, w_graph, w_vector) = if use_vector {
            (H_BM25, H_FRESH, H_QUALITY, H_GRAPH, H_VECTOR)
        } else {
            (W_BM25, W_FRESH, W_QUALITY, W_GRAPH, 0.0)
        };

        for r in results.iter_mut() {
            let time_ref = r.published_at.unwrap_or(r.crawled_at);
            let freshness = Self::freshness_score(time_ref);
            r.scores.freshness = Some(freshness);

            let quality = r
                .metrics
                .as_ref()
                .map(|m| Self::quality_score(m.reading_ease, m.grade_level))
                .unwrap_or(0.5);
            r.scores.quality = Some(quality);

            let graph = graph_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .unwrap_or(0.0);
            r.scores.graph = Some(graph);

            let vector = vector_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .unwrap_or(0.0);
            r.scores.vector = if use_vector { Some(vector) } else { None };

            let final_score = w_bm25 * r.scores.bm25
                + w_fresh * freshness
                + w_quality * quality
                + w_graph * graph
                + w_vector * vector;

            r.scores.final_score = final_score;
            r.score = final_score;
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        for (i, r) in results.iter_mut().enumerate() {
            r.rank = (i + 1) as u32;
        }

        results
    }

    /// Freshness score (0.0–1.0) based on page age.
    /// Exponential decay with 180-day half-life.
    pub fn freshness_score(crawled_at: chrono::DateTime<chrono::Utc>) -> f64 {
        let age_days = (chrono::Utc::now() - crawled_at).num_days().max(0) as f64;
        (-age_days / 180.0).exp()
    }

    /// Quality proxy from readability metrics.
    pub fn quality_score(reading_ease: f64, grade_level: f64) -> f64 {
        let ease_norm = (reading_ease / 100.0).clamp(0.0, 1.0);
        let grade_penalty = if (6.0..=12.0).contains(&grade_level) {
            1.0
        } else if grade_level < 6.0 {
            grade_level / 6.0
        } else {
            (20.0 - grade_level) / 8.0
        }
        .clamp(0.0, 1.0);

        0.7 * ease_norm + 0.3 * grade_penalty
    }
}

impl Default for Ranker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_freshness_recent() {
        let recent = chrono::Utc::now() - chrono::Duration::hours(1);
        let score = Ranker::freshness_score(recent);
        assert!(score > 0.9, "recent page should score high: {}", score);
    }

    #[test]
    fn test_freshness_old() {
        let old = chrono::Utc::now() - chrono::Duration::days(400);
        let score = Ranker::freshness_score(old);
        assert!(score < 0.15, "old page should score low: {}", score);
    }

    #[test]
    fn test_freshness_one_year() {
        let one_year = chrono::Utc::now() - chrono::Duration::days(365);
        let score = Ranker::freshness_score(one_year);
        assert!(
            score > 0.1 && score < 0.15,
            "one year old should score ~0.13: {}",
            score
        );
    }

    #[test]
    fn test_freshness_one_month() {
        let one_month = chrono::Utc::now() - chrono::Duration::days(30);
        let score = Ranker::freshness_score(one_month);
        assert!(score > 0.8, "one month old should score high: {}", score);
    }

    #[test]
    fn test_quality_easy_read() {
        let score = Ranker::quality_score(70.0, 8.0);
        assert!(score > 0.6, "easy readable content should score well");
    }

    #[test]
    fn test_quality_hard_read() {
        let score = Ranker::quality_score(20.0, 18.0);
        assert!(score < 0.3, "hard content should score poorly");
    }

    #[test]
    fn test_freshness_domination() {
        let now = chrono::Utc::now();
        let fresh = Ranker::freshness_score(now);
        let stale = Ranker::freshness_score(now - chrono::Duration::days(365));
        assert!(
            fresh > stale * 5.0,
            "fresh should dominate stale: {} vs {}",
            fresh,
            stale
        );
    }

    #[test]
    fn test_rank_applies_graph_boost() {
        use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
        use crate::schema::response::{ContentMetrics, ScoreBreakdown, SearchResult};
        let now = chrono::Utc::now();
        let base = SearchResult {
            rank: 0,
            url: "https://example.com/base".to_string(),
            title: "Base".to_string(),
            snippet: "".to_string(),
            domain: "example.com".to_string(),
            published_at: Some(now),
            modified_at: None,
            crawled_at: now,
            author: None,
            site_name: None,
            score: 0.0,
            scores: ScoreBreakdown {
                bm25: 0.5,
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                final_score: 0.0,
            },
            content: None,
            keywords: None,
            metrics: Some(ContentMetrics {
                reading_ease: 70.0,
                grade_level: 8.0,
                fog_index: 8.0,
                sentence_count: 10,
                avg_words_per_sentence: 12.0,
                language: "en".to_string(),
                language_confidence: 0.99,
                has_structured_data: false,
                schema_type: None,
            }),
            favicon: None,
            thumbnail: None,
            language: "en".to_string(),
            content_type: ContentType::Any,
        };
        let mut boosted = base.clone();
        boosted.url = "https://example.com/boosted".to_string();
        boosted.scores.bm25 = 0.5;
        let request = SearchRequest {
            query: "test".to_string(),
            depth: SearchDepth::Standard,
            limit: 10,
            output: OutputFormat::Json,
            language: None,
            date_range: None,
            domains: None,
            content_type: Some(ContentType::Any),
            include_content: false,
            include_graph: false,
            include_keywords: false,
            include_metrics: false,
            hybrid: false,
        };
        let mut graph_scores = HashMap::new();
        graph_scores.insert(base.url.clone(), 0.0);
        graph_scores.insert(boosted.url.clone(), 1.0);

        let ranker = Ranker::new();
        let ranked = ranker.rank(
            vec![base.clone(), boosted.clone()],
            &request,
            Some(&graph_scores),
            None,
        );

        assert_eq!(ranked[0].url, boosted.url);
        assert!(ranked[0].scores.graph.unwrap() > ranked[1].scores.graph.unwrap());
    }
}
