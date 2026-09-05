//! DuckDuckGo Lite engine adapter: scrapes the HTML-only endpoint, which needs
//! no API key and returns clean, parseable result markup.

use std::sync::LazyLock;

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::{
    normalize_result_url, parse_date_from_snippet, positional_relevance, strip_date_prefix,
};
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

static RESULT_LINK: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.result-link").expect("static selector is valid"));
static RESULT_SNIPPET: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result-snippet").expect("static selector is valid"));

/// DuckDuckGo Lite adapter.
pub struct DuckDuckGoEngine {
    client: Client,
}

impl DuckDuckGoEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        if let (Some(country), Some(language)) = (&opts.country, &opts.language) {
            let lang = language.get(..2).unwrap_or(language);
            serializer.append_pair(
                "kl",
                &format!("{}-{}", country.to_lowercase(), lang.to_lowercase()),
            );
        }
        format!("https://lite.duckduckgo.com/lite/?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for DuckDuckGoEngine {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let html = match self
            .client
            .fetch_html(&url, self.client.next_user_agent(), opts)
            .await
        {
            Ok(html) => html,
            Err(Error::Blocked) => {
                // Retry once with a fresh user agent before reporting the block.
                self.client
                    .fetch_html(&url, self.client.next_user_agent(), opts)
                    .await?
            }
            Err(e) => return Err(e),
        };
        Ok(parse_results(&html, opts.max_results))
    }
}

/// Parse DDG Lite result HTML into ranked hits.
///
/// Links and snippets are collected as separate columns and paired by index,
/// matching the lite endpoint's table layout.
#[must_use]
pub fn parse_results(html: &str, max_results: usize) -> Vec<Hit> {
    let document = Html::parse_document(html);
    let links: Vec<ElementRef> = document.select(&RESULT_LINK).collect();
    let snippets: Vec<ElementRef> = document.select(&RESULT_SNIPPET).collect();
    let total = links.len().min(snippets.len()).min(max_results);

    let mut hits = Vec::with_capacity(total);
    for i in 0..total {
        let Some(href) = links[i].value().attr("href") else {
            continue;
        };
        let title = links[i].text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let snippet = snippets[i].text().collect::<String>().trim().to_string();
        let published_at = parse_date_from_snippet(&snippet);
        let snippet = strip_date_prefix(&snippet);
        hits.push(Hit {
            url: normalize_result_url(href),
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "duckduckgo",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <html><body>
        <a class="result-link" href="https://example.com/one">First Result</a>
        <div class="result-snippet">2025-01-15 · Snippet one</div>
        <a class="result-link" href="https://example.com/two">Second Result</a>
        <div class="result-snippet">Snippet two</div>
        </body></html>
    "#;

    #[test]
    fn parses_links_and_snippets() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/one");
        assert_eq!(hits[0].title, "First Result");
        assert_eq!(hits[0].snippet, "Snippet one");
        assert_eq!(hits[0].engine, "duckduckgo");
    }

    #[test]
    fn parses_date_from_snippet() {
        let hits = parse_results(FIXTURE, 10);
        assert!(hits[0].published_at.is_some());
        assert!(hits[1].published_at.is_none());
    }

    #[test]
    fn respects_max_results() {
        let hits = parse_results(FIXTURE, 1);
        assert_eq!(hits.len(), 1);
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
