use serde_json::Value;
use chrono::Utc;
use super::*;
use std::time::Instant;
use crate::schema::content::*;

impl Fetcher {
pub(crate) async fn try_manifest_fast_path(&self, parsed_url: &url::Url) -> Option<StructuredContent> {
        let origin = match (parsed_url.scheme(), parsed_url.host_str(), parsed_url.port()) {
            (scheme, Some(host), Some(port)) => format!("{scheme}://{host}:{port}"),
            (scheme, Some(host), None) => format!("{scheme}://{host}"),
            _ => return None,
        };

        // Ordered priority: /llms-full.txt (complete corpus), /llms.txt (curated), /llm.txt, /.well-known/ai-catalog.json, /ai-catelog.json
        let candidates = [
            format!("{origin}/llms-full.txt"),
            format!("{origin}/llms.txt"),
            format!("{origin}/llm.txt"),
            format!("{origin}/.well-known/ai-catalog.json"),
            format!("{origin}/.well-known/ai-catelog.json"),
            format!("{origin}/ai-catalog.json"),
            format!("{origin}/ai-catelog.json"),
        ];

        for candidate_url in candidates {
            let start = Instant::now();
            let resp = self
                .client
                .get(&candidate_url)
                .timeout(std::time::Duration::from_millis(1800))
                .send()
                .await;

            if let Ok(response) = resp
                && response.status().is_success()
                && let Ok(body) = response.text().await
            {
                let trimmed = body.trim();
                    if !trimmed.is_empty()
                        && !trimmed.to_ascii_lowercase().starts_with("<!doctype html")
                        && !trimmed.to_ascii_lowercase().starts_with("<html")
                    {
                        let elapsed = start.elapsed().as_millis() as u64;
                        let kind = if candidate_url.ends_with(".json") {
                            ContentKind::Json
                        } else {
                            ContentKind::Text
                        };
                        let mut sc = extract_from_text(
                            &body,
                            parsed_url.as_str(),
                            &candidate_url,
                            200,
                            candidate_url.starts_with("https://"),
                            elapsed,
                            None,
                            kind,
                        )
                        .ok()?;
                        sc.content_type = "text/markdown; manifest=llms.txt".to_string();
                        sc.content_type_header = "text/plain".to_string();
                        sc.is_valid_content = true;
                        return Some(sc);
                    }
            }
        }
        None
    }
}

// ── Content-Type classification ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContentKind {
    Html,
    Text,
    Json,
    Xml,
    Binary,
}

/// Classify a Content-Type header into a broad extraction strategy.
pub(crate) fn parse_content_type_header(ct: Option<&str>) -> (ContentKind, Option<String>) {
    let raw = ct.unwrap_or("text/html").to_lowercase();
    let mut parts = raw.split(';');
    let mime = parts.next().unwrap_or("text/html").trim();
    let charset = parts.find_map(|p| {
        let p = p.trim();
        p.strip_prefix("charset=")
            .or_else(|| p.strip_prefix("charset ="))
            .map(|s| s.trim_matches(['"', '\''].as_slice()).to_string())
    });

    let kind = if mime == "text/html" || mime == "application/xhtml+xml" {
        ContentKind::Html
    } else if mime.starts_with("text/") {
        ContentKind::Text
    } else if mime == "application/json" || mime == "application/ld+json" || mime.ends_with("+json")
    {
        ContentKind::Json
    } else if mime == "application/xml"
        || mime == "application/atom+xml"
        || mime == "application/rss+xml"
        || mime == "application/rdf+xml"
        || mime == "text/xml"
    {
        ContentKind::Xml
    } else if mime.starts_with("image/")
        || mime.starts_with("audio/")
        || mime.starts_with("video/")
        || mime == "application/pdf"
        || mime == "application/zip"
        || mime == "application/gzip"
        || mime == "application/x-gzip"
        || mime == "application/x-tar"
        || mime == "application/x-bzip2"
        || mime == "application/x-7z-compressed"
        || mime == "application/x-rar-compressed"
        || mime == "application/msword"
        || mime == "application/vnd.openxmlformats-officedocument"
        || mime == "application/octet-stream"
    {
        ContentKind::Binary
    } else {
        // Unknown types are treated as HTML; servers frequently mislabel.
        ContentKind::Html
    };

    (kind, charset)
}

