//! raw.githubusercontent engine: GitHub Code Search resolved to RAW source URLs.
//!
//! The `github-code` engine returns `github.com/.../blob/...` HTML pages. For
//! high-precision source retrieval the user wants the RAW file
//! (`raw.githubusercontent.com/...`), which is the authoritative, un-rendered
//! payload. This engine queries the same GitHub Search API but rewrites each
//! `html_url` blob URL into its `raw.githubusercontent.com` equivalent, so hits
//! land in `SourceTier::RawSource` (0.95 trust) instead of `Community`.
//!
//! blob URL:  https://github.com/{owner}/{repo}/blob/{ref}/{path}
//! raw URL:   https://raw.githubusercontent.com/{owner}/{repo}/{ref}/{path}

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const USER_AGENT: &str = crate::engine::util::USER_AGENT;

#[derive(Debug, Deserialize)]
struct GhRepo {
    full_name: Option<String>,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GhCodeItem {
    path: Option<String>,
    html_url: Option<String>,
    repository: Option<GhRepo>,
}

#[derive(Debug, Deserialize)]
struct GhResponse {
    items: Option<Vec<GhCodeItem>>,
}

/// GitHub code search, resolved to raw source URLs.
pub struct RawGithubEngine {
    client: Client,
}

impl RawGithubEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        let _ = serializer.append_pair("q", query);
        let _ = serializer.append_pair("per_page", &opts.max_results.to_string());
        format!("https://api.github.com/search/code?{}", serializer.finish())
    }
}

/// Rewrite a GitHub blob URL into its raw.githubusercontent.com equivalent.
/// Returns `None` if the URL is not a `/blob/` URL.
fn blob_to_raw(url: &str) -> Option<String> {
    let marker = "github.com/";
    let blob = "/blob/";
    let host_end = url.find(marker)? + marker.len();
    let repo_seg = &url[host_end..];
    let blob_at = repo_seg.find(blob)?;
    let owner_repo = &repo_seg[..blob_at];
    let after = &repo_seg[blob_at + blob.len()..];
    // after = "{ref}/{path}" — strip a trailing query/fragment if present.
    let after = after.split(['?', '#']).next().unwrap_or(after);
    Some(format!("https://raw.githubusercontent.com/{owner_repo}/{after}"))
}

#[async_trait]
impl Engine for RawGithubEngine {
    fn name(&self) -> &'static str {
        "raw-github"
    }

    fn should_query(&self, query: &str) -> bool {
        // Active for code/identifier intents: same signal as source_tier::route_intent_code.
        crate::engine::source_tier::route_intent_code(query)
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        // Optional GITHUB_TOKEN for authenticated rate limit (5000/hr vs 60/hr keyless).
        // raw.githubusercontent.com itself ignores auth, but the Search API we query
        // requires it for the higher quota. Source: GitHub API rate limit docs.
        let auth_header = std::env::var("GITHUB_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .map(|t| format!("Bearer {t}"));
        let mut extra_headers = vec![
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
        ];
        if let Some(ref auth) = auth_header {
            extra_headers.push(("Authorization", auth.as_str()));
        }

        let body = match self
            .client
            .fetch(&url, USER_AGENT, opts, "application/json", &extra_headers)
            .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("raw-github query failed: {}", e);
                return Ok(Vec::new());
            }
        };

        let parsed: GhResponse = match serde_json::from_str(&body) {
            Ok(p) => p,
            Err(_) => return Ok(Vec::new()),
        };

        Ok(parse_results(parsed, opts.max_results))
    }
}

fn parse_results(response: GhResponse, max_results: usize) -> Vec<Hit> {
    let items = response.items.unwrap_or_default();
    let total = items.len().min(max_results);
    let mut hits = Vec::with_capacity(total);

    for (i, item) in items.into_iter().take(total).enumerate() {
        let Some(html_url) = item.html_url else { continue };
        // Prefer the raw URL; skip if it is not a blob URL we can rewrite.
        let Some(raw_url) = blob_to_raw(&html_url) else { continue };
        let path = item.path.clone().unwrap_or_default();
        let repo_name = item
            .repository
            .as_ref()
            .and_then(|r| r.full_name.clone())
            .unwrap_or_else(|| "github".to_string());

        let title = format!("{repo_name} — {path}");
        let description = item.repository.and_then(|r| r.description);
        let snippet = match description {
            Some(desc) if !desc.is_empty() => format!("{desc} — {path}"),
            _ => path,
        };

        hits.push(Hit {
            url: raw_url,
            title,
            snippet,
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "raw-github",
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_blob_to_raw() {
        let blob = "https://github.com/serde-rs/serde/blob/main/src/lib.rs";
        let raw = blob_to_raw(blob).unwrap();
        assert_eq!(raw, "https://raw.githubusercontent.com/serde-rs/serde/main/src/lib.rs");
    }

    #[test]
    fn non_blob_returns_none() {
        assert!(blob_to_raw("https://github.com/serde-rs/serde").is_none());
    }

    #[test]
    fn parses_to_raw_urls() {
        let json = r#"{
            "items": [
                {
                    "path": "src/lib.rs",
                    "html_url": "https://github.com/serde-rs/serde/blob/main/src/lib.rs",
                    "repository": {"full_name": "serde-rs/serde", "description": "Serde"}
                }
            ]
        }"#;
        let parsed: GhResponse = serde_json::from_str(json).expect("valid json");
        let hits = parse_results(parsed, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].url, "https://raw.githubusercontent.com/serde-rs/serde/main/src/lib.rs");
        assert_eq!(hits[0].engine, "raw-github");
        // Must classify as RawSource (0.95 trust).
        assert_eq!(
            crate::engine::source_tier::classify(&hits[0].url),
            crate::engine::source_tier::SourceTier::RawSource
        );
    }
}
