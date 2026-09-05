//! Bing engine adapter: scrapes the standard SERP HTML and resolves `/ck/a`
//! tracker links to their destinations.

use std::sync::LazyLock;

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::{
    decode_bing_tracker_url, parse_date_from_snippet, positional_relevance, strip_date_prefix,
};
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

static RESULT_ITEM: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("li.b_algo").expect("static selector is valid"));
static RESULT_LINK: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h2 a").expect("static selector is valid"));
static RESULT_SNIPPET: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".b_lineclamp2, .b_lineclamp3, .b_caption p").expect("static selector is valid")
});
static RESULT_DATE: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".news_dt, span[aria-label]").expect("static selector is valid")
});

/// Bing adapter.
pub struct BingEngine {
    client: Client,
}

impl BingEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        if let Some(country) = &opts.country {
            serializer.append_pair("cc", country);
        } else {
            // Pin the market to English so a non-US IP does not get
            // locale-mixed results; an explicit country opts back in.
            serializer.append_pair("mkt", "en-US");
            serializer.append_pair("setlang", "en");
        }
        format!("https://www.bing.com/search?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for BingEngine {
    fn name(&self) -> &'static str {
        "bing"
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

/// Parse Bing SERP HTML into ranked hits.
///
/// Result links are `/ck/a` tracker URLs; each is decoded to its destination.
/// Publication dates come from the news-date element when present, falling
/// back to a date prefix in the snippet.
#[must_use]
pub fn parse_results(html: &str, max_results: usize) -> Vec<Hit> {
    let document = Html::parse_document(html);
    let items: Vec<ElementRef> = document.select(&RESULT_ITEM).collect();
    let total = items.len().min(max_results);

    let mut hits = Vec::with_capacity(total);
    for (i, item) in items.into_iter().take(total).enumerate() {
        let Some(link) = item.select(&RESULT_LINK).next() else {
            continue;
        };
        let Some(href) = link.value().attr("href") else {
            continue;
        };
        let title = link.text().collect::<String>().trim().to_string();
        if title.is_empty() {
            continue;
        }
        let snippet = item
            .select(&RESULT_SNIPPET)
            .next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let published_at = item
            .select(&RESULT_DATE)
            .next()
            .and_then(|el| parse_date_from_snippet(&el.text().collect::<String>()))
            .or_else(|| parse_date_from_snippet(&snippet));
        let snippet = strip_date_prefix(&snippet);

        hits.push(Hit {
            url: decode_bing_tracker_url(href),
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "bing",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"
        <html><body>
        <li class="b_algo">
            <h2><a href="https://www.bing.com/ck/a?u=a1aHR0cHM6Ly9leGFtcGxlLmNvbS9wYWdlP3E9MQ&p=1">Bing Result One</a></h2>
            <p class="b_lineclamp2">2025-01-15 · Snippet one</p>
        </li>
        <li class="b_algo">
            <h2><a href="https://example.org/two">Bing Result Two</a></h2>
            <p class="b_caption p">Snippet two</p>
        </li>
        </body></html>
    "#;

    #[test]
    fn parses_items_and_decodes_tracker_urls() {
        let hits = parse_results(FIXTURE, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/page?q=1");
        assert_eq!(hits[0].title, "Bing Result One");
        assert_eq!(hits[0].engine, "bing");
        assert_eq!(hits[1].url, "https://example.org/two");
    }

    #[test]
    fn parses_dates_from_snippets() {
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
