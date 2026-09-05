use std::collections::HashSet;
use std::num::NonZeroU32;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use readability_rust::Readability;
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use unicode_segmentation::UnicodeSegmentation;

use crate::engine::device_profile::{DeviceProfile, SessionManager};
use crate::engine::fingerprint::{FingerprintAuditLog, FingerprintGenerator};
use crate::engine::human_client::HumanClient;
use crate::engine::proxy_pool::ProxyPool;
use crate::schema::content::{Entities, ImageInfo, OpenGraph, StructuredContent, TwitterCard};
use crate::schema::response::Keyword;

const USER_AGENT: &str = "webfind/0.1 (+https://github.com/Gaurav-Wankhede/webfind)";
const FETCH_TIMEOUT_SECS: u64 = 30;

/// Minimum word count for a page to be considered valid content.
///
/// Kept deliberately low: many legitimate pages (docs landing pages, error /
/// redirect stubs, single-purpose pages like example.com) carry only a few
/// dozen words yet are exactly what an agent wants scraped. A high floor
/// (previously 50) silently dropped these, so `webfind_fetch` returned
/// `is_valid_content: false` and research treated them as failed crawls —
/// which is why the LLM fell back to DuckDuckGo. 5 words still filters
/// empty / JS-shell pages while retaining real content.
pub(crate) const MIN_VALID_WORDS: u32 = 5;

/// Whether to rotate the User-Agent header per request.
#[derive(Clone, Copy)]
pub enum RotateUserAgent {
    Rotate,
    Fixed,
}

/// Fetcher: HTTP client + content extraction pipeline.
pub struct Fetcher {
    client: Client,
    human_client: Option<HumanClient>,
    profile: Option<DeviceProfile>,
    audit_log: Option<Arc<dyn FingerprintAuditLog>>,
    dynamic_fallback: bool,
    dynamic_wait_ms: u64,
    dynamic_deep: bool,
}

