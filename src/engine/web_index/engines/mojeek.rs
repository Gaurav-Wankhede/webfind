//! Mojeek engine adapter: scrapes the independent web index.
//!
//! Mojeek runs its own crawl — no Bing/Google reliance — adding an independent
//! lexical signal that dilutes brand-collision outcomes from the major
//! engines. Free HTML search; no API key. The request shape mirrors SearXNG's
//! proven adapter: no `fmt` param, `safe=0`, and no `s` offset on page 1
//! (sending `s=0` triggers rate-limiting).

use std::sync::LazyLock;

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

static RESULT_ITEM: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("ul.results-standard li, .results .result").expect("static selector is valid")
});
static TITLE_ANCHOR: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.title, h2 a.title, h2 a").expect("static selector is valid")
});
static SNIPPET: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("p.s, .description").expect("static selector is valid"));

/// Mojeek adapter.
pub struct MojeekEngine {
    client: Client,
}

impl MojeekEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("safe", "0");
        format!("https://www.mojeek.com/search?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for MojeekEngine {
    fn name(&self) -> &'static str {
        "mojeek"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query);
        let html = self
            .client
            .fetch_html(&url, self.client.next_user_agent(), opts)
            .await?;
        Ok(parse_results(&html, opts.max_results))
    }
}

/// Parse Mojeek result HTML into ranked hits.
#[must_use]
pub fn parse_results(html: &str, max_results: usize) -> Vec<Hit> {
    let document = Html::parse_document(html);
    let items: Vec<ElementRef> = document.select(&RESULT_ITEM).collect();
    let total = items.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, item) in items.into_iter().take(total).enumerate() {
        // Mojeek's anchor selector varies slightly across page variants; try
        // the documented one first and fall back to the first <a>.
        let anchor = item.select(&TITLE_ANCHOR).next().or_else(|| {
            item.select(&Selector::parse("a").expect("static selector is valid"))
                .next()
        });
        let Some(anchor) = anchor else { continue };
        let Some(href) = anchor.value().attr("href") else {
            continue;
        };
        let title = anchor.text().collect::<String>().trim().to_string();
        if href.is_empty() || title.is_empty() {
            continue;
        }
        let snippet = item
            .select(&SNIPPET)
            .next()
            .map(|s| s.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        hits.push(Hit {
            url: href.to_string(),
            title,
            snippet,
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "mojeek",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <html><body>
        <ul class="results-standard">
          <li>
            <h2><a class="title" href="https://example.com/one">First Result</a></h2>
            <p class="s">Snippet one</p>
          </li>
          <li>
            <h2><a class="title" href="https://example.com/two">Second Result</a></h2>
            <p class="s">Snippet two</p>
          </li>
        </ul>
        </body></html>
    "#;

    #[test]
    fn parses_results() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/one");
        assert_eq!(hits[0].title, "First Result");
        assert_eq!(hits[0].snippet, "Snippet one");
        assert_eq!(hits[0].engine, "mojeek");
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
