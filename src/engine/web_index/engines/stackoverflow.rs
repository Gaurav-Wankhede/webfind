//! Stack Overflow engine adapter: queries the Stack Exchange API.
//!
//! Free and keyless (rate-limited per IP); returns question titles, links, and
//! body-derived snippets — a strong developer-Q&A signal.

use async_trait::async_trait;
use chrono::DateTime;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const SNIPPET_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
struct SoItem {
    title: Option<String>,
    link: Option<String>,
    body: Option<String>,
    creation_date: Option<i64>,
}

#[derive(Debug, Deserialize)]
struct SoBody {
    items: Option<Vec<SoItem>>,
}

/// Stack Overflow adapter.
pub struct StackOverflowEngine {
    client: Client,
}

impl StackOverflowEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("order", "desc");
        serializer.append_pair("sort", "relevance");
        serializer.append_pair("q", query);
        serializer.append_pair("site", "stackoverflow");
        serializer.append_pair("filter", "withbody");
        serializer.append_pair("pagesize", &opts.max_results.to_string());
        format!(
            "https://api.stackexchange.com/2.3/search/advanced?{}",
            serializer.finish()
        )
    }
}

#[async_trait]
impl Engine for StackOverflowEngine {
    fn name(&self) -> &'static str {
        "stackoverflow"
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
        let parsed: SoBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Strip HTML tags from a question body for a plain-text snippet.
fn strip_html(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse Stack Exchange items into ranked hits.
#[must_use]
fn parse_results(body: SoBody, max_results: usize) -> Vec<Hit> {
    let items = body.items.unwrap_or_default();
    let total = items.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, item) in items.into_iter().take(total).enumerate() {
        let Some(title) = item.title else { continue };
        let Some(url) = item.link else { continue };
        if title.is_empty() || url.is_empty() {
            continue;
        }
        let snippet = item
            .body
            .as_deref()
            .map(strip_html)
            .filter(|s| !s.is_empty())
            .map(|s| truncate(&s, SNIPPET_LIMIT))
            .unwrap_or_default();
        let published_at = item
            .creation_date
            .and_then(|secs| DateTime::from_timestamp(secs, 0));
        hits.push(Hit {
            url,
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "stackoverflow",
        });
    }
    hits
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

    #[test]
    fn parses_items_and_strips_html() {
        let body = SoBody {
            items: Some(vec![SoItem {
                title: Some("How to use tokio?".to_string()),
                link: Some("https://stackoverflow.com/q/1".to_string()),
                body: Some("<p>Use <code>tokio::main</code>.</p>".to_string()),
                creation_date: Some(1_700_000_000),
            }]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "How to use tokio?");
        assert_eq!(hits[0].snippet, "Use tokio::main.");
        assert_eq!(hits[0].engine, "stackoverflow");
        assert!(hits[0].published_at.is_some());
    }

    #[test]
    fn skips_items_missing_title_or_link() {
        let body = SoBody {
            items: Some(vec![
                SoItem {
                    title: None,
                    link: Some("https://x".to_string()),
                    body: None,
                    creation_date: None,
                },
                SoItem {
                    title: Some("Ok".to_string()),
                    link: Some("https://ok".to_string()),
                    body: None,
                    creation_date: None,
                },
            ]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://ok");
    }

    #[test]
    fn respects_max_results() {
        let body = SoBody {
            items: Some(vec![
                SoItem {
                    title: Some("A".to_string()),
                    link: Some("https://a".to_string()),
                    body: None,
                    creation_date: None,
                },
                SoItem {
                    title: Some("B".to_string()),
                    link: Some("https://b".to_string()),
                    body: None,
                    creation_date: None,
                },
            ]),
        };
        assert_eq!(parse_results(body, 1).len(), 1);
    }

    #[test]
    fn empty_body_yields_no_hits() {
        assert!(parse_results(SoBody { items: None }, 10).is_empty());
    }
}