impl Fetcher {
    pub fn new() -> Result<Self> {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .build()
            .context("failed to build HTTP client")?;
        Ok(Self {
            client,
            human_client: None,
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }

    /// Build a Fetcher from an existing reqwest client.
    pub fn from_client(client: Client) -> Result<Self> {
        let mut fetcher = Self::new()?;
        fetcher.client = client;
        Ok(fetcher)
    }

    /// Attach a device profile so every request carries consistent headers.
    pub fn with_profile(mut self, profile: DeviceProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    /// Build a fetcher with human-like request handling and privacy fingerprint rotation.
    pub fn new_human(
        proxy_pool: Option<ProxyPool>,
        session_manager: Option<SessionManager>,
        rotate_ua: RotateUserAgent,
        rps: u32,
    ) -> Result<Self> {
        let rps = NonZeroU32::new(rps.max(1)).expect("rps.max(1) is always >= 1");
        let ua_rotate = match rotate_ua {
            RotateUserAgent::Rotate => true,
            RotateUserAgent::Fixed => false,
        };
        let mut builder = HumanClient::builder()
            .requests_per_second(rps)
            .burst_size(rps.get() * 3)
            .rotate_ua(ua_rotate);
        if let Some(pool) = proxy_pool {
            builder = builder.proxy_pool(pool);
        }
        if let Some(mgr) = session_manager {
            builder = builder.session_manager(mgr);
        }
        let fingerprint_generator = Some(FingerprintGenerator::new());
        if let Some(generator) = fingerprint_generator {
            builder = builder.fingerprint_generator(generator);
        }
        let human_client = builder.build()?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .build()
            .context("failed to build fallback HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(human_client),
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }

    /// Attach an audit sink for fingerprint usage events.
    pub fn with_audit_log(mut self, audit_log: Arc<dyn FingerprintAuditLog>) -> Self {
        self.audit_log = Some(audit_log.clone());
        if let Some(ref mut hc) = self.human_client {
            hc.set_audit_log(audit_log);
        }
        self
    }

    /// Enable Chromium-based dynamic rendering fallback for JS-heavy pages.
    pub fn with_dynamic_fallback(mut self, wait_ms: u64) -> Self {
        self.dynamic_fallback = true;
        self.dynamic_wait_ms = wait_ms.max(500);
        self
    }

    /// Enable deep browser rendering: stealth mode + infinite-scroll so
    /// progressively rendered / bot-protected pages are fully captured.
    pub fn with_dynamic_deep(mut self, deep: bool) -> Self {
        self.dynamic_deep = deep;
        self
    }

    /// Fetch a URL and extract StructuredContent.
    pub async fn fetch_url(&self, url: &str) -> Result<StructuredContent> {
        // Reddit-specific path: try .json endpoint, fall back to old Reddit HTML.
        if super::reddit::is_reddit_url(url) {
            return self.fetch_reddit(url).await;
        }

        let start = Instant::now();
        let response = if let Some(ref hc) = self.human_client {
            hc.get(url).await?
        } else {
            let mut req = self.client
                .get(url)
                .header("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,application/atom+xml,application/rss+json;q=0.8,*/*;q=0.7");
            if let Some(ref profile) = self.profile {
                req = profile.apply_headers(req);
            }
            req.send()
                .await
                .with_context(|| format!("HTTP request failed for {}", url))?
        };

        let status = response.status().as_u16();
        let final_url = response.url().to_string();
        let ssl_valid = final_url.starts_with("https://");
        let last_modified = response
            .headers()
            .get("last-modified")
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .map(String::from);

        let content_type_header = content_type.clone().unwrap_or_default();
        let (kind, _charset) = parse_content_type_header(content_type.as_deref());

        let elapsed = start.elapsed().as_millis() as u64;

        // Binary resources are not indexed; return a minimal invalid record so
        // callers can log the URL but not waste tokens on garbage text.
        if kind == ContentKind::Binary {
            return Ok(make_invalid_content(
                url,
                &final_url,
                status,
                ssl_valid,
                elapsed,
                content_type,
            ));
        }

        // Zero-copy body read: `bytes()` borrows reqwest's internal buffer, and
        // `from_utf8_lossy` borrows it again as `&str` when the body is valid
        // UTF-8 (the common case), avoiding a full-body `String` allocation.
        // Non-UTF-8 bodies are decoded lossily instead of erroring.
        let body_bytes = response
            .bytes()
            .await
            .context("failed to read response body")?;
        let body = String::from_utf8_lossy(&body_bytes);

        let mut content = match kind {
            ContentKind::Html => self.extract_from_html(
                &body,
                url,
                &final_url,
                status,
                ssl_valid,
                elapsed,
                last_modified.as_deref(),
            )?,
            ContentKind::Text | ContentKind::Json | ContentKind::Xml => extract_from_text(
                &body,
                url,
                &final_url,
                status,
                ssl_valid,
                elapsed,
                last_modified.as_deref(),
                kind,
            )?,
            ContentKind::Binary => unreachable!(),
        };

        content.content_type_header = content_type_header.clone();
        content.content_type =
            super::content_classifier::classify_content_type(&content).to_string();

        // If static fetch yields no meaningful content and dynamic fallback is enabled,
        // render the page in Chromium and re-extract.
        if !content.is_valid_content && self.dynamic_fallback && kind == ContentKind::Html {
            #[cfg(feature = "dynamic")]
            {
                let dynamic = super::dynamic_fetcher::DynamicFetcher::new();
                let (rendered_html, dynamic_final_url) = dynamic
                    .render(url, self.dynamic_wait_ms, self.dynamic_deep)
                    .await?;
                let ssl = dynamic_final_url.starts_with("https://");
                let mut dyn_content = self.extract_from_html(
                    &rendered_html,
                    url,
                    &dynamic_final_url,
                    200,
                    ssl,
                    elapsed + self.dynamic_wait_ms,
                    None,
                )?;
                dyn_content.content_type_header = content_type_header.clone();
                dyn_content.content_type =
                    super::content_classifier::classify_content_type(&dyn_content).to_string();
                return Ok(dyn_content);
            }
            #[cfg(not(feature = "dynamic"))]
            {
                anyhow::bail!("dynamic fallback requested but 'dynamic' feature is not enabled");
            }
        }

        Ok(content)
    }

    /// Fetch a Reddit URL, preferring the `.json` endpoint and falling back
    /// to old.reddit.com HTML when JSON returns 403.
    async fn fetch_reddit(&self, url: &str) -> Result<StructuredContent> {
        let start = Instant::now();

        // Attempt 1: try the .json endpoint
        if let Some(json_url) = super::reddit::to_reddit_json_url(url) {
            let json_request = self.build_reddit_request(&json_url);
            match json_request.send().await {
                Ok(resp) if resp.status().is_success() => {
                    let status = resp.status().as_u16();
                    let final_url = resp.url().to_string();
                    let body = resp
                        .text()
                        .await
                        .context("failed to read Reddit JSON body")?;
                    let elapsed = start.elapsed().as_millis() as u64;

                    match super::reddit::parse_reddit_listing(&body) {
                        Ok(items) if !items.is_empty() => {
                            let text = super::reddit::listing_to_text(&items);
                            return self
                                .build_reddit_content(url, &final_url, status, elapsed, &text);
                        }
                        _ => {
                            // JSON parsed but no items — fall through to HTML
                        }
                    }
                }
                Ok(_) | Err(_) => {
                    // 403 or network error — fall through to HTML
                }
            }
        }

        // Attempt 2: fetch old.reddit.com HTML (cleaner than new Reddit)
        let old_url = super::reddit::to_old_reddit_url(url);
        let html_request = self.build_reddit_request(&old_url);
        let resp = html_request
            .send()
            .await
            .with_context(|| format!("HTTP request failed for Reddit {}", old_url))?;

        let status = resp.status().as_u16();
        let final_url = resp.url().to_string();
        let ssl_valid = final_url.starts_with("https://");
        let body_bytes = resp
            .bytes()
            .await
            .context("failed to read Reddit HTML body")?;
        let body = String::from_utf8_lossy(&body_bytes);
        let elapsed = start.elapsed().as_millis() as u64;

        self.extract_from_html(&body, url, &final_url, status, ssl_valid, elapsed, None)
    }

    /// Build a Reddit request with descriptive User-Agent and rate-limit delay.
    fn build_reddit_request(&self, url: &str) -> reqwest::RequestBuilder {
        // Respect rate limits — sleep before each Reddit request
        std::thread::sleep(super::reddit::REDDIT_RATE_LIMIT);

        self.client
            .get(url)
            .header("User-Agent", super::reddit::REDDIT_USER_AGENT)
            .header(
                "Accept",
                "application/json, text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
            )
    }

    /// Build a StructuredContent from extracted Reddit text.
    fn build_reddit_content(
        &self,
        original_url: &str,
        final_url: &str,
        status_code: u16,
        fetch_duration_ms: u64,
        text: &str,
    ) -> Result<StructuredContent> {
        let collapsed = normalize_text(text);
        let title = collapsed
            .lines()
            .find(|l| !l.trim().is_empty())
            .map(|l| l.trim().chars().take(120).collect())
            .unwrap_or_else(|| title_from_url(final_url));

        let excerpt = collapsed.chars().take(300).collect::<String>();
        let word_count = count_words(&collapsed);
        let sentence_count = count_sentences(&collapsed);
        let reading_time_seconds = estimate_reading_time(word_count);
        let reading_ease = textstat::flesch_reading_ease(&collapsed);
        let grade_level = textstat::flesch_kincaid_grade(&collapsed);
        let keywords = extract_keywords(&collapsed, 15);
        let (language, language_confidence) = whatlang::detect(&collapsed)
            .map(|i| (i.lang().code().to_string(), i.confidence() as f64))
            .unwrap_or_else(|| ("en".to_string(), 0.3));
        let content_markdown = collapsed.clone();

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
            modified_at: None,
            author: None,
            site_name: Some("reddit.com".to_string()),
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
            schema_type: Some("reddit".to_string()),
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: collapsed,
            fetched_at: Utc::now(),
            fetch_duration_ms,
            html_size_bytes: 0,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            content_type: "reddit/json".to_string(),
            content_type_header: "application/json".to_string(),
            is_paywalled: false,
            is_valid_content: word_count >= MIN_VALID_WORDS,
            entities: extract_entities(text),
        })
    }

    /// Extract StructuredContent from raw HTML.
    pub fn extract_from_html(
        &self,
        html: &str,
        original_url: &str,
        final_url: &str,
        status_code: u16,
        ssl_valid: bool,
        fetch_duration_ms: u64,
        last_modified: Option<&str>,
    ) -> Result<StructuredContent> {
        let doc = Html::parse_document(html);

        // 1. Readability extraction
        let article = Readability::new(html, None)
            .ok()
            .and_then(|mut r| r.parse());

        // Fallback body text when readability cannot isolate the article.
        let fallback_text = fallback_body_text(&doc);

        // 2. Metadata from <head>
        let meta = extract_meta(&doc);

        // 3. OpenGraph
        let open_graph = extract_open_graph(&doc);

        // 4. Twitter Card
        let twitter_card = extract_twitter_card(&doc);

        // 5. JSON-LD
        let json_ld = extract_json_ld(&doc);

        // 6. Links
        let (internal_links, external_links) = extract_links(&doc, final_url);

        // 7. Images
        let images = extract_images(&doc);

        // 8. Title: readability > meta > og > h1 > URL path
        let h1_title = extract_h1(&doc);
        let title = article
            .as_ref()
            .and_then(|a| a.title.clone())
            .or_else(|| meta.title.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.title.clone()))
            .or(h1_title)
            .unwrap_or_else(|| title_from_url(final_url))
            .trim()
            .to_string();

        // 9. Content text
        let content_text = article
            .as_ref()
            .and_then(|a| a.text_content.clone())
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(fallback_text);

        // 10. Content HTML — cleaned for AI agent consumption
        let raw_html = article
            .as_ref()
            .and_then(|a| a.content.clone())
            .unwrap_or_else(|| format!("<div>{}</div>", html_escape(&content_text)));
        let content_html = clean_content_html(&raw_html);

        // 11. Excerpt
        let excerpt = article
            .as_ref()
            .and_then(|a| a.excerpt.clone())
            .or_else(|| meta.description.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.description.clone()))
            .unwrap_or_default();

        // 12. Author
        let author = article
            .as_ref()
            .and_then(|a| a.byline.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.article_author.clone()))
            .or_else(|| meta.author.clone());

