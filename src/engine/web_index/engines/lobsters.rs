//! Lobsters engine adapter: scrapes the tech-news search page.
//!
//! Lobsters' `search.json` route was removed (the `.json` suffix adds an
//! unpermitted `format` param and the site raises on unpermitted params), so
//! the HTML search page is scraped instead. Free, no API key; a descriptive
//! user agent is required (UA-less requests get 400).

use std::sync::LazyLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use scraper::{ElementRef, Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

/// Descriptive user agent; Lobsters' Rack middleware rejects UA-less traffic.
const USER_AGENT: &str = "webfind/0.3.2 (+https://github.com/Gaurav-Wankhede/webfind)";

static STORY: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".story_liner").expect("static selector is valid"));
static STORY_LINK: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("span.link a.u-url, a.link").expect("static selector is valid")
});
static STORY_TIME: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("time[datetime]").expect("static selector is valid"));
static STORY_DESCRIPTION: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".description").expect("static selector is valid"));

/// Lobsters adapter.
pub struct LobstersEngine {
    client: Client,
}

impl LobstersEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("what", "stories");
        format!("https://lobste.rs/search?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for LobstersEngine {
    fn name(&self) -> &'static str {
        "lobsters"
    }

    fn should_query(&self, query: &str) -> bool {
        let q = query.to_ascii_lowercase();
        const TECH_HINTS: &[&str] = &[
            "rust", "linux", "kernel", "c++", "compiler", "database", "distributed",
            "security", "exploit", "crypto", "git", "unix", "programming", "software",
            "open source", "networking", "protocol", "algorithm", "hacker", "code",
        ];
        TECH_HINTS.iter().any(|hint| q.contains(hint))
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query);
        let html = self.client.fetch_html(&url, USER_AGENT, opts).await?;
        Ok(parse_results(&html, opts.max_results))
    }
}

/// Parse Lobsters search HTML into ranked hits.
#[must_use]
pub fn parse_results(html: &str, max_results: usize) -> Vec<Hit> {
    let document = Html::parse_document(html);
    let stories: Vec<ElementRef> = document.select(&STORY).collect();
    let total = stories.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, story) in stories.into_iter().take(total).enumerate() {
        let Some(link) = story.select(&STORY_LINK).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();
        if href.is_empty() || title.is_empty() {
            continue;
        }
        let snippet = story
            .select(&STORY_DESCRIPTION)
            .next()
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let published_at = story
            .select(&STORY_TIME)
            .next()
            .and_then(|t| t.value().attr("datetime"))
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        hits.push(Hit {
            url: href.to_string(),
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "lobsters",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <html><body>
        <div class="story_liner h-entry">
          <div class="details">
            <span class="link h-cite u-repost-of">
              <a class="u-url" href="https://example.com/one">First Story</a>
            </span>
            <time datetime="2026-09-04T19:29:30Z">2026-09-04</time>
            <div class="description">A description of the first story.</div>
          </div>
        </div>
        <div class="story_liner h-entry">
          <div class="details">
            <span class="link h-cite u-repost-of">
              <a class="u-url" href="https://example.com/two">Second Story</a>
            </span>
            <time datetime="2026-09-03T10:00:00Z">2026-09-03</time>
          </div>
        </div>
        </body></html>
    "#;

    #[test]
    fn parses_stories() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/one");
        assert_eq!(hits[0].title, "First Story");
        assert_eq!(hits[0].snippet, "A description of the first story.");
        assert_eq!(hits[0].engine, "lobsters");
        assert!(hits[0].published_at.is_some());
        assert!(hits[1].published_at.is_some());
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_results(FIXTURE, 1).len(), 1);
    }

    #[test]
    fn empty_html_yields_no_hits() {
        assert!(parse_results("<html></html>", 10).is_empty());
    }

    #[test]
    fn relevance_decays_with_rank() {
        let hits = parse_results(FIXTURE, 10);
        assert!(hits[0].relevance_score > hits[1].relevance_score);
    }
}
