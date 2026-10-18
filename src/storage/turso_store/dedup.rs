use super::*;
use anyhow::{Context, Result};
use libsql::params;

impl TursoStore {
    pub async fn find_near_duplicate(
        &self,
        candidate: u64,
        exclude_url: &str,
    ) -> Result<Option<(String, u32)>> {
        let mut rows = self
            .conn
            .query(
                "SELECT url, simhash FROM url_nodes WHERE simhash IS NOT NULL AND simhash <> 0 AND id <> ?1",
                params![url_id(exclude_url)],
            )
            .await
            .context("scan fingerprints for near-duplicate")?;
        let threshold = crate::storage::cache_store::SimHash::NEAR_DUP_THRESHOLD;
        let mut best: Option<(String, u32)> = None;
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let stored: i64 = match row.get(1) {
                        Ok(h) => h,
                        Err(_) => continue,
                    };
                    let dist = (candidate as i64 ^ stored).count_ones();
                    let better = match &best {
                        None => true,
                        Some((best_url, best_dist)) => {
                            dist < *best_dist || (dist == *best_dist && url < *best_url)
                        }
                    };
                    if dist <= threshold && better {
                        best = Some((url, dist));
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate fingerprints"),
            }
        }
        Ok(best)
    }

    /// Word floor below which the near-dup gate is skipped.
    ///
    /// Short pages are dominated by shared chrome (nav, footer, cookie
    /// banners), so their fingerprints collide spuriously — the same defect
    /// that motivated the char-trigram -> word-shingle rewrite. Below this
    /// floor the fingerprint is not trustworthy enough to reject on.
    pub const MIN_DEDUP_WORDS: usize = 40;

    /// Minimum substantive word count for a page to be worth indexing.
    ///
    /// Pages under this are thin: they satisfy no search intent and crowd out
    /// real results. Cited thresholds cluster around 300 words
    /// (https://polytraffic.com/articles/identify-thin-content-audit); 300 is
    /// used here as a conservative screening floor, not a length requirement.
    
    pub async fn find_duplicate(&self, url: &str, content_text: &str) -> Result<Option<String>> {
        let words = content_text.split_whitespace().count();
        if words < Self::MIN_DEDUP_WORDS {
            return Ok(None);
        }
        let candidate = crate::storage::cache_store::SimHash::compute(content_text);
        let Some((existing_url, _dist)) = self.find_near_duplicate(candidate, url).await? else {
            return Ok(None);
        };
        // First-seen: compare discovery timestamps.
        let existing_seen = self
            .get_url(&existing_url)
            .await
            .map(|n| n.discovered_at);
        let incoming_seen = self.get_url(url).await.map(|n| n.discovered_at);
        let existing_is_canonical = match (existing_seen, incoming_seen) {
            (Some(e), Some(i)) => e < i || (e == i && existing_url.as_str() < url),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => true,
        };
        if existing_is_canonical {
            Ok(Some(existing_url))
        } else {
            Ok(None)
        }
    }

pub const MIN_QUALITY_WORDS: usize = 300;

    /// Thin-content quality gate evaluated at ingest.
    ///
    /// Applies a **word floor** on the already-boilerplate-stripped
    /// `content_text` (the fetcher purges docs/feedback chrome before this
    /// point, so the count reflects substantive text).
    ///
    /// Honest scope limitation: a true *unique-content ratio* — the stronger
    /// thin-content signal, where a 2,000-word page that is 1,800 words of
    /// shared template text is still thin
    /// (https://contentify.ai/blog/programmatic-seo-quality-gates/) — requires
    /// comparing each page against its sibling corpus, which is a larger
    /// feature. This gate implements only the word floor; the ratio check is
    /// deliberately not faked. `Purgeable fraction` is reported as a
    /// diagnostic signal, not used as a rejection criterion.
    pub fn check_quality(content_text: &str) -> QualityVerdict {
        let words = content_text.split_whitespace().count();
        if words < Self::MIN_QUALITY_WORDS {
            return QualityVerdict::Reject("thin content below word floor");
        }
        QualityVerdict::Accept
    }
}

/// Outcome of the thin-content quality gate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QualityVerdict {
    /// Content is substantive enough to index.
    Accept,
    /// Content is too thin to index, with the reason.
    Reject(&'static str),
}

/// A hybrid-search hit with the fused RRF score and the signals it matched.
#[derive(Debug, Clone)]
pub struct HybridHit {
    pub url: String,
    pub score: f64,
    pub signals: Vec<String>,
    /// Raw RRF contribution from the BM25 signal (None when not matched).
    pub bm25_score: Option<f64>,
    /// Raw RRF contribution from the vector signal (None when not matched).
    pub vector_score: Option<f64>,
    /// Raw RRF contribution from the graph/PageRank signal (None when not matched).
    pub graph_score: Option<f64>,
    /// Raw RRF contribution from the freshness signal (None when not matched).
    pub freshness_score: Option<f64>,
}

/// A renderable search hit with a title + excerpt derived from stored content.
#[derive(Debug, Clone)]
pub struct TursoSearchHit {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    pub score: f64,
    pub signals: Vec<String>,
    pub bm25_score: Option<f64>,
    pub vector_score: Option<f64>,
    pub graph_score: Option<f64>,
    pub crawled_at: Option<DateTime<Utc>>,
}
