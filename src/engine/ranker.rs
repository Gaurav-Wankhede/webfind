use std::collections::HashMap;

use crate::schema::request::SearchRequest;
use crate::schema::response::SearchResult;

/// Ranker applies additional scoring signals on top of lexical and vector scores.
///
/// Default signals: BM25, freshness, quality, graph/PageRank, authority, diversity.
/// When vector scores are supplied, vector is added and weights are re-balanced.
/// Default weights: BM25=0.50, freshness=0.15, quality=0.10, graph=0.10, authority=0.10
/// Hybrid weights: BM25=0.40, freshness=0.12, quality=0.08, graph=0.10, vector=0.20, authority=0.10
pub struct Ranker;

const W_BM25: f64 = 0.50;
const W_FRESH: f64 = 0.15;
const W_QUALITY: f64 = 0.10;
const W_GRAPH: f64 = 0.10;
const W_AUTHORITY: f64 = 0.10;

const H_BM25: f64 = 0.40;
const H_FRESH: f64 = 0.12;
const H_QUALITY: f64 = 0.08;
const H_GRAPH: f64 = 0.10;
const H_VECTOR: f64 = 0.20;
const H_AUTHORITY: f64 = 0.10;

/// Diversity penalty factor per prior occurrence of the same domain.
const DIVERSITY_PENALTY: f64 = 0.85;

impl Ranker {
    pub fn new() -> Self {
        Self
    }

