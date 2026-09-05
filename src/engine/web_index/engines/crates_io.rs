//! crates.io engine adapter: queries the Rust package registry's search API.
//!
//! Free and keyless; returns canonical crate metadata (name, description,
//! downloads, latest version) so ecosystem results surface directly for
//! queries that name or resemble a crate. crates.io's crawler policy requires
//! a descriptive user agent — a generic browser UA risks being blocked.

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

/// Descriptive user agent required by crates.io's crawler policy.
const USER_AGENT: &str = "webfind/0.1 (+https://github.com/Gaurav-Wankhede/webfind)";

#[derive(Debug, Deserialize)]
struct CrateHit {
    name: Option<String>,
    description: Option<String>,
    downloads: Option<u64>,
    max_version: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CratesBody {
    crates: Option<Vec<CrateHit>>,
}

/// crates.io adapter.
pub struct CratesIoEngine {
    client: Client,
}

impl CratesIoEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("per_page", &opts.max_results.to_string());
        format!("https://crates.io/api/v1/crates?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for CratesIoEngine {
    fn name(&self) -> &'static str {
        "crates-io"
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(&url, USER_AGENT, opts, "application/json", &[])
            .await?;
        let parsed: CratesBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse crate hits into ranked hits.
///
/// The snippet carries the description plus version and download counts when
/// available; the URL is the crate's canonical registry page.
#[must_use]
fn parse_results(body: CratesBody, max_results: usize) -> Vec<Hit> {
    let crates = body.crates.unwrap_or_default();
    let total = crates.len().min(max_results);
    let mut hits = Vec::with_capacity(total);
    for (i, item) in crates.into_iter().take(total).enumerate() {
        let Some(name) = item.name else { continue };
        if name.is_empty() {
            continue;
        }
        let description = item.description.unwrap_or_default();
        let snippet = match (item.max_version, item.downloads) {
            (Some(version), Some(downloads)) => {
                format!("{description} (v{version}, {downloads} downloads)")
            }
            (Some(version), None) => format!("{description} (v{version})"),
            _ => description,
        };
        hits.push(Hit {
            url: format!("https://crates.io/crates/{name}"),
            title: name,
            snippet,
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "crates-io",
        });
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_crates_with_metadata() {
        let body = CratesBody {
            crates: Some(vec![CrateHit {
                name: Some("tokio".to_string()),
                description: Some("An event-driven, non-blocking I/O platform".to_string()),
                downloads: Some(939_160_044),
                max_version: Some("1.53.1".to_string()),
            }]),
        };
        let hits = parse_results(body, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://crates.io/crates/tokio");
        assert_eq!(hits[0].title, "tokio");
        assert_eq!(
            hits[0].snippet,
            "An event-driven, non-blocking I/O platform (v1.53.1, 939160044 downloads)"
        );
        assert_eq!(hits[0].engine, "crates-io");
    }

    #[test]
    fn skips_crates_missing_name() {
        let body = CratesBody {
            crates: Some(vec![CrateHit {
                name: None,
                description: None,
                downloads: None,
                max_version: None,
            }]),
        };
        assert!(parse_results(body, 10).is_empty());
    }

    #[test]
    fn respects_max_results() {
        let body = CratesBody {
            crates: Some(vec![
                CrateHit {
                    name: Some("a".to_string()),
                    description: None,
                    downloads: None,
                    max_version: None,
                },
                CrateHit {
                    name: Some("b".to_string()),
                    description: None,
                    downloads: None,
                    max_version: None,
                },
            ]),
        };
        assert_eq!(parse_results(body, 1).len(), 1);
    }

    #[test]
    fn empty_body_yields_no_hits() {
        assert!(parse_results(CratesBody { crates: None }, 10).is_empty());
    }
}
