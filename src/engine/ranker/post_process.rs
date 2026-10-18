use super::Ranker;
use crate::schema::response::SearchResult;
use super::DIVERSITY_PENALTY;

impl Ranker {
    /// Down-rank repeated domains in the top results.
    pub(crate) fn apply_diversity_penalty(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
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
    pub(crate) fn fix_snippets(results: &mut [SearchResult]) {
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
}