        // 13. Site name
        let site_name = article
            .as_ref()
            .and_then(|a| a.site_name.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.site_name.clone()))
            .or_else(|| extract_site_name(final_url));

        // 14. Language detection
        let (language, language_confidence) = detect_language(&content_text, html);

        // 15. Published date (priority: readability > JSON-LD > og > meta)
        let published_at = article
            .as_ref()
            .and_then(|a| a.published_time.clone())
            .or_else(|| {
                open_graph
                    .as_ref()
                    .and_then(|og| og.article_published_time.clone())
            })
            .or_else(|| meta.published_time.clone())
            .as_deref()
            .and_then(parse_date);

        // 16. Modified date
        let modified_at = open_graph
            .as_ref()
            .and_then(|og| og.article_modified_time.clone())
            .or_else(|| meta.modified_time.clone())
            .or_else(|| meta.updated_time.clone())
            .or_else(|| meta.last_modified.clone())
            .or_else(|| last_modified.map(String::from))
            .as_deref()
            .and_then(parse_date);

        // 17. Content metrics
        let word_count = count_words(&content_text);
        let char_count = content_text.len() as u32;
        let sentence_count = count_sentences(&content_text);
        let reading_time_seconds = estimate_reading_time(word_count);
        let reading_ease = textstat::flesch_reading_ease(&content_text);
        let grade_level = textstat::flesch_kincaid_grade(&content_text);

        // 18. Keywords
        let keywords = extract_keywords(&content_text, 15);

        // 19. Normalize text
        let normalized_text = normalize_text(&content_text);

        // 19b. Regex entity extraction — deterministic facts for the LLM.
        let entities = extract_entities(&content_text);

        // Pre-compute values before moves
        let content_markdown = html_to_markdown(&content_html);
        let is_valid = !content_text.is_empty() && word_count >= MIN_VALID_WORDS;

        // 20. Schema type from JSON-LD
        let schema_type = json_ld
            .iter()
            .find_map(|v| v.get("@type").and_then(|t| t.as_str()).map(String::from));

        // 21. Paywall detection
        let is_paywalled = detect_paywall(html);

        // 22. Favicon
        let favicon = extract_favicon(&doc);

        // 23. RSS
        let rss_url = extract_rss(&doc);

        // 24. Encoding from content type (if we had headers)
        let encoding: Option<String> = None;

        Ok(StructuredContent {
            url: original_url.to_string(),
            final_url: final_url.to_string(),
            status_code,
            title,
            description: meta.description,
            canonical_url: meta.canonical_url,
            language,
            language_confidence,
            published_at,
            modified_at,
            author,
            site_name,
            content_text,
            content_html,
            content_markdown,
            excerpt,
            word_count,
            char_count,
            sentence_count,
            reading_time_seconds,
            reading_ease,
            grade_level,
            keywords,
            open_graph,
            twitter_card,
            json_ld,
            schema_type,
            images,
            internal_links,
            external_links,
            favicon,
            rss_url,
            normalized_text,
            fetched_at: Utc::now(),
            fetch_duration_ms,
            html_size_bytes: html.len() as u64,
            encoding,
            ssl_valid,
            redirect_count: 0,
            content_type: String::new(),
            content_type_header: String::new(),
            is_paywalled,
            is_valid_content: is_valid,
            entities,
        })
    }
}

