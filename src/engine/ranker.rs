impl Ranker {
    /// Source-precision boost based on URL classification and query code intent.
    #[must_use]
    pub fn source_precision_boost(url: &str, query_intent_code: bool) -> f64 {
        let tier = crate::engine::source_tier::classify(url);
        let mut mult = tier.precision();
        if query_intent_code {
            if tier == crate::engine::source_tier::SourceTier::RawSource {
                mult = (mult * 1.15).min(1.0);
            } else if tier == crate::engine::source_tier::SourceTier::Encyclopedia {
                mult *= 0.70;
            }
        }
        mult
    }
}

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
const W_AX: f64 = 0.20;

const H_BM25: f64 = 0.35;
const H_FRESH: f64 = 0.10;
const H_QUALITY: f64 = 0.07;
const H_GRAPH: f64 = 0.08;
pub mod funnel;
pub mod post_process;
pub use funnel::*;

pub mod scoring;
pub mod examples;

impl Ranker {
    pub fn new() -> Self {
        Self
    }

    /// Calculate Machine-Readability Index (S_AX) evaluating structured agent readiness:
    /// - Has_Structured_Manifest (+0.35): llms.txt or ai-catalog.json present
    /// - Clean_Markdown_Available (+0.25): markdown or structured code available
    /// - Has_OpenAPI_or_MCP (+0.20): OpenAPI schema or MCP server declared
    /// - Direct_API_Affordance (+0.20): copy-executable commands or structured schema
    #[must_use]
    pub fn calculate_ax_score(r: &SearchResult) -> Option<f64> {
        let mut score: f64 = 0.0;
        let mut has_any = false;

        if r.llms_txt.is_some() || r.ai_catalog.is_some() {
            score += 0.35;
            has_any = true;
        }

        if let Some(content) = &r.content
            && (content.markdown.is_some() || content.text.contains("```") || content.text.contains('|'))
        {
            score += 0.25;
            has_any = true;
        }

        if r.openapi_spec.is_some() || r.mcp_server.is_some() {
            score += 0.20;
            has_any = true;
        }

        if let Some(metrics) = &r.metrics
            && metrics.has_structured_data
        {
            score += 0.20;
            has_any = true;
        }

        if has_any {
            Some(score.clamp(0.0, 1.0))
        } else {
            None
        }
    }

