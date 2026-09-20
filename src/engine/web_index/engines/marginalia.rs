//! Marginalia engine adapter: queries the non-commercial long-tail web index.
//!
//! Marginalia focuses on the small web — pages the major engines deprioritize.
//! The public JSON API needs no key; the `API-Key: public` header is the
//! documented anonymous credential.

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

/// Descriptive user agent; Marginalia's API is bot-friendly but expects a
/// real identifier.
const USER_AGENT: &str = "webfind/0.3.2 (+https://github.com/Gaurav-Wankhede/webfind)";

#[derive(Debug, Deserialize)]
struct MarginaliaResult {
    url: Option<String>,
    title: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MarginaliaBody {
    results: Option<Vec<MarginaliaResult>>,
}

/// Marginalia adapter.
pub struct MarginaliaEngine {
    client: Client,
}

impl MarginaliaEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("query", query);
        serializer.append_pair("count", &opts.max_results.to_string());
        // Domain category 3 = general web.
        serializer.append_pair("dc", "3");
        format!(
            "https://api2.marginalia-search.com/search?{}",
            serializer.finish()
        )
    }
}

#[async_trait]
impl Engine for MarginaliaEngine {
    fn name(&self) -> &'static str {
        "marginalia"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(
                &url,
                USER_AGENT,
                opts,
                "application/json",
                &[("API-Key", "public")],
            )
            .await?;
        let parsed: MarginaliaBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse Marginalia results into ranked hits, preserving the API's ordering.
#[must_use]
fn parse_results(body: MarginaliaBody, max_results: usize) -> Vec<Hit> {
    let results = body.results.unwrap_or_default();
    let total = results.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, item) in results.into_iter().take(total).enumerate() {
        let Some(url) = item.url else { continue };
        let Some(title) = item.title else { continue };
        if url.is_empty() || title.is_empty() {
            continue;
        }
        hits.push(Hit {
            url,
            title,
            snippet: item.description.unwrap_or_default(),
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "marginalia",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_results_in_order() {
        let body = MarginaliaBody {
            results: Some(vec![
                MarginaliaResult {
                    url: Some("https://example.com/one".to_string()),
                    title: Some("First".to_string()),
                    description: Some("Desc one".to_string()),
                },
                MarginaliaResult {
                    url: Some("https://example.com/two".to_string()),
                    title: Some("Second".to_string()),
                    description: None,
                },
            ]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://example.com/one");
        assert_eq!(hits[0].snippet, "Desc one");
        assert_eq!(hits[0].engine, "marginalia");
        assert!(hits[0].relevance_score > hits[1].relevance_score);
    }

    #[test]
    fn skips_items_missing_url_or_title() {
        let body = MarginaliaBody {
            results: Some(vec![
                MarginaliaResult {
                    url: None,
                    title: Some("No url".to_string()),
                    description: None,
                },
                MarginaliaResult {
                    url: Some("https://example.com/ok".to_string()),
                    title: Some("Ok".to_string()),
                    description: None,
                },
            ]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://example.com/ok");
    }

    #[test]
    fn respects_max_results() {
        let body = MarginaliaBody {
            results: Some(vec![
                MarginaliaResult {
                    url: Some("https://example.com/one".to_string()),
                    title: Some("One".to_string()),
                    description: None,
                },
                MarginaliaResult {
                    url: Some("https://example.com/two".to_string()),
                    title: Some("Two".to_string()),
                    description: None,
                },
            ]),
        };
        assert_eq!(parse_results(body, 1).len(), 1);
    }

    #[test]
    fn empty_body_yields_no_hits() {
        let body = MarginaliaBody { results: None };
        assert!(parse_results(body, 10).is_empty());
    }
}
