//! Wikipedia engine adapter: queries the MediaWiki query API with generator=search
//! and extract/info properties for rich, informative snippets and canonical URLs.
//!
//! Free, keyless, and returns authoritative encyclopedic results that dilute
//! brand-collision outcomes from the general engines (e.g. "next" → Next.js,
//! not the UK retailer).

use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

/// Descriptive user agent; Wikipedia's API policy prefers real identifiers.
const USER_AGENT: &str = "webfind/0.1 (+https://github.com/Gaurav-Wankhede/webfind)";

#[derive(Debug, Deserialize)]
struct WikiPage {
    title: Option<String>,
    index: Option<u32>,
    extract: Option<String>,
    canonicalurl: Option<String>,
    fullurl: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WikiQuery {
    pages: Option<HashMap<String, WikiPage>>,
}

#[derive(Debug, Deserialize)]
struct WikiResponse {
    query: Option<WikiQuery>,
}

/// Wikipedia adapter.
pub struct WikipediaEngine {
    client: Client,
}

impl WikipediaEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let language = opts
            .language
            .as_deref()
            .unwrap_or("en")
            .get(..2)
            .unwrap_or("en")
            .to_lowercase();
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("action", "query");
        serializer.append_pair("generator", "search");
        serializer.append_pair("gsrsearch", query);
        serializer.append_pair("gsrlimit", &opts.max_results.clamp(1, 20).to_string());
        serializer.append_pair("prop", "extracts|info");
        serializer.append_pair("inprop", "url");
        serializer.append_pair("exintro", "1");
        serializer.append_pair("explaintext", "1");
        serializer.append_pair("exsentences", "2");
        serializer.append_pair("format", "json");
        format!(
            "https://{language}.wikipedia.org/w/api.php?{}",
            serializer.finish()
        )
    }
}

#[async_trait]
impl Engine for WikipediaEngine {
    fn name(&self) -> &'static str {
        "wikipedia"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(&url, USER_AGENT, opts, "application/json", &[])
            .await?;
        let parsed: WikiResponse = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse Wikipedia search generator response into ranked hits.
#[must_use]
fn parse_results(body: WikiResponse, max_results: usize) -> Vec<Hit> {
    let Some(query) = body.query else {
        return Vec::new();
    };
    let Some(pages_map) = query.pages else {
        return Vec::new();
    };

    let mut pages: Vec<WikiPage> = pages_map.into_values().collect();
    // Sort by Wikipedia's generator search index
    pages.sort_by_key(|p| p.index.unwrap_or(u32::MAX));

    let total = pages.len().min(max_results);
    let mut hits = Vec::with_capacity(total);

    for (i, page) in pages.into_iter().take(total).enumerate() {
        let Some(title) = page.title else { continue };
        let url = page
            .canonicalurl
            .or(page.fullurl)
            .unwrap_or_default();
        if title.is_empty() || url.is_empty() {
            continue;
        }

        let snippet = page
            .extract
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();

        hits.push(Hit {
            url,
            title,
            snippet,
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "wikipedia",
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body() -> WikiResponse {
        let raw = r#"{
            "query": {
                "pages": {
                    "100": {
                        "title": "Rust (programming language)",
                        "index": 1,
                        "extract": "Rust is a general-purpose programming language emphasizing performance and safety.",
                        "canonicalurl": "https://en.wikipedia.org/wiki/Rust_(programming_language)"
                    },
                    "200": {
                        "title": "Rust",
                        "index": 2,
                        "extract": "Rust is an iron oxide.",
                        "canonicalurl": "https://en.wikipedia.org/wiki/Rust"
                    }
                }
            }
        }"#;
        serde_json::from_str(raw).expect("fixture parses")
    }

    #[test]
    fn parses_wikipedia_generator_shape() {
        let hits = parse_results(body(), 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust (programming language)");
        assert_eq!(
            hits[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert!(hits[0].snippet.contains("general-purpose programming language"));
        assert_eq!(hits[0].engine, "wikipedia");
        assert_eq!(hits[1].title, "Rust");
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_results(body(), 1).len(), 1);
    }

    #[test]
    fn empty_body_yields_no_hits() {
        let empty = WikiResponse { query: None };
        assert!(parse_results(empty, 10).is_empty());
    }
}
