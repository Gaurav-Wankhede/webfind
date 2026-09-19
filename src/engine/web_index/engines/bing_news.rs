//! Bing News engine adapter: scrapes Bing News vertical SERP results.
//!
//! Surfaces fresh, dated news stories and articles with publication timestamps.

use std::sync::LazyLock;

use async_trait::async_trait;
use scraper::{ElementRef, Html, Selector};

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::{
    decode_bing_tracker_url, parse_date_from_snippet, positional_relevance, strip_date_prefix,
};
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

static NEWS_CARD: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".news-card, .news-card-body, .nws_itm, li.b_algo")
        .expect("static selector is valid")
});

static NEWS_LINK: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse("a.title, a[data-id], h2 a, h3 a, a.news-card-title")
        .expect("static selector is valid")
});

static NEWS_SNIPPET: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".snippet, .news-card-body-text, .b_caption p, .news-snippet")
        .expect("static selector is valid")
});

static NEWS_DATE: LazyLock<Selector> = LazyLock::new(|| {
    Selector::parse(".news_dt, .source time, time, span[aria-label]").expect("static selector is valid")
});

/// Bing News adapter.
pub struct BingNewsEngine {
    client: Client,
}

impl BingNewsEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("filters", "tnews");
        serializer.append_pair("form", "YFNR");
        if let Some(country) = &opts.country {
            serializer.append_pair("cc", country);
        } else {
            serializer.append_pair("mkt", "en-US");
            serializer.append_pair("setlang", "en");
        }
        format!("https://www.bing.com/search?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for BingNewsEngine {
    fn name(&self) -> &'static str {
        "bing-news"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let user_agent = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
        let lang = opts.language.as_deref().unwrap_or("en-US,en;q=0.9");
        let headers = [("Accept-Language", lang)];

        let body = self
            .client
            .fetch(&url, user_agent, opts, "text/html", &headers)
            .await?;
        let document = Html::parse_document(&body);
        Ok(parse_results(&document, opts.max_results))
    }
}

fn parse_results(doc: &Html, max_results: usize) -> Vec<Hit> {
    let cards: Vec<ElementRef> = doc.select(&NEWS_CARD).collect();
    let total = cards.len().min(max_results);
    let mut hits = Vec::with_capacity(total);

    for (i, card) in cards.into_iter().take(total).enumerate() {
        let Some(link_el) = card.select(&NEWS_LINK).next() else { continue };
        let Some(raw_href) = link_el.value().attr("href") else { continue };
        let href = decode_bing_tracker_url(raw_href);
        let title = link_el.text().collect::<String>().trim().to_string();
        if href.is_empty() || title.is_empty() {
            continue;
        }

        let raw_snippet = card
            .select(&NEWS_SNIPPET)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        let date_text = card
            .select(&NEWS_DATE)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string());

        let published_at = date_text
            .as_deref()
            .and_then(parse_date_from_snippet)
            .or_else(|| parse_date_from_snippet(&raw_snippet));

        let snippet = strip_date_prefix(&raw_snippet).to_string();

        hits.push(Hit {
            url: href,
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "bing-news",
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bing_news_card() {
        let html = r#"
        <div class="news-card">
            <h2 class="title"><a class="news-card-title" href="https://example.com/news/1">Breaking Tech News</a></h2>
            <div class="snippet"><span class="news_dt">1 hour ago</span> Rust 2024 edition stabilizes new features.</div>
        </div>
        "#;
        let doc = Html::parse_document(html);
        let hits = parse_results(&doc, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Breaking Tech News");
        assert_eq!(hits[0].url, "https://example.com/news/1");
        assert_eq!(hits[0].engine, "bing-news");
        assert!(hits[0].published_at.is_some());
    }
}
