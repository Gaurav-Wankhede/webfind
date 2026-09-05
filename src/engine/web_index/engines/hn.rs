//! Hacker News engine adapter: queries the Algolia-powered search API.
//!
//! Free, keyless, and fast; returns story metadata (points, comments, date)
//! that enriches the tech-community signal for developer queries.

use async_trait::async_trait;
use chrono::DateTime;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const SNIPPET_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
struct HnHit {
    object_id: Option<String>,
    title: Option<String>,
    url: Option<String>,
    story_text: Option<String>,
    points: Option<i64>,
    num_comments: Option<i64>,
    created_at_i: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct HnBody {
    hits: Option<Vec<HnHit>>,
}

/// Hacker News (Algolia) adapter.
pub struct HnEngine {
    client: Client,
}

impl HnEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("query", query);
        serializer.append_pair("hitsPerPage", &opts.max_results.to_string());
        serializer.append_pair("tags", "story");
        format!(
            "https://hn.algolia.com/api/v1/search?{}",
            serializer.finish()
        )
    }
}

#[async_trait]
impl Engine for HnEngine {
    fn name(&self) -> &'static str {
        "hn-algolia"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(
                &url,
                self.client.next_user_agent(),
                opts,
                "application/json",
                &[],
            )
            .await?;
        let parsed: HnBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse HN hits into ranked hits.
///
/// Stories without an external URL link to their HN discussion page; the
/// snippet is the story text when present, otherwise a points/comments summary.
#[must_use]
fn parse_results(body: HnBody, max_results: usize) -> Vec<Hit> {
    let hits = body.hits.unwrap_or_default();
    let total = hits.len().min(max_results);
    let mut out = Vec::with_capacity(total);
    for (i, hit) in hits.into_iter().take(total).enumerate() {
        let Some(title) = hit.title else { continue };
        if title.is_empty() {
            continue;
        }
        let url = hit.url.filter(|u| !u.is_empty()).unwrap_or_else(|| {
            hit.object_id
                .as_deref()
                .map(|id| format!("https://news.ycombinator.com/item?id={id}"))
                .unwrap_or_default()
        });
        if url.is_empty() {
            continue;
        }
        let snippet = hit
            .story_text
            .filter(|s| !s.is_empty())
            .map(|s| truncate(&s, SNIPPET_LIMIT))
            .unwrap_or_else(|| {
                format!(
                    "{} points · {} comments",
                    hit.points.unwrap_or(0),
                    hit.num_comments.unwrap_or(0)
                )
            });
        let published_at = hit
            .created_at_i
            .and_then(|secs| DateTime::from_timestamp(secs, 0));
        out.push(Hit {
            url,
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "hn-algolia",
        });
    }
    out
}

fn truncate(s: &str, limit: usize) -> String {
    if s.len() <= limit {
        s.to_string()
    } else {
        let mut end = limit;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(url: Option<&str>, title: &str, points: Option<i64>) -> HnHit {
        HnHit {
            object_id: Some("123".to_string()),
            title: Some(title.to_string()),
            url: url.map(str::to_string),
            story_text: None,
            points,
            num_comments: Some(5),
            created_at_i: Some(1_700_000_000),
        }
    }

    #[test]
    fn parses_external_urls() {
        let body = HnBody {
            hits: Some(vec![hit(Some("https://example.com"), "Story", Some(10))]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com");
        assert_eq!(hits[0].snippet, "10 points · 5 comments");
        assert_eq!(hits[0].engine, "hn-algolia");
        assert!(hits[0].published_at.is_some());
    }

    #[test]
    fn falls_back_to_discussion_page() {
        let body = HnBody {
            hits: Some(vec![hit(None, "Ask HN: anything", None)]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits[0].url, "https://news.ycombinator.com/item?id=123");
    }

    #[test]
    fn uses_story_text_as_snippet() {
        let mut h = hit(Some("https://example.com"), "Story", Some(10));
        h.story_text = Some("A long story text that should be used".to_string());
        let body = HnBody {
            hits: Some(vec![h]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits[0].snippet, "A long story text that should be used");
    }

    #[test]
    fn truncates_long_story_text() {
        let mut h = hit(Some("https://example.com"), "Story", Some(10));
        h.story_text = Some("x".repeat(SNIPPET_LIMIT + 50));
        let body = HnBody {
            hits: Some(vec![h]),
        };
        let hits = parse_results(body, 10);
        // Truncation keeps SNIPPET_LIMIT bytes of the original plus the
        // 3-byte ellipsis character.
        assert_eq!(hits[0].snippet.len(), SNIPPET_LIMIT + 3);
        assert!(hits[0].snippet.ends_with('…'));
    }

    #[test]
    fn empty_body_yields_no_hits() {
        assert!(parse_results(HnBody { hits: None }, 10).is_empty());
    }
}
