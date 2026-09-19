//! Semantic Scholar engine adapter: queries the official Semantic Scholar Graph API.
//!
//! Free and keyless; returns academic research papers, authors, citations,
//! and open-access PDF links for scientific and technical research queries.

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36";
const SNIPPET_LIMIT: usize = 200;

#[derive(Debug, Deserialize)]
struct OpenAccessPdf {
    url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct S2Paper {
    #[serde(rename = "paperId")]
    paper_id: Option<String>,
    title: Option<String>,
    abstract_text: Option<String>,
    year: Option<i64>,
    url: Option<String>,
    #[serde(rename = "openAccessPdf")]
    open_access_pdf: Option<OpenAccessPdf>,
}

#[derive(Debug, Deserialize)]
struct S2Response {
    data: Option<Vec<S2Paper>>,
}

/// Semantic Scholar adapter.
pub struct SemanticScholarEngine {
    client: Client,
}

impl SemanticScholarEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    fn build_url(query: &str, opts: &EngineOptions) -> String {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        serializer.append_pair("query", query);
        serializer.append_pair("limit", &opts.max_results.to_string());
        serializer.append_pair(
            "fields",
            "title,abstract,year,url,authors,externalIds,openAccessPdf",
        );
        format!(
            "https://api.semanticscholar.org/graph/v1/paper/search?{}",
            serializer.finish()
        )
    }
}

#[async_trait]
impl Engine for SemanticScholarEngine {
    fn name(&self) -> &'static str {
        "semantic-scholar"
    }

    fn should_query(&self, query: &str) -> bool {
        let q = query.to_ascii_lowercase();
        const ACADEMIC_HINTS: &[&str] = &[
            "paper", "research", "study", "algorithm", "theorem", "analysis",
            "benchmark", "consensus", "physics", "quantum", "neural", "deep learning",
            "transformer", "model", "survey", "proof", "dataset", "biology", "medicine",
            "clinical", "citation", "conference", "journal", "ieee", "acm",
        ];
        ACADEMIC_HINTS.iter().any(|hint| q.contains(hint)) || q.split_whitespace().count() >= 5
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let url = Self::build_url(query, opts);
        let body = self
            .client
            .fetch(&url, USER_AGENT, opts, "application/json", &[])
            .await?;
        let parsed: S2Response = serde_json::from_str(&body)?;
        Ok(parse_results(parsed, opts.max_results))
    }
}

fn parse_results(response: S2Response, max_results: usize) -> Vec<Hit> {
    let papers = response.data.unwrap_or_default();
    let total = papers.len().min(max_results);
    let mut hits = Vec::with_capacity(total);

    for (i, paper) in papers.into_iter().take(total).enumerate() {
        let Some(title) = paper.title else { continue };
        if title.is_empty() {
            continue;
        }

        let pdf_url = paper.open_access_pdf.and_then(|p| p.url);
        let fallback_url = paper
            .paper_id
            .as_ref()
            .map(|id| format!("https://www.semanticscholar.org/paper/{id}"));
        let Some(url) = pdf_url.or(paper.url).or(fallback_url) else { continue };
        if url.is_empty() {
            continue;
        }

        let abstract_text = paper.abstract_text.unwrap_or_default();
        let snippet = if abstract_text.len() <= SNIPPET_LIMIT {
            abstract_text
        } else {
            let mut end = SNIPPET_LIMIT;
            while !abstract_text.is_char_boundary(end) && end > 0 {
                end -= 1;
            }
            format!("{}…", &abstract_text[..end])
        };

        let published_at = paper
            .year
            .and_then(|y| DateTime::parse_from_rfc3339(&format!("{y:04}-01-01T00:00:00Z")).ok())
            .map(|dt| dt.with_timezone(&Utc));

        hits.push(Hit {
            url,
            title,
            snippet,
            published_at,
            relevance_score: positional_relevance(i, total),
            engine: "semantic-scholar",
        });
    }

    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_semantic_scholar_response() {
        let json = r#"{
            "data": [
                {
                    "paperId": "12345",
                    "title": "Attention Is All You Need",
                    "abstract": "The dominant sequence transduction models are based on complex recurrent or convolutional neural networks.",
                    "year": 2017,
                    "url": "https://www.semanticscholar.org/paper/12345",
                    "openAccessPdf": {
                        "url": "https://arxiv.org/pdf/1706.03762.pdf"
                    }
                }
            ]
        }"#;

        let parsed: S2Response = serde_json::from_str(json).expect("valid json");
        let hits = parse_results(parsed, 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title, "Attention Is All You Need");
        assert_eq!(hits[0].url, "https://arxiv.org/pdf/1706.03762.pdf");
        assert_eq!(hits[0].engine, "semantic-scholar");
    }
}