// ── Content-Type classification ───────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentKind {
    Html,
    Text,
    Json,
    Xml,
    Binary,
}

/// Classify a Content-Type header into a broad extraction strategy.
fn parse_content_type_header(ct: Option<&str>) -> (ContentKind, Option<String>) {
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
fn extract_from_text(
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
    let keywords = extract_keywords(&collapsed, 15);
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
        entities: Entities::default(),
    })
}

/// Build a minimal invalid record for binary or otherwise un-indexable URLs.
fn make_invalid_content(
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

struct PageMeta {
    title: Option<String>,
    description: Option<String>,
    canonical_url: Option<String>,
    published_time: Option<String>,
    modified_time: Option<String>,
    updated_time: Option<String>,
    last_modified: Option<String>,
    author: Option<String>,
}

fn extract_meta(doc: &Html) -> PageMeta {
    let mut title = None;
    let mut description = None;
    let mut canonical_url = None;
    let mut published_time = None;
    let mut modified_time = None;
    let mut updated_time = None;
    let mut last_modified = None;
    let mut author = None;

    // <title>
    if let Ok(sel) = Selector::parse("title") {
        if let Some(el) = doc.select(&sel).next() {
            let t: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            if !t.is_empty() {
                title = Some(t);
            }
        }
    }

    // <meta> tags
    if let Ok(sel) = Selector::parse("meta[name],meta[property]") {
        for el in doc.select(&sel) {
            let name = el
                .value()
                .attr("name")
                .or_else(|| el.value().attr("property"));
            let content = el.value().attr("content");
            if let (Some(name), Some(content)) = (name, content) {
                match name {
                    "description" => {
                        if description.is_none() {
                            description = Some(content.to_string());
                        }
                    }
                    "author" => author = Some(content.to_string()),
                    "article:published_time"
                    | "datePublished"
                    | "publish_date"
                    | "date"
                    | "publish-date" => {
                        if published_time.is_none() {
                            published_time = Some(content.to_string());
                        }
                    }
                    "article:modified_time"
                    | "dateModified"
                    | "modified_time"
                    | "modified-date" => {
                        if modified_time.is_none() {
                            modified_time = Some(content.to_string());
                        }
                    }
                    "updated_time" | "dateUpdated" | "updated-date" => {
                        if updated_time.is_none() {
                            updated_time = Some(content.to_string());
                        }
                    }
                    "last-modified" | "last_modified" => {
                        if last_modified.is_none() {
                            last_modified = Some(content.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // <link rel="canonical">
    if let Ok(sel) = Selector::parse("link[rel=canonical]") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                canonical_url = Some(href.to_string());
            }
        }
    }

    PageMeta {
        title,
        description,
        canonical_url,
        published_time,
        modified_time,
        updated_time,
        last_modified,
        author,
    }
}

// ── OpenGraph ───────────────────────────────────────────────────────────────

fn extract_open_graph(doc: &Html) -> Option<OpenGraph> {
    let mut og = OpenGraph {
        title: None,
        r#type: None,
        image: None,
        url: None,
        description: None,
        site_name: None,
        locale: None,
        article_author: None,
        article_published_time: None,
        article_modified_time: None,
        article_section: None,
        article_tags: vec![],
    };

    if let Ok(sel) = Selector::parse("meta[property^=\"og:\"]") {
        for el in doc.select(&sel) {
            let prop = el.value().attr("property")?;
            let content = el.value().attr("content")?;
            match prop {
                "og:title" => og.title = Some(content.to_string()),
                "og:type" => og.r#type = Some(content.to_string()),
                "og:image" => og.image = Some(content.to_string()),
                "og:url" => og.url = Some(content.to_string()),
                "og:description" => og.description = Some(content.to_string()),
                "og:site_name" => og.site_name = Some(content.to_string()),
                "og:locale" => og.locale = Some(content.to_string()),
                "article:author" => og.article_author = Some(content.to_string()),
                "article:published_time" => og.article_published_time = Some(content.to_string()),
                "article:modified_time" => og.article_modified_time = Some(content.to_string()),
                "article:section" => og.article_section = Some(content.to_string()),
                "article:tag" => og.article_tags.push(content.to_string()),
                _ => {}
            }
        }
    }

    if og.title.is_some() || og.description.is_some() {
        Some(og)
    } else {
        None
    }
}

// ── Twitter Card ────────────────────────────────────────────────────────────

fn extract_twitter_card(doc: &Html) -> Option<TwitterCard> {
    let mut tc = TwitterCard {
        card: None,
        site: None,
        creator: None,
        title: None,
        description: None,
        image: None,
    };

    if let Ok(sel) = Selector::parse("meta[name^=\"twitter:\"]") {
        for el in doc.select(&sel) {
            let name = el.value().attr("name")?;
            let content = el.value().attr("content")?;
            match name {
                "twitter:card" => tc.card = Some(content.to_string()),
                "twitter:site" => tc.site = Some(content.to_string()),
                "twitter:creator" => tc.creator = Some(content.to_string()),
                "twitter:title" => tc.title = Some(content.to_string()),
                "twitter:description" => tc.description = Some(content.to_string()),
                "twitter:image" => tc.image = Some(content.to_string()),
                _ => {}
            }
        }
    }

    if tc.card.is_some() || tc.title.is_some() {
        Some(tc)
    } else {
        None
    }
}

// ── JSON-LD ─────────────────────────────────────────────────────────────────

fn extract_json_ld(doc: &Html) -> Vec<serde_json::Value> {
    let mut results = vec![];
    if let Ok(sel) = Selector::parse("script[type=\"application/ld+json\"]") {
        for el in doc.select(&sel) {
            let text: String = el.text().collect();
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&text) {
                results.push(val);
            }
        }
    }
    results
}

// ── Links ───────────────────────────────────────────────────────────────────

fn extract_links(doc: &Html, base_url: &str) -> (Vec<String>, Vec<String>) {
    let base_domain = extract_domain(base_url);
    let mut internal = vec![];
    let mut external = vec![];
    let mut seen = HashSet::new();

    if let Ok(sel) = Selector::parse("a[href]") {
        for el in doc.select(&sel) {
            if let Some(href) = el.value().attr("href") {
                let href = href.trim();
                if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
                    continue;
                }
                if !seen.insert(href) {
                    continue;
                }

                let full_url = if href.starts_with("http") {
                    href.to_string()
                } else if href.starts_with("//") {
                    format!("https:{}", href)
                } else if href.starts_with('/') {
                    if let Some(pos) = base_url.find("://") {
                        let scheme_end = pos + 3;
                        if let Some(slash_pos) = base_url[scheme_end..].find('/') {
                            format!("{}{}", &base_url[..scheme_end + slash_pos], href)
                        } else {
                            format!("{}{}", base_url, href)
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                let link_domain = extract_domain(&full_url);
                if link_domain == base_domain || link_domain.is_empty() {
                    internal.push(full_url);
                } else {
                    external.push(full_url);
                }
            }
        }
    }

    (internal, external)
}

// ── Images ──────────────────────────────────────────────────────────────────

fn extract_images(doc: &Html) -> Vec<ImageInfo> {
    let mut images = vec![];
    if let Ok(sel) = Selector::parse("img[src]") {
        for el in doc.select(&sel) {
            if let Some(src) = el.value().attr("src") {
                let alt = el.value().attr("alt").map(String::from);
                let width = el.value().attr("width").and_then(|w| w.parse::<u32>().ok());
                let height = el
                    .value()
                    .attr("height")
                    .and_then(|h| h.parse::<u32>().ok());
                images.push(ImageInfo {
                    url: src.to_string(),
                    alt,
                    width,
                    height,
                });
            }
        }
    }
    images
}

// ── Favicon ─────────────────────────────────────────────────────────────────

fn extract_favicon(doc: &Html) -> Option<String> {
    if let Ok(sel) = Selector::parse("link[rel~=\"icon\"]") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                return Some(href.to_string());
            }
        }
    }
    Some("/favicon.ico".to_string())
}

// ── RSS ─────────────────────────────────────────────────────────────────────

fn extract_rss(doc: &Html) -> Option<String> {
    if let Ok(sel) = Selector::parse("link[type=\"application/rss+xml\"]") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(href) = el.value().attr("href") {
                return Some(href.to_string());
            }
        }
    }
    None
}

// ── Language Detection ──────────────────────────────────────────────────────

fn detect_language(text: &str, html: &str) -> (String, f64) {
    if let Some(info) = whatlang::detect(text) {
        let lang = info.lang().code().to_string();
        let conf = info.confidence();
        return (lang, conf as f64);
    }

    // Fallback: check html lang attribute
    let doc = Html::parse_document(html);
    if let Ok(sel) = Selector::parse("html[lang]") {
        if let Some(el) = doc.select(&sel).next() {
            if let Some(lang) = el.value().attr("lang") {
                let code = lang.split('-').next().unwrap_or("en").to_string();
                return (code, 0.5);
            }
        }
    }

    ("en".to_string(), 0.3)
}

// ── Date Parsing ────────────────────────────────────────────────────────────

fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    dateparser::parse(s).ok()
}