/// Extract useful text from plain text, JSON, or XML/feed responses.
#[allow(clippy::too_many_arguments)]
pub(crate) fn extract_from_text(
    body: &str,
    original_url: &str,
    final_url: &str,
    status_code: u16,
    ssl_valid: bool,
    fetch_duration_ms: u64,
    last_modified: Option<&str>,
    kind: ContentKind,
) -> Result<StructuredContent> {
    let collapsed = normalize_text(body);
    let title = if kind == ContentKind::Json {
        // Try to pull a top-level "title" field without fully parsing.
        serde_json::from_str::<Value>(body)
            .ok()
            .and_then(|v| v.get("title").and_then(|t| t.as_str()).map(String::from))
            .or_else(|| {
                collapsed
                    .lines()
                    .find(|l| !l.trim().is_empty())
                    .map(|l| l.trim().chars().take(120).collect())
            })
    } else {
        collapsed
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().chars().take(120).collect())
    }
    .unwrap_or_else(|| title_from_url(final_url));

    let excerpt = collapsed.chars().take(300).collect::<String>();
    let word_count = count_words(&collapsed);
    let sentence_count = count_sentences(&collapsed);
    let reading_time_seconds = estimate_reading_time(word_count);
    let reading_ease = textstat::flesch_reading_ease(&collapsed);
    let grade_level = textstat::flesch_kincaid_grade(&collapsed);
    let keywords = extract_page_keywords(&collapsed, 15);
    let (language, language_confidence) = detect_language(&collapsed, body);

    let content_markdown = if kind == ContentKind::Json {
        format!("```json\n{}\n```", body.trim())
    } else if kind == ContentKind::Xml {
        format!("```xml\n{}\n```", body.trim())
    } else {
        collapsed.clone()
    };

    let modified_at = last_modified.and_then(parse_date);

    Ok(StructuredContent {
        url: original_url.to_string(),
        final_url: final_url.to_string(),
        status_code,
        title,
        description: None,
        canonical_url: None,
        language,
        language_confidence,
        published_at: None,
        modified_at,
        author: None,
        site_name: extract_site_name(final_url),
        content_text: collapsed.clone(),
        content_html: format!("<pre>{}</pre>", html_escape(&collapsed)),
        content_markdown,
        excerpt,
        word_count,
        char_count: collapsed.len() as u32,
        sentence_count,
        reading_time_seconds,
        reading_ease,
        grade_level,
        keywords,
        open_graph: None,
        twitter_card: None,
        json_ld: vec![],
        schema_type: if kind == ContentKind::Json {
            Some("json".to_string())
        } else if kind == ContentKind::Xml {
            Some("xml".to_string())
        } else {
            None
        },
        images: vec![],
        internal_links: vec![],
        external_links: vec![],
        favicon: None,
        rss_url: None,
        normalized_text: collapsed,
        fetched_at: Utc::now(),
        fetch_duration_ms,
        html_size_bytes: body.len() as u64,
        encoding: None,
        ssl_valid,
        redirect_count: 0,
        content_type: String::new(),
        content_type_header: String::new(),
        is_paywalled: false,
        is_valid_content: word_count >= MIN_VALID_WORDS,
        entities: extract_entities(body),
    })
}

/// Build a minimal invalid record for binary or otherwise un-indexable URLs.
pub(crate) fn make_invalid_content(
    original_url: &str,
    final_url: &str,
    status_code: u16,
    ssl_valid: bool,
    fetch_duration_ms: u64,
    ct_header: Option<String>,
) -> StructuredContent {
    StructuredContent {
        url: original_url.to_string(),
        final_url: final_url.to_string(),
        status_code,
        title: title_from_url(final_url),
        description: ct_header.clone(),
        canonical_url: None,
        language: "en".to_string(),
        language_confidence: 0.0,
        published_at: None,
        modified_at: None,
        author: None,
        site_name: extract_site_name(final_url),
        content_text: String::new(),
        content_html: String::new(),
        content_markdown: String::new(),
        excerpt: ct_header
            .clone()
            .unwrap_or_else(|| "Binary or unsupported content type".to_string()),
        word_count: 0,
        char_count: 0,
        sentence_count: 0,
        reading_time_seconds: 0,
        reading_ease: 0.0,
        grade_level: 0.0,
        keywords: vec![],
        open_graph: None,
        twitter_card: None,
        json_ld: vec![],
        schema_type: Some("binary".to_string()),
        images: vec![],
        internal_links: vec![],
        external_links: vec![],
        favicon: None,
        rss_url: None,
        normalized_text: String::new(),
        fetched_at: Utc::now(),
        fetch_duration_ms,
        html_size_bytes: 0,
        encoding: None,
        ssl_valid,
        redirect_count: 0,
        content_type: String::new(),
        content_type_header: ct_header.unwrap_or_default(),
        is_paywalled: false,
        is_valid_content: false,
        entities: Entities::default(),
    }
}

// ── Meta extraction ─────────────────────────────────────────────────────────


