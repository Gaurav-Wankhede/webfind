//! MDN engine adapter: queries the MDN Web Docs search API.
//!
//! Free and keyless; returns authoritative web-platform documentation for
//! developer queries (APIs, CSS, HTML, JS).

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const MDN_HOST: &str = "https://developer.mozilla.org";

#[derive(Debug, Deserialize)]
struct MdnDoc {
    mdn_url: Option<String>,
    title: Option<String>,
    summary: Option<String>,
}

#[derive(Debug, Deserialize)]
struct MdnBody {
    documents: Option<Vec<MdnDoc>>,
}

/// MDN adapter.
pub struct MdnEngine {
    client: Client,
}

impl MdnEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("locale", "en-US");
        serializer.append_pair("size", &opts.max_results.to_string());
        format!("{MDN_HOST}/api/v1/search?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for MdnEngine {
    fn name(&self) -> &'static str {
        "mdn"
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
        let parsed: MdnBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse MDN documents into ranked hits.
///
/// `mdn_url` may be relative (`/en-US/docs/...`) or absolute; relative paths
/// are resolved against the MDN host.
#[must_use]
fn parse_results(body: MdnBody, max_results: usize) -> Vec<Hit> {
    let docs = body.documents.unwrap_or_default();
    let total = docs.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, doc) in docs.into_iter().take(total).enumerate() {
        let Some(title) = doc.title else { continue };
        let Some(path) = doc.mdn_url else { continue };
        if title.is_empty() || path.is_empty() {
            continue;
        }
        let url = if path.starts_with("http") {
            path
        } else {
            format!("{MDN_HOST}{path}")
        };
        hits.push(Hit {
            url,
            title,
            snippet: doc.summary.unwrap_or_default(),
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "mdn",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documents_and_resolves_relative_urls() {
        let body = MdnBody {
            documents: Some(vec![MdnDoc {
                mdn_url: Some("/en-US/docs/Web/API/Fetch_API".to_string()),
                title: Some("Fetch API".to_string()),
                summary: Some("The Fetch API provides an interface".to_string()),
            }]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].url,
            "https://developer.mozilla.org/en-US/docs/Web/API/Fetch_API"
        );
        assert_eq!(hits[0].title, "Fetch API");
        assert_eq!(hits[0].engine, "mdn");
    }

    #[test]
    fn keeps_absolute_urls() {
        let body = MdnBody {
            documents: Some(vec![MdnDoc {
                mdn_url: Some("https://example.com/doc".to_string()),
                title: Some("Doc".to_string()),
                summary: None,
            }]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits[0].url, "https://example.com/doc");
    }

    #[test]
    fn skips_documents_missing_title_or_url() {
        let body = MdnBody {
            documents: Some(vec![
                MdnDoc {
                    mdn_url: None,
                    title: Some("No url".to_string()),
                    summary: None,
                },
                MdnDoc {
                    mdn_url: Some("/ok".to_string()),
                    title: Some("Ok".to_string()),
                    summary: None,
                },
            ]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Ok");
    }

    #[test]
    fn empty_body_yields_no_hits() {
        assert!(parse_results(MdnBody { documents: None }, 10).is_empty());
    }
}