// ── Keyword Extraction ──────────────────────────────────────────────────────

pub(crate) fn extract_keywords(text: &str, max: usize) -> Vec<Keyword> {
    if text.len() < 100 {
        return vec![];
    }

    let stop_words: Vec<String> = stop_words::get(stop_words::LANGUAGE::English)
        .into_iter()
        .collect();

    let rake = keyword_extraction::rake::Rake::new(
        keyword_extraction::rake::RakeParams::WithDefaults(text, &stop_words),
    );

    let ranked = rake.get_ranked_keyword_scores(max);

    ranked
        .into_iter()
        .enumerate()
        .map(|(i, (word, score))| Keyword {
            text: word,
            tfidf_score: score as f64,
            rank: (i + 1) as u32,
        })
        .collect()
}

// ── Word / Sentence Counting ───────────────────────────────────────────────

pub(crate) fn count_words(text: &str) -> u32 {
    text.unicode_words().count() as u32
}

pub(crate) fn count_sentences(text: &str) -> u32 {
    text.unicode_sentences().count() as u32
}

pub(crate) fn estimate_reading_time(word_count: u32) -> u32 {
    (word_count as f64 / 238.0 * 60.0).ceil() as u32
}

// ── Text Normalization ──────────────────────────────────────────────────────

pub(crate) fn normalize_text(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn html_to_markdown(html: &str) -> String {
    html2text::from_read(html.as_bytes(), 120).unwrap_or_default()
}

// ── Regex entity extraction ───────────────────────────────────────────────
//
// Deterministic, zero-cost entity extraction (emails, phones, addresses, ...)
// applied to the *clean* page text inside the fetch pipeline. This gives the
// LLM machine-checkable facts instead of raw text and avoids paying tokens or
// risking nondeterminism by asking a model to extract them. Regexes are
// compiled once and reused via std LazyLock.

/// Emails: standard local@domain[.tld], tolerant of a trailing period.
static RE_EMAIL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}\b").expect("valid email regex")
});

/// Phone numbers: E.164/US-centric — optional +country, then 7-15 digits
/// separated by space/dash/dot. Requires at least 7 digits total so a date
/// like `2017-05-13` (fewer digits) is not misclassified as a phone.
static RE_PHONE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?:\+?\d{1,3}[\s\-\.]?)?\(?\d{3}\)?[\s\-\.]?\d{3}[\s\-\.]\d{4}\b")
        .expect("valid phone regex")
});

