//! Wikipedia engine adapter: queries the MediaWiki opensearch API.
//!
//! Free, keyless, and returns authoritative encyclopedic results that dilute
//! brand-collision outcomes from the general engines (e.g. "next" → Next.js,
//! not the UK retailer).

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

/// Descriptive user agent; Wikipedia's API policy prefers real identifiers.
const USER_AGENT: &str = "webfind/0.1 (+https://github.com/Gaurav-Wankhede/webfind)";

/// OpenSearch response: `[query, titles[], snippets[], urls[]]`.
#[derive(Debug, Deserialize)]
struct OpenSearchBody(Vec<serde_json::Value>);

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
        serializer.append_pair("action", "opensearch");
        serializer.append_pair("format", "json");
        serializer.append_pair("search", query);
        serializer.append_pair("limit", &opts.max_results.clamp(1, 20).to_string());
        serializer.append_pair("namespace", "0");
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
        let parsed: OpenSearchBody = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

/// Parse the OpenSearch 4-array shape into ranked hits.
#[must_use]
fn parse_results(body: OpenSearchBody, max_results: usize) -> Vec<Hit> {
    if body.0.len() < 4 {
        return Vec::new();
    }
    let titles = body.0[1]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
    let snippets = body.0[2]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
    let urls = body.0[3]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>());
    let (Some(titles), Some(urls)) = (titles, urls) else {
        return Vec::new();
    };

    let total = titles.len().min(urls.len()).min(max_results);
    let mut hits = Vec::with_capacity(total);
    for i in 0..total {
        let title = titles[i];
        let url = urls[i];
        if title.is_empty() || url.is_empty() {
            continue;
        }
        hits.push(Hit {
            url: url.to_string(),
            title: title.to_string(),
            snippet: snippets
                .as_ref()
                .and_then(|s| s.get(i))
                .copied()
                .unwrap_or("")
                .to_string(),
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

    fn body() -> OpenSearchBody {
        OpenSearchBody(serde_json::from_str(
            r#"["rust",["Rust (programming language)","Rust"],["A language empowering everyone",""],["https://en.wikipedia.org/wiki/Rust_(programming_language)","https://en.wikipedia.org/wiki/Rust"]]"#,
        ).expect("fixture parses"))
    }

    #[test]
    fn parses_opensearch_shape() {
        let hits = parse_results(body(), 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].title, "Rust (programming language)");
        assert_eq!(
            hits[0].url,
            "https://en.wikipedia.org/wiki/Rust_(programming_language)"
        );
        assert_eq!(hits[0].snippet, "A language empowering everyone");
        assert_eq!(hits[0].engine, "wikipedia");
    }

    #[test]
    fn respects_max_results() {
        assert_eq!(parse_results(body(), 1).len(), 1);
    }

    #[test]
    fn short_body_yields_no_hits() {
        let short = OpenSearchBody(serde_json::from_str(r#"["rust"]"#).expect("fixture parses"));
        assert!(parse_results(short, 10).is_empty());
    }
}