    /// Re-rank results by combining BM25 with freshness, quality, graph, vector,
    /// authority and domain-diversity signals.
    pub fn rank(
        &self,
        mut results: Vec<SearchResult>,
        _request: &SearchRequest,
        graph_scores: Option<&HashMap<String, f64>>,
        vector_scores: Option<&HashMap<String, f64>>,
    ) -> Vec<SearchResult> {
        let use_vector = vector_scores.map(|m| !m.is_empty()).unwrap_or(false);
        let (w_bm25, w_fresh, w_quality, w_graph, w_vector, w_authority) = if use_vector {
            (H_BM25, H_FRESH, H_QUALITY, H_GRAPH, H_VECTOR, H_AUTHORITY)
        } else {
            (W_BM25, W_FRESH, W_QUALITY, W_GRAPH, 0.0, W_AUTHORITY)
        };

        // Build domain authority from per-URL graph scores.
        let domain_authority = Self::domain_authority(graph_scores, &results);

        for r in results.iter_mut() {
            let time_ref = r.published_at.unwrap_or(r.crawled_at);
            let freshness = Self::freshness_score(time_ref);
            r.scores.freshness = Some(freshness);

            let structured_boost = r
                .metrics
                .as_ref()
                .map(|m| if m.has_structured_data { 0.05 } else { 0.0 })
                .unwrap_or(0.0);

            let quality = r.metrics.as_ref().map(|m| {
                (Self::quality_score(m.reading_ease, m.grade_level) + structured_boost)
                    .clamp(0.0, 1.0)
            });
            r.scores.quality = quality;

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

            let authority = *domain_authority.get(&r.domain).unwrap_or(&0.0);

            // Compute score dynamically over present signals without phantom defaults.
            let (quality_term, active_w_quality) = match quality {
                Some(q) => (w_quality * q, w_quality),
                None => (0.0, 0.0),
            };
            let weight_sum = w_bm25 + w_fresh + active_w_quality + w_graph + w_vector + w_authority;
            let raw_final = w_bm25 * r.scores.bm25.unwrap_or(0.0)
                + w_fresh * freshness
                + quality_term
                + w_graph * graph
                + w_vector * vector
                + w_authority * authority;
            let final_score = if weight_sum > 0.0 {
                (raw_final / weight_sum).clamp(0.0, 1.0)
            } else {
                raw_final
            };

            r.scores.final_score = final_score;
            r.score = final_score;
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Apply a domain-diversity penalty so one domain cannot dominate the top results.
        results = Self::apply_diversity_penalty(results);

        for (i, r) in results.iter_mut().enumerate() {
            r.rank = (i + 1) as u32;
        }

        Self::fix_snippets(&mut results);

        results
    }

    /// Compute domain-level authority as the mean graph score of pages on that domain.
    fn domain_authority(
        graph_scores: Option<&HashMap<String, f64>>,
        results: &[SearchResult],
    ) -> ahash::AHashMap<String, f64> {
        let mut sums: ahash::AHashMap<String, (f64, usize)> = ahash::AHashMap::new();
        for r in results {
            let page_auth = graph_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .unwrap_or(0.0);
            let entry = sums.entry(r.domain.clone()).or_insert((0.0, 0));
            entry.0 += page_auth;
            entry.1 += 1;
        }
        sums.into_iter()
            .map(|(domain, (sum, count))| {
                (domain, if count > 0 { sum / count as f64 } else { 0.0 })
            })
            .collect()
    }

    /// Down-rank repeated domains in the top results.
    fn apply_diversity_penalty(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
        let mut domain_counts: ahash::AHashMap<String, usize> = ahash::AHashMap::new();
        for r in results.iter_mut() {
            let count = domain_counts.entry(r.domain.clone()).or_insert(0);
            if *count > 0 {
                let penalty = DIVERSITY_PENALTY.powi(*count as i32);
                r.score *= penalty;
                r.scores.final_score *= penalty;
            }
            *count += 1;
        }

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results
    }

    /// Ensure no result has an empty snippet — fall back to content excerpt or text.
    fn fix_snippets(results: &mut [SearchResult]) {
        for r in results.iter_mut() {
            if !r.snippet.is_empty() {
                continue;
            }
            if let Some(ref content) = r.content {
                if !content.excerpt.is_empty() {
                    r.snippet = content.excerpt.clone();
                    continue;
                }
                if !content.text.is_empty() {
                    let end = content
                        .text
                        .char_indices()
                        .nth(200)
                        .map(|(i, _)| i)
                        .unwrap_or(content.text.len());
                    r.snippet = format!("{}…", &content.text[..end]);
                    continue;
                }
            }
            r.snippet = r.title.clone();
        }
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
                bm25: Some(0.5),
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
            content_type: "text".to_string(),
        };
        let mut boosted = base.clone();
        boosted.url = "https://example.com/boosted".to_string();
        boosted.scores.bm25 = Some(0.5);
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

    #[test]
    fn test_diversity_penalty_does_not_dominate_relevance() {
        use crate::schema::request::{ContentType, OutputFormat, SearchDepth, SearchRequest};
        use crate::schema::response::{ContentMetrics, ScoreBreakdown, SearchResult};
        let now = chrono::Utc::now();
        let dup = SearchResult {
            rank: 0,
            url: "https://example.com/a".to_string(),
            title: "A".to_string(),
            snippet: "".to_string(),
            domain: "example.com".to_string(),
            published_at: Some(now),
            modified_at: None,
            crawled_at: now,
            author: None,
            site_name: None,
            score: 0.0,
            scores: ScoreBreakdown {
                bm25: Some(0.9),
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
            content_type: "text".to_string(),
        };
        let mut dup2 = dup.clone();
        dup2.url = "https://example.com/b".to_string();
        dup2.scores.bm25 = Some(0.85);
        let mut unique = dup.clone();
        unique.url = "https://other.com/c".to_string();
        unique.domain = "other.com".to_string();
        unique.scores.bm25 = Some(0.7);

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
        let ranker = Ranker::new();
        let ranked = ranker.rank(vec![dup, dup2, unique], &request, None, None);

        // The single unique result should outrank the slightly weaker duplicate
        // because diversity penalty pushes the second example.com page below it.
        assert_eq!(ranked[0].url, "https://example.com/a");
        assert_eq!(ranked[1].url, "https://other.com/c");
        assert_eq!(ranked[2].url, "https://example.com/b");
    }
}