/// Addresses: a leading number + street words, then a street suffix, then an
/// optional city / state / ZIP tail (e.g. "Austin, TX 78701").
static RE_ADDRESS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b\d{1,6}\s+(?:[a-z0-9]+\.?[\s\-']+){1,6}(?:street|st|avenue|ave|road|rd|boulevard|blvd|lane|ln|drive|dr|court|ct|way|place|pl|highway|hwy|parkway|pkwy|square|sq|suite|ste)\b[,\s]*(?:[a-z][a-z\s.]*?)?(?:[,]\s*[a-z]{2})?\s*\d{5}(?:-\d{4})?",
    )
    .expect("valid address regex")
});

/// URLs/URIs: http(s), ftp, mailto, www, or a bare domain with a path.
/// Uses `[^\s<>]` (no quote chars in the class) so it stays a valid raw string.
static RE_URL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)\b(?:https?://|ftp://|mailto:|www\.)[^\s<>]+|(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\.)+(?:com|org|net|io|ai|dev|app|co|edu|gov|info|me|us|uk|ca|de|fr|in)(?:/[^\s<>]*)?",
    )
    .expect("valid url regex")
});

/// Prices: currency symbol then digits (with optional cents), or digits then symbol.
static RE_PRICE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:\$\s?\d{1,3}(?:,\d{3})*(?:\.\d{2})?|(?:\d{1,3}(?:,\d{3})*(?:\.\d{2})?)\s?(?:USD|EUR|GBP|€|£))",
    )
    .expect("valid price regex")
});

/// Dates: ISO 8601, and common human dates (Month D, YYYY; D Month YYYY; MM/DD/YYYY).
static RE_DATE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?ix)\b\d{4}-\d{1,2}-\d{1,2}\b|\b(?:jan|feb|mar|apr|may|jun|jul|aug|sep|oct|nov|dec)[a-z]*[\s.,]+\d{1,2}[\s.,]*(?:,?\s*\d{2,4})?\b|\b\d{1,2}[/-]\d{1,2}[/-]\d{2,4}\b",
    )
    .expect("valid date regex")
});

/// IPv4 / IPv6 addresses.
static RE_IP: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:\b(?:25[0-5]|2[0-4][0-9]|1?[0-9][0-9]?)(?:\.(?:25[0-5]|2[0-4][0-9]|1?[0-9][0-9]?)){3}\b|(?i)\b(?:[a-f0-9]{1,4}:){2,7}[a-f0-9]{1,4}\b)",
    )
    .expect("valid ip regex")
});

/// Social handles: @username for Twitter/X, GitHub, etc. Must be preceded by
/// whitespace or line start so an email local-part (`name@example.com`) is not
/// mistaken for a handle.
static RE_SOCIAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?:\A|\s)(@[a-zA-Z0-9_]{3,30})\b").expect("valid social regex")
});

