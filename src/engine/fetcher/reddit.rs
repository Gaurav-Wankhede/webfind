use std::time::Instant;
use anyhow::{Context, Result};
use chrono::Utc;
use crate::schema::content::*;
use super::text_process::{count_sentences, count_words, estimate_reading_time, extract_page_keywords, normalize_text};
use super::url_util::{html_escape, title_from_url};
use super::entities::extract_entities;
use super::{Fetcher, MIN_VALID_WORDS};

impl Fetcher {
pub(crate) async fn fetch_reddit(&self, url: &str) -> Result<StructuredContent> {
        let start = Instant::now();

        // Attempt 1: try the .json endpoint
        if let Some(json_url) = crate::engine::reddit::to_reddit_json_url(url) {
            tokio::time::sleep(crate::engine::reddit::REDDIT_RATE_LIMIT).await;
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

                    match crate::engine::reddit::parse_reddit_listing(&body) {
                        Ok(items) if !items.is_empty() => {
                            let text = crate::engine::reddit::listing_to_text(&items);
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
        tokio::time::sleep(crate::engine::reddit::REDDIT_RATE_LIMIT).await;
        let old_url = crate::engine::reddit::to_old_reddit_url(url);
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
        let body = match simdutf8::compat::from_utf8(&body_bytes) {
            Ok(valid_str) => std::borrow::Cow::Borrowed(valid_str),
            Err(_) => String::from_utf8_lossy(&body_bytes),
        };
        let elapsed = start.elapsed().as_millis() as u64;

        self.extract_from_html(&body, url, &final_url, status, ssl_valid, elapsed, None)
    }

    /// Build a Reddit request with descriptive User-Agent without blocking thread sleep.
    pub(crate) fn build_reddit_request(&self, url: &str) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .header("User-Agent", crate::engine::reddit::REDDIT_USER_AGENT)
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
        let keywords = extract_page_keywords(&collapsed, 15);
        let (language, language_confidence) = whatlang::detect(&collapsed)
            .map(|i| (i.lang().code().to_string(), i.confidence()))
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

    
}
