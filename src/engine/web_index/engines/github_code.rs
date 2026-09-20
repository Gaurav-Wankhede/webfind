//! GitHub Code Search engine adapter: queries the GitHub Search API.
//!
//! Searches open-source code repositories, paths, and implementations across GitHub.

use async_trait::async_trait;
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const USER_AGENT: &str = "webfind/0.3.2 (+https://github.com/Gaurav-Wankhede/webfind)";

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

/// GitHub Code Search adapter.
pub struct GithubCodeEngine {
    client: Client,
}

impl GithubCodeEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("q", query);
        serializer.append_pair("per_page", &opts.max_results.to_string());
        format!("https://api.github.com/search/code?{}", serializer.finish())
    }
}

#[async_trait]
impl Engine for GithubCodeEngine {
    fn name(&self) -> &'static str {
        "github-code"
    }

    fn should_query(&self, query: &str) -> bool {
        let q = query.to_ascii_lowercase();
        const CODE_HINTS: &[&str] = &[
            "github", "repo", "git", "code", "fn ", "def ", "class ", "import ",
            "const ", "let ", "func ", "pub ", "struct ", "impl ", "interface ",
            "library", "package", "npm", "pip", "cargo", "go get",
        ];
        CODE_HINTS.iter().any(|hint| q.contains(hint))
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let extra_headers = [
            ("Accept", "application/vnd.github+json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
        ];

        let body = match self
            .client
            .fetch(&url, USER_AGENT, opts, "application/json", &extra_headers)
            .await
        {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!("github-code query failed: {}", e);
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
        let Some(url) = item.html_url else { continue };
        let Some(path) = item.path else { continue };
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
            url,
            title,
            snippet,
            published_at: None,
            relevance_score: positional_relevance(i, total),
            engine: "github-code",
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_github_code_results() {
        let json = r#"{
            "items": [
                {
                    "path": "src/main.rs",
                    "html_url": "https://github.com/example/repo/blob/main/src/main.rs",
                    "repository": {
                        "full_name": "example/repo",
                        "description": "An example repository"
                    }
                }
            ]
        }"#;

        let parsed: GhResponse = serde_json::from_str(json).expect("valid json");
        let hits = parse_results(parsed, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "example/repo — src/main.rs");
        assert_eq!(hits[0].url, "https://github.com/example/repo/blob/main/src/main.rs");
        assert_eq!(hits[0].engine, "github-code");
    }
}