/// Extract structured entities from cleaned page text.
pub(crate) fn extract_entities(text: &str) -> Entities {
    fn unique(v: Vec<String>) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        v.into_iter()
            .filter(|s| !s.trim().is_empty())
            .filter(|s| seen.insert(s.to_lowercase()))
            .collect()
    }

    let emails = unique(
        RE_EMAIL
            .find_iter(text)
            .map(|m| m.as_str().trim_end_matches('.').to_string())
            .collect(),
    );
    let phones = unique(
        RE_PHONE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let addresses = unique(
        RE_ADDRESS
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let urls = unique(
        RE_URL
            .find_iter(text)
            .map(|m| {
                m.as_str()
                    .trim_end_matches(['.', ')', ';', ','].as_slice())
                    .to_string()
            })
            .collect(),
    );
    let prices = unique(
        RE_PRICE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let dates = unique(
        RE_DATE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let ip_addresses = unique(
        RE_IP
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let social_handles = unique(
        RE_SOCIAL
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .collect(),
    );

    Entities {
        emails,
        phones,
        addresses,
        urls,
        prices,
        dates,
        ip_addresses,
        social_handles,
    }
}

/// Strip web noise from HTML content using proper DOM traversal, retaining only
/// semantic structural elements suitable for AI agent consumption.
///
/// Pipeline:
///   1. Strip HTML comments (`<!-- ... -->`)
///   2. Parse with HTML5 parser (handles malformed input gracefully)
///   3. DOM tree traversal: only keep structural/semantic tags
///   4. Normalize whitespace
///
/// Retains: h1-h6, p, div, span, a, ul, ol, li, table, pre, code, blockquote, etc.
/// Removes: script, style, nav, header, footer, aside, form, input, iframe, etc.
fn clean_content_html(html: &str) -> String {
    // Step 1: strip HTML comments
    let no_comments = strip_html_comments(html);
    // Step 2: parse fragment via HTML5 spec parser
    let doc = Html::parse_fragment(&no_comments);

    // Step 3: traverse the body and rebuild clean HTML
    let mut out = String::with_capacity(no_comments.len());
    if let Ok(body_sel) = Selector::parse("body") {
        if let Some(body) = doc.select(&body_sel).next() {
            traverse_clean(body, &mut out);
        }
    }

    // Step 4: normalize whitespace
    normalize_html_whitespace(&out)
}

/// Tags whose entire subtree is kept (structural + semantic content).
/// Matches PLAN.md spec: h1-h6, p, div, span, standard inline formatting.
const CLEAN_KEEP: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "div",
    "span",
    "a",
    "ul",
    "ol",
    "li",
    "table",
    "tr",
    "td",
    "th",
    "pre",
    "code",
    "blockquote",
    "em",
    "strong",
    "i",
    "b",
    "u",
    "br",
    "hr",
    "img",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "abbr",
    "cite",
    "q",
    "sub",
    "sup",
    "section",
    "article",
    "main",
];

/// Tags that are completely removed including their content.
/// Matches PLAN.md spec: script, style, nav, header, footer, aside, form, button, head, noscript.
const CLEAN_REMOVE: &[&str] = &[
    "script", "style", "noscript", "iframe", "canvas", "svg", "nav", "header", "footer", "aside",
    "form", "button", "input", "select", "textarea", "label", "option", "head", "link", "meta",
];

/// Recursive DOM traversal: rebuilds clean HTML from the scraper tree.
fn traverse_clean(el: scraper::ElementRef, out: &mut String) {
    let tag = el.value().name();
    let tag_lower = tag.to_ascii_lowercase();

    if CLEAN_REMOVE.contains(&tag_lower.as_str()) {
        return;
    }

    // Void / self-closing elements — emit inline
    if tag_lower == "br" {
        out.push_str("<br>");
        return;
    }
    if tag_lower == "hr" {
        out.push_str("<hr>");
        return;
    }
    if tag_lower == "img" {
        if let Some(src) = el.value().attr("src") {
            let alt = el.value().attr("alt").unwrap_or("");
            out.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\">",
                html_escape(src),
                html_escape(alt)
            ));
        }
        return;
    }

    // Unknown / custom elements — strip entirely (e.g. <div class="ad">, <tracking-widget>)
    if !CLEAN_KEEP.contains(&tag_lower.as_str()) && !CLEAN_REMOVE.contains(&tag_lower.as_str()) {
        // Unknown tag: skip it + its content
        return;
    }

    // Open the tag with preserved attributes for critical elements
    out.push('<');
    out.push_str(tag_lower.as_str());

    // Preserve href on links (traceability / citation)
    if tag_lower == "a" {
        if let Some(href) = el.value().attr("href") {
            out.push_str(&format!(" href=\"{}\"", html_escape(href)));
        }
    }

    out.push('>');

    // Recursively process children
    for child in el.children() {
        if let Some(child_el) = scraper::ElementRef::wrap(child) {
            traverse_clean(child_el, out);
        } else if let Some(text) = child.value().as_text() {
            out.push_str(text);
        }
    }

    out.push_str("</");
    out.push_str(tag_lower.as_str());
    out.push('>');
    out.push('\n');
}

/// Strip HTML comments (`<!-- ... -->`) from a string.
fn strip_html_comments(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Check for <!--
        if i + 3 < len
            && bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && bytes[i + 2] == b'-'
            && bytes[i + 3] == b'-'
        {
            // Skip until -->
            i += 4;
            while i + 2 < len && !(bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>')
            {
                i += 1;
            }
            i += 3; // skip -->
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Collapse consecutive whitespace (including newlines) into single spaces,
/// then trim leading/trailing whitespace.
fn normalize_html_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

// ── Paywall Detection ───────────────────────────────────────────────────────

fn detect_paywall(html: &str) -> bool {
    let indicators = [
        "paywall",
        "subscribe to continue",
        "membership required",
        "premium content",
        "sign in to read",
        "members-only",
        "subscription required",
        "behind a paywall",
    ];
    let lower = html.to_lowercase();
    indicators.iter().any(|i| lower.contains(i))
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn extract_domain(url: &str) -> String {
    super::util::extract_domain(url).unwrap_or_default()
}

/// Extract the first <h1> text as a title fallback.
fn extract_h1(doc: &Html) -> Option<String> {
    Selector::parse("h1")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|el| el.text().collect::<Vec<_>>().join("").trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Strip scripts, styles, and noscript tags, then collect visible body text.
fn fallback_body_text(doc: &Html) -> String {
    let mut text_parts = Vec::new();
    if let Ok(body_sel) = Selector::parse("body") {
        if let Some(body) = doc.select(&body_sel).next() {
            let skip_sel = Selector::parse(
                "script,style,noscript,iframe,canvas,svg,nav,header,footer,aside,form,button",
            )
            .ok();
            collect_text_nodes(body, &skip_sel, &mut text_parts);
        }
    }
    normalize_text(&text_parts.join(" "))
}

fn collect_text_nodes(
    node: scraper::ElementRef,
    skip_sel: &Option<Selector>,
    out: &mut Vec<String>,
) {
    for child in node.children() {
        if let Some(child_el) = scraper::ElementRef::wrap(child) {
            if skip_sel
                .as_ref()
                .map(|_sel| {
                    child_el.value().name() == "script"
                        || child_el.value().name() == "style"
                        || child_el.value().name() == "noscript"
                        || child_el.value().name() == "iframe"
                        || child_el.value().name() == "canvas"
                        || child_el.value().name() == "svg"
                })
                .unwrap_or(false)
            {
                continue;
            }
            collect_text_nodes(child_el, skip_sel, out);
        } else if let Some(text) = child.value().as_text() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
}

/// Build a human-readable title from the last path segment of a URL.
pub(crate) fn title_from_url(url: &str) -> String {
    let path = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('?')
        .next()
        .unwrap_or(url);
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .map(|s| percent_decode(s))
        .unwrap_or_else(|| extract_domain(url))
        .trim()
        .to_string()
}

/// Decode common percent-encoded characters in a URL fragment.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let a = chars.next();
            let b = chars.next();
            if let (Some(a), Some(b)) = (a, b) {
                if let Ok(byte) = u8::from_str_radix(&format!("{}{}", a, b), 16) {
                    out.push(byte as char);
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// Derive a site name from the registered domain, or fall back to the full host.
pub(crate) fn extract_site_name(url: &str) -> Option<String> {
    let host = extract_domain(url);
    if host.is_empty() {
        return None;
    }
    // Strip a leading "www." again and return the domain without port.
    host.split(':')
        .next()
        .map(|h| h.trim_start_matches("www.").to_string())
        .filter(|h| !h.is_empty())
}

/// Minimal HTML-escape for fallback plain text blocks.
pub(crate) fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_meta() {
        let html = "<html><head>\
            <title>My Page Title</title>\
            <meta name=\"description\" content=\"A great page about Rust\">\
            <meta name=\"author\" content=\"Gaurav\">\
            <meta property=\"og:title\" content=\"OG Title\">\
            <link rel=\"canonical\" href=\"https://example.com/canonical\">\
            </head><body><p>Hello world</p></body></html>";
        let doc = Html::parse_document(html);
        let meta = extract_meta(&doc);
        assert_eq!(meta.title.as_deref(), Some("My Page Title"));
        assert_eq!(meta.description.as_deref(), Some("A great page about Rust"));
        assert_eq!(meta.author.as_deref(), Some("Gaurav"));
        assert_eq!(
            meta.canonical_url.as_deref(),
            Some("https://example.com/canonical")
        );
    }

    #[test]
    fn test_extract_open_graph() {
        let html = "<html><head>\
            <meta property=\"og:title\" content=\"OG Title\">\
            <meta property=\"og:description\" content=\"OG Desc\">\
            <meta property=\"og:image\" content=\"https://example.com/img.jpg\">\
            <meta property=\"og:type\" content=\"article\">\
            </head><body></body></html>";
        let doc = Html::parse_document(html);
        let og = extract_open_graph(&doc).unwrap();
        assert_eq!(og.title.as_deref(), Some("OG Title"));
        assert_eq!(og.description.as_deref(), Some("OG Desc"));
        assert_eq!(og.r#type.as_deref(), Some("article"));
    }

    #[test]
    fn test_extract_links() {
        let html = "<html><body>\
            <a href=\"/page1\">Internal</a>\
            <a href=\"https://external.com/page\">External</a>\
            <a href=\"#anchor\">Anchor</a>\
            <a href=\"javascript:void(0)\">JS</a>\
            </body></html>";
        let doc = Html::parse_document(html);
        let (internal, external) = extract_links(&doc, "https://example.com/home");
        assert!(internal.iter().any(|u| u.contains("/page1")));
        assert!(external.iter().any(|u| u.contains("external.com")));
        assert!(!internal.iter().any(|u| u.contains("#anchor")));
    }

    #[test]
    fn test_domain_extraction() {
        assert_eq!(
            extract_domain("https://www.example.com/path"),
            "example.com"
        );
        assert_eq!(
            extract_domain("http://blog.example.co.uk/post"),
            "blog.example.co.uk"
        );
    }

    #[test]
    fn test_word_count() {
        assert_eq!(count_words("Hello world"), 2);
        assert_eq!(count_words("one two three four five"), 5);
    }

    #[test]
    fn test_short_page_is_valid_content() {
        // A short-but-real page (e.g. example.com, ~19 words) must be treated
        // as valid content so webfind_fetch does not mark it invalid and drop
        // it. Regression for the >50 word threshold that pushed the LLM to
        // DuckDuckGo.
        let html = r#"<!DOCTYPE html><html><head><title>Example Domain</title></head>
<body><h1>Example Domain</h1>
<p>This domain is for use in documentation examples without needing permission. Avoid use in operations. Learn more.</p>
</body></html>"#;
        let fetcher = Fetcher::new().unwrap();
        let content = fetcher
            .extract_from_html(
                html,
                "https://example.com/",
                "https://example.com/",
                200,
                true,
                5,
                None,
            )
            .unwrap();
        assert!(content.word_count > 0);
        assert!(
            content.is_valid_content,
            "short page marked invalid (word_count={})",
            content.word_count
        );
    }

    #[test]
    fn test_empty_page_is_invalid_content() {
        let html = "<!DOCTYPE html><html><head><title>Empty</title></head><body></body></html>";
        let fetcher = Fetcher::new().unwrap();
        let content = fetcher
            .extract_from_html(html, "https://x.com/", "https://x.com/", 200, true, 5, None)
            .unwrap();
        assert!(!content.is_valid_content, "empty page must stay invalid");
    }

    #[test]
    fn test_reading_time() {
        assert_eq!(estimate_reading_time(238), 60);
        assert_eq!(estimate_reading_time(100), 26);
    }

    #[test]
    fn test_paywall_detection() {
        assert!(detect_paywall("Please subscribe to continue reading"));
        assert!(detect_paywall("This is premium content behind a paywall"));
        assert!(!detect_paywall("Normal article content"));
    }

    #[test]
    fn test_extract_entities() {
        let text = concat!(
            "Contact support@example.com or sales@corp.io. ",
            "Call +1 555-123-4567. ",
            "Visit https://example.com/pricing or www.example.org. ",
            "The product costs $1,299.00 (or 999 USD). ",
            "Founded on 2024-03-15. Server at 192.168.0.1. ",
            "Follow us @OpenCodeHQ. Office at 123 Main Street, Austin, TX 78701.",
        );
        let e = extract_entities(text);
        assert!(e.emails.iter().any(|m| m == "support@example.com"));
        assert!(e.emails.iter().any(|m| m == "sales@corp.io"));
        assert!(e.phones.iter().any(|m| m.contains("555-123-4567")));
        assert!(e.urls.iter().any(|m| m == "https://example.com/pricing"));
        assert!(e.prices.iter().any(|m| m.contains("1,299")));
        assert!(e.prices.iter().any(|m| m.contains("999")));
        assert!(e.dates.iter().any(|m| m == "2024-03-15"));
        assert!(e.ip_addresses.iter().any(|m| m == "192.168.0.1"));
        assert!(e.social_handles.iter().any(|m| m == "@OpenCodeHQ"));
        assert!(e.addresses.iter().any(|m| m.contains("123 Main Street")));
        assert!(!e.is_empty());
    }

    #[test]
    fn test_extract_entities_avoids_false_positives() {
        // A date must not be classified as a phone; an email local-part must
        // not be classified as a social handle.
        let e = extract_entities("Released 2017-05-13. Reach sales@corp.io directly.");
        assert!(
            e.phones.is_empty(),
            "date misclassified as phone: {:?}",
            e.phones
        );
        assert!(
            !e.social_handles.iter().any(|m| m.contains("corp")),
            "email local-part misclassified as handle: {:?}",
            e.social_handles
        );
        assert!(e.dates.iter().any(|m| m == "2017-05-13"));
        assert!(e.emails.iter().any(|m| m == "sales@corp.io"));
    }

    #[test]
    fn test_extract_entities_empty() {
        let e = extract_entities("Just some plain text with no structured data here.");
        assert!(e.is_empty());
        assert!(e.emails.is_empty() && e.phones.is_empty());
    }
}