    /// Re-rank results by combining BM25 with freshness, quality, graph, vector,
    /// authority, machine-readability (S_AX) and domain-diversity signals.
    /// Extracts high-value code blocks or CLI usage examples from markdown/text.
    pub fn rank(
        &self,
        mut results: Vec<SearchResult>,
        _request: &SearchRequest,
        graph_scores: Option<&HashMap<String, f64>>,
        vector_scores: Option<&HashMap<String, f64>>,
    ) -> Vec<SearchResult> {
        let use_vector = vector_scores.map(|m| !m.is_empty()).unwrap_or(false);
        let (w_bm25, w_fresh, w_quality, w_graph, w_vector, w_authority, w_ax) = if use_vector {
            (H_BM25, H_FRESH, H_QUALITY, H_GRAPH, H_VECTOR, H_AUTHORITY, H_AX)
        } else {
            (W_BM25, W_FRESH, W_QUALITY, W_GRAPH, 0.0, W_AUTHORITY, W_AX)
        };

        // Build domain authority from per-URL graph scores.
        let domain_authority = Self::domain_authority(graph_scores, &results);

        // Pre-evaluate candidate quality, freshness, and examples
        for r in results.iter_mut() {
            let time_ref = r.published_at.unwrap_or(r.crawled_at);
            let freshness = Self::freshness_score(time_ref);
            r.scores.freshness = Some(freshness);

            let structured_boost = r
                .metrics
                .as_ref()
                .map(|m| if m.has_structured_data { 0.05 } else { 0.0 })
                .unwrap_or(0.0);

            let quality = r
                .metrics
                .as_ref()
                .map(|m| {
                    (Self::quality_score(m.reading_ease, m.grade_level) + structured_boost)
                        .clamp(0.0, 1.0)
                })
                .or_else(|| {
                    let text = r.content.as_ref().map(|c| c.text.as_str()).unwrap_or(&r.snippet);
                    let wc = text.split_whitespace().count();
                    if wc >= 3 {
                        let ease = textstat::flesch_reading_ease(text);
                        let grade = textstat::flesch_kincaid_grade(text);
                        Some(Self::quality_score(ease, grade).clamp(0.0, 1.0))
                    } else {
                        Some(0.5)
                    }
                });
            r.scores.quality = quality;

            let authority = *domain_authority.get(&r.domain).unwrap_or(&0.0);
            let graph = graph_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .or(Some(authority));
            r.scores.graph = graph;

            let vector = vector_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .unwrap_or(0.0);
            r.scores.vector = if use_vector { Some(vector) } else { None };

            let ax = Self::calculate_ax_score(r);
            r.scores.ax_score = ax;

            // Extract practical code or usage examples
            let text_source = r
                .content
                .as_ref()
                .and_then(|c| c.markdown.as_ref().or(Some(&c.text)))
                .map(|s| s.as_str())
                .unwrap_or(&r.snippet);
            let extracted = Self::extract_examples(text_source);
            let example_count = extracted.len();
            if !extracted.is_empty() {
                r.examples = Some(extracted);
            }

            // Compute score dynamically over present signals without phantom defaults.
            let (quality_term, active_w_quality) = match quality {
                Some(q) => (w_quality * q, w_quality),
                None => (0.0, 0.0),
            };
            let (graph_term, active_w_graph) = match graph {
                Some(g) => (w_graph * g, w_graph),
                None => (0.0, 0.0),
            };
            let (ax_term, active_w_ax) = match ax {
                Some(a) => (w_ax * a, w_ax),
                None => (0.0, 0.0),
            };
            let example_bonus = if example_count > 0 { 0.05 } else { 0.0 };
            let weight_sum = w_bm25 + w_fresh + active_w_quality + active_w_graph + w_vector + w_authority + active_w_ax;
            let raw_final = w_bm25 * r.scores.bm25.unwrap_or(0.0)
                + w_fresh * freshness
                + quality_term
                + graph_term
                + w_vector * vector
                + w_authority * authority
                + ax_term
                + example_bonus;
            let final_score = if weight_sum > 0.0 {
                (raw_final / weight_sum).clamp(0.001, 1.0)
            } else {
                raw_final.max(0.001)
            };

            r.scores.final_score = final_score;
            r.score = final_score;

            // Assign QualityTier according to dual thresholds
            let q_val = quality.unwrap_or(0.5);
            let v_val = if use_vector { vector } else { 1.0 };
            let tier = if v_val >= FUNNEL_SEMANTIC_THRESHOLD && q_val >= FUNNEL_QUALITY_THRESHOLD {
                crate::schema::response::QualityTier::HighQualityRelevant
            } else if v_val >= FUNNEL_SEMANTIC_THRESHOLD || q_val >= FUNNEL_QUALITY_THRESHOLD {
                crate::schema::response::QualityTier::Marginal
            } else {
                crate::schema::response::QualityTier::RejectedLowQuality
            };
            r.tier = Some(tier);
        }

        // Execute Zero-Heap Stack-Allocated Funnel sorting over candidate descriptors
        let mut funnel = ZeroHeapFunnelEngine::new(
            FUNNEL_SEMANTIC_THRESHOLD as f32,
            FUNNEL_QUALITY_THRESHOLD as f32,
        );
        for (idx, r) in results.iter().enumerate().take(MAX_STACK_CANDIDATES) {
            let desc = CandidateDescriptor {
                index: idx,
                vector_sim: r.scores.vector.unwrap_or(1.0) as f32,
                quality_metric: r.scores.quality.unwrap_or(0.5) as f32,
                bm25_score: r.scores.bm25.unwrap_or(0.0) as f32,
                freshness_score: r.scores.freshness.unwrap_or(0.0) as f32,
                graph_score: r.scores.graph.unwrap_or(0.0) as f32,
                example_count: r.examples.as_ref().map(|e| e.len() as u8).unwrap_or(0),
                final_rank_score: r.score as f32,
                passed_gates: r.tier == Some(crate::schema::response::QualityTier::HighQualityRelevant),
            };
            let _ = funnel.ingest_candidate(desc);
        }

        // Use the funnel sort to reorder results: High quality & relevant first, then by score.
        let ranked = funnel.rank_and_slice();
        let mut reordered: Vec<SearchResult> = ranked
            .iter()
            .map(|d| results[d.index].clone())
            .collect();
        // Append any results that did not fit in the funnel buffer.
        let funnel_indices: std::collections::HashSet<usize> =
            ranked.iter().map(|d| d.index).collect();
        for (idx, r) in results.iter().enumerate() {
            if !funnel_indices.contains(&idx) {
                reordered.push(r.clone());
            }
        }
        results = reordered;

        // Apply a domain-diversity penalty so one domain cannot dominate the top results.
        results = Self::apply_diversity_penalty(results);

        for (i, r) in results.iter_mut().enumerate() {
            r.rank = (i + 1) as u32;
        }

        Self::fix_snippets(&mut results);

        results
    }

}

impl Default for Ranker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

