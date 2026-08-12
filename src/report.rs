use crate::schema::request::OutputFormat;
use crate::schema::response::SearchResponse;

/// Format a SearchResponse to the requested output format.
pub fn format_response(response: &SearchResponse, format: &OutputFormat) -> String {
    match format {
        OutputFormat::Json => to_json(response),
        OutputFormat::Report => to_report(response),
        OutputFormat::Markdown => to_markdown(response),
    }
}

/// Pretty-printed JSON.
fn to_json(response: &SearchResponse) -> String {
    serde_json::to_string_pretty(response).unwrap_or_else(|_| "{}".to_string())
}

/// Human-readable formatted text report.
fn to_report(response: &SearchResponse) -> String {
    let mut out = String::with_capacity(4096);

    out.push_str("╔══════════════════════════════════════════════════════════════╗\n");
    out.push_str("║                    WEBFIND SEARCH RESULTS                   ║\n");
    out.push_str("╚══════════════════════════════════════════════════════════════╝\n\n");

    out.push_str(&format!(
        "  Query:    \"{}\"\n  Depth:    {:?}\n  Results:  {} of {} total\n  Latency:  {}ms\n  Index:    {} documents | v{}\n\n",
        response.query,
        response.depth,
        response.returned,
        response.total_results,
        response.latency_ms,
        response.metadata.index_size,
        response.metadata.index_version,
    ));

    out.push_str("──────────────────────────────────────────────────────────────\n");

    for result in &response.results {
        out.push_str(&format!(
            "\n  #{}  {}  (score: {:.3})\n",
            result.rank, result.title, result.score
        ));
        out.push_str(&format!("      {}\n", result.url));
        out.push_str(&format!(
            "      Domain: {} | Lang: {} | Words: {}\n",
            result.domain,
            result.language,
            result.snippet.split_whitespace().count()
        ));
        if !result.snippet.is_empty() {
            let short: String = result.snippet.chars().take(160).collect();
            out.push_str(&format!("      \"{}…\"\n", short));
        }

        // Score breakdown
        out.push_str(&format!(
            "      Scores: BM25={:.3} | Vector={} | Graph={} | Fresh={} | Quality={} | Final={:.3}\n",
            result.scores.bm25,
            fmt_opt(result.scores.vector),
            fmt_opt(result.scores.graph),
            fmt_opt(result.scores.freshness),
            fmt_opt(result.scores.quality),
            result.scores.final_score,
        ));

        if let Some(ref content) = result.content {
            out.push_str(&format!(
                "      Content: {} words, {}s read time\n",
                content.word_count, content.reading_time_seconds
            ));
        }
    }

    if !response.suggestions.is_empty() {
        out.push_str("\n──────────────────────────────────────────────────────────────\n");
        out.push_str("  Suggestions:\n");
        for s in &response.suggestions {
            out.push_str(&format!("    • {}\n", s));
        }
    }

    if !response.related.is_empty() {
        out.push_str("  Related:\n");
        for r in &response.related {
            out.push_str(&format!("    • {}\n", r));
        }
    }

    out.push('\n');
    out
}

/// Markdown formatted output.
fn to_markdown(response: &SearchResponse) -> String {
    let mut out = String::with_capacity(4096);

    out.push_str(&format!("# WebFind Search: \"{}\"\n\n", response.query));
    out.push_str(&format!(
        "**Depth:** {:?} | **Results:** {} of {} | **Latency:** {}ms | **Index:** {} docs\n\n",
        response.depth,
        response.returned,
        response.total_results,
        response.latency_ms,
        response.metadata.index_size,
    ));

    out.push_str("---\n\n");

    for result in &response.results {
        out.push_str(&format!(
            "## #{} [{}]({})\n\n",
            result.rank, result.title, result.url
        ));
        out.push_str(&format!(
            "**Domain:** `{}` | **Score:** {:.3} | **Lang:** {}\n\n",
            result.domain, result.score, result.language,
        ));
        if !result.snippet.is_empty() {
            let short: String = result.snippet.chars().take(300).collect();
            out.push_str(&format!("> {}\n\n", short));
        }
        out.push_str(&format!(
            "| BM25 | Vector | Graph | Fresh | Quality | Final |\n\
             |------|--------|-------|-------|---------|-------|\n\
             | {:.3} | {} | {} | {} | {} | {:.3} |\n\n",
            result.scores.bm25,
            fmt_opt(result.scores.vector),
            fmt_opt(result.scores.graph),
            fmt_opt(result.scores.freshness),
            fmt_opt(result.scores.quality),
            result.scores.final_score,
        ));
    }

    if !response.suggestions.is_empty() {
        out.push_str("## Suggestions\n\n");
        for s in &response.suggestions {
            out.push_str(&format!("- {}\n", s));
        }
        out.push('\n');
    }

    if !response.related.is_empty() {
        out.push_str("## Related\n\n");
        for r in &response.related {
            out.push_str(&format!("- {}\n", r));
        }
        out.push('\n');
    }

    out
}

fn fmt_opt(v: Option<f64>) -> String {
    match v {
        Some(f) => format!("{:.3}", f),
        None => "—".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::request::SearchDepth;
    use crate::schema::response::*;
    use chrono::Utc;

    fn sample_response() -> SearchResponse {
        SearchResponse {
            request_id: "test-1".to_string(),
            query: "rust programming".to_string(),
            depth: SearchDepth::Shallow,
            total_results: 2,
            returned: 2,
            latency_ms: 3,
            results: vec![SearchResult {
                rank: 1,
                url: "https://rust-lang.org".to_string(),
                title: "Rust Programming Language".to_string(),
                snippet: "A systems language for reliability and performance.".to_string(),
                domain: "rust-lang.org".to_string(),
                published_at: None,
                modified_at: None,
                crawled_at: Utc::now(),
                author: None,
                site_name: None,
                score: 1.0,
                scores: ScoreBreakdown {
                    bm25: 1.0,
                    vector: None,
                    graph: None,
                    freshness: None,
                    quality: None,
                    final_score: 1.0,
                },
                content: None,
                keywords: None,
                metrics: None,
                favicon: None,
                thumbnail: None,
                language: "en".to_string(),
                content_type: "text".to_string(),
            }],
            suggestions: vec!["rust ownership".to_string()],
            related: vec!["go programming".to_string()],
            graph: None,
            metadata: SearchMetadata {
                index_version: "1".to_string(),
                index_size: 100,
                engine_version: "0.1.0".to_string(),
                searched_at: Utc::now(),
                signals_used: vec!["bm25".to_string()],
                index_freshness: IndexFreshness {
                    oldest_page: None,
                    newest_page: None,
                    avg_age_days: 0.0,
                },
            },
        }
    }

    #[test]
    fn test_json_output() {
        let resp = sample_response();
        let json = format_response(&resp, &OutputFormat::Json);
        assert!(json.contains("rust-lang.org"));
        assert!(json.contains("Rust Programming Language"));
    }

    #[test]
    fn test_report_output() {
        let resp = sample_response();
        let report = format_response(&resp, &OutputFormat::Report);
        assert!(report.contains("WEBFIND SEARCH RESULTS"));
        assert!(report.contains("rust-lang.org"));
        assert!(report.contains("Suggestions"));
    }

    #[test]
    fn test_markdown_output() {
        let resp = sample_response();
        let md = format_response(&resp, &OutputFormat::Markdown);
        assert!(md.contains("# WebFind Search"));
        assert!(md.contains("[Rust Programming Language](https://rust-lang.org)"));
    }
}
