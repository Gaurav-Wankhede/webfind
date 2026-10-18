import * as fs from "fs";

// 1. fetcher.rs:
// Clean imports & re-exports
let f = `use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use reqwest::Client;

use crate::engine::device_profile::DeviceProfile;
use crate::engine::fingerprint::FingerprintAuditLog;
use crate::engine::human_client::HumanClient;
use crate::schema::content::StructuredContent;

pub mod cleaner;
pub mod container;
pub mod dom_entities;
pub mod entities;
pub mod extract;
pub mod meta;
pub mod paywall;
pub mod quality_entities;
pub mod raw_content;
pub mod reddit;
pub mod text_process;
pub mod url_util;

#[cfg(test)]
mod tests;

pub use cleaner::clean_content_html;
pub use container::{extract_domain, extract_doc_container, extract_h1, fallback_body_text};
pub use dom_entities::extract_dom_entities;
pub use entities::extract_entities;
pub use meta::{extract_meta, extract_open_graph, extract_twitter_card, extract_json_ld, extract_links, extract_images, extract_favicon, extract_rss, detect_language, parse_date};
pub use paywall::detect_paywall;
pub use quality_entities::{classify_diagrams, extract_quality_entities};
pub use raw_content::{parse_content_type_header, extract_from_text, make_invalid_content, ContentKind};
pub use text_process::{extract_page_keywords, count_words, count_sentences, estimate_reading_time, normalize_text, clean_html_streaming, content_score, html_to_markdown, strip_code_blocks, strip_junk_segments, looks_like_prose, looks_like_css, looks_like_javascript, derive_excerpt, is_empty_shell, text_to_html};
pub use url_util::{title_from_url, extract_site_name, html_escape};

const USER_AGENT: &str = crate::engine::util::USER_AGENT;
const FETCH_TIMEOUT_SECS: u64 = 30;

pub(crate) const MIN_VALID_WORDS: u32 = 5;

#[derive(Clone, Copy)]
pub enum RotateUserAgent {
    Rotate,
    Fixed,
}

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
            .build()
            .context("failed to build default HTTP client")?;

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

    pub fn from_client(client: Client) -> Result<Self> {
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

    pub fn with_profile(mut self, profile: DeviceProfile) -> Self {
        self.profile = Some(profile);
        self
    }

    pub fn new_human(
        proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        rotate_ua: RotateUserAgent,
        rps: Option<std::num::NonZeroU32>,
    ) -> Result<Self> {
        let mut b = HumanClient::builder();
        if let Some(pool) = proxy_pool {
            b = b.proxy_pool((*pool).clone());
        }
        if let Some(mgr) = session_manager {
            if let Ok(guard) = mgr.try_lock() {
                b = b.session_manager((*guard).clone());
            }
        }
        if let Some(r) = rps {
            b = b.requests_per_second(r);
        }
        let hc = b.build()?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(hc),
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }

    pub fn with_audit_log(mut self, audit_log: Arc<dyn FingerprintAuditLog>) -> Self {
        self.audit_log = Some(audit_log);
        self
    }

    pub fn with_dynamic_fallback(mut self, wait_ms: u64) -> Self {
        self.dynamic_fallback = true;
        self.dynamic_wait_ms = wait_ms;
        self
    }

    pub fn with_dynamic_deep(mut self, deep: bool) -> Self {
        self.dynamic_deep = deep;
        self
    }

    pub async fn fetch_url(&self, url: &str) -> Result<StructuredContent> {
        if crate::engine::reddit::is_reddit_url(url) {
            return self.fetch_reddit(url).await;
        }

        if let Ok(parsed_url) = url::Url::parse(url) {
            let path = parsed_url.path();
            let is_manifest_eligible = path.is_empty() || path == "/" || path == "/docs" || path == "/docs/";
            if is_manifest_eligible
                && parsed_url.query().is_none()
                && (parsed_url.scheme() == "http" || parsed_url.scheme() == "https")
                && let Some(manifest_content) = self.try_manifest_fast_path(&parsed_url).await
            {
                return Ok(manifest_content);
            }
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
        let (kind, charset) = parse_content_type_header(content_type.as_deref());

        let elapsed = start.elapsed().as_millis() as u64;

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

        let body_bytes = response
            .bytes()
            .await
            .context("failed to read response body")?;
        let body = match simdutf8::compat::from_utf8(&body_bytes) {
            Ok(valid_str) => std::borrow::Cow::Borrowed(valid_str),
            Err(_) => String::from_utf8_lossy(&body_bytes),
        };

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

        content.encoding = charset;
        content.content_type_header = content_type_header.clone();
        content.content_type =
            super::content_classifier::classify_content_type(&content).to_string();

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
                anyhow::bail!("dynamic fallback requested but dynamic feature is not enabled");
            }
        }

        Ok(content)
    }
}
`;
fs.writeFileSync("src/engine/fetcher.rs", f, "utf8");
console.log("Rewrote fetcher.rs cleanly!");

// 2. Fix reddit.rs: replace `super::reddit::` with `crate::engine::reddit::`
let r = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
r = r.replace(/super::reddit::/g, "crate::engine::reddit::");
fs.writeFileSync("src/engine/fetcher/reddit.rs", r, "utf8");
console.log("Fixed reddit.rs!");
