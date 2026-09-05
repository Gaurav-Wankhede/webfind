//! arXiv engine adapter: queries the arXiv Atom API.
//!
//! Free and keyless; returns academic paper metadata (title, abstract,
//! publication date) for research queries. The Atom feed is parsed with the
//! HTML scraper, which tolerates the feed's XML shape well enough for the
//! stable `entry`/`title`/`summary`/`published` elements.

use std::sync::LazyLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const SNIPPET_LIMIT: usize = 200;

static ENTRY: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry").expect("static selector is valid"));
static ENTRY_ID: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry > id").expect("static selector is valid"));
static ENTRY_TITLE: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry > title").expect("static selector is valid"));
static ENTRY_SUMMARY: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry > summary").expect("static selector is valid"));
static ENTRY_PUBLISHED: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("entry > published").expect("static selector is valid"));

/// arXiv adapter.
pub struct ArxivEngine {
    client: Client,
}

impl ArxivEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("search_query", &format!("all:{query}"));
        serializer.append_pair("max_results", &opts.max_results.to_string());
        serializer.append_pair("sortBy", "relevance");
        format!("https://export.arxiv.org/api/query?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for ArxivEngine {
    fn name(&self) -> &'static str {
        "arxiv"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(
                &url,
                self.client.next_user_agent(),
                opts,
                "application/atom+xml",
                &[],
            )
            .await?;
        Ok(parse_results(&body, opts.max_results))
    }
}

/// Parse the arXiv Atom feed into ranked hits.
#[must_use]
pub fn parse_results(xml: &str, max_results: usize) -> Vec<Hit> {
    let document = Html::parse_document(xml);
    let entries: Vec<_> = document.select(&ENTRY).collect();
    let total = entries.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, entry) in entries.into_iter().take(total).enumerate() {
        let Some(url) = entry
            .select(&ENTRY_ID)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
        else {
            continue;
        };
        let title = entry
            .select(&ENTRY_TITLE)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        if url.is_empty() || title.is_empty() {
            continue;
        }
        let snippet = entry
            .select(&ENTRY_SUMMARY)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let published_at = entry
            .select(&ENTRY_PUBLISHED)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        hits.push(Hit {
            url,
            title,
            snippet: truncate(&snippet, SNIPPET_LIMIT),
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "arxiv",
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

    const FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>arXiv Query</title>
  <entry>
    <id>http://arxiv.org/abs/2401.00001v1</id>
    <title>Async Rust in Production</title>
    <summary>We study async runtimes in production systems.</summary>
    <published>2024-01-01T00:00:00Z</published>
  </entry>
  <entry>
    <id>http://arxiv.org/abs/2401.00002v1</id>
    <title>Second Paper</title>
    <summary>Another abstract.</summary>
    <published>2024-02-01T00:00:00Z</published>
  </entry>
</feed>"#;

    #[test]
    fn parses_atom_entries() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "http://arxiv.org/abs/2401.00001v1");
        assert_eq!(hits[0].title, "Async Rust in Production");
        assert_eq!(
            hits[0].snippet,
            "We study async runtimes in production systems."
        );
        assert_eq!(hits[0].engine, "arxiv");
        assert!(hits[0].published_at.is_some());
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_results(FIXTURE, 1).len(), 1);
    }

    #[test]
    fn empty_feed_yields_no_hits() {
        assert!(parse_results("<feed></feed>", 10).is_empty());
    }

    #[test]
    fn relevance_decays_with_rank() {
        let hits = parse_results(FIXTURE, 10);
        assert!(hits[0].relevance_score > hits[1].relevance_score);
    }
}
