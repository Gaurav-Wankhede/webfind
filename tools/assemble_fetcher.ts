import * as fs from "fs";

// 1. Setup fetcher/raw_content.rs with impl Fetcher
let raw = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
if (!raw.includes("impl Fetcher {")) {
    raw = `use super::*;
use std::time::Instant;
use anyhow::{Context, Result};
use crate::schema::content::*;

impl Fetcher {
` + raw + "\n}\n";
    fs.writeFileSync("src/engine/fetcher/raw_content.rs", raw, "utf8");
    console.log("Updated fetcher/raw_content.rs");
}

// 2. Setup fetcher/reddit.rs with impl Fetcher
let reddit = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
if (!reddit.includes("impl Fetcher {")) {
    reddit = `use super::*;
use std::time::Instant;
use anyhow::{Context, Result};
use chrono::Utc;
use crate::schema::content::*;

impl Fetcher {
` + reddit + "\n}\n";
    fs.writeFileSync("src/engine/fetcher/reddit.rs", reddit, "utf8");
    console.log("Updated fetcher/reddit.rs");
}

// 3. Setup fetcher/extract.rs with impl Fetcher
let extract = fs.readFileSync("src/engine/fetcher/extract.rs", "utf8");
if (!extract.includes("impl Fetcher {")) {
    extract = `use super::*;
use std::time::Instant;
use anyhow::Result;
use scraper::Html;
use readability_rust::Readability;
use crate::schema::content::*;

impl Fetcher {
` + extract + "\n}\n";
    fs.writeFileSync("src/engine/fetcher/extract.rs", extract, "utf8");
    console.log("Updated fetcher/extract.rs");
}

// 4. Clean parent src/engine/fetcher.rs:
// Keep imports, constants, Fetcher struct, constructor/builder methods, and fetch_url.
// Declare submodules and re-export items so external modules and tests work seamlessly.
const parentCode = `use std::sync::Arc;
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

pub use cleaner::*;
pub use container::*;
pub use dom_entities::*;
pub use entities::*;
pub use meta::*;
pub use paywall::*;
pub use quality_entities::*;
pub use raw_content::*;
pub use text_process::*;
pub use url_util::*;

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
        profile: Option<DeviceProfile>,
        proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        audit_log: Option<Arc<dyn FingerprintAuditLog>>,
        rotate_ua: RotateUserAgent,
    ) -> Result<Self> {
        let (human_client, profile) = match (profile, proxy_pool) {
            (Some(prof), Some(pool)) => {
                let hc = HumanClient::new(
                    prof.clone(),
                    pool,
                    session_manager,
                    audit_log.clone(),
                    matches!(rotate_ua, RotateUserAgent::Rotate),
                )?;
                (Some(hc), Some(prof))
            }
            (Some(prof), None) => {
                let pool = Arc::new(crate::engine::proxy_pool::ProxyPool::new(vec![]));
                let hc = HumanClient::new(
                    prof.clone(),
                    pool,
                    session_manager,
                    audit_log.clone(),
                    matches!(rotate_ua, RotateUserAgent::Rotate),
                )?;
                (Some(hc), Some(prof))
            }
            _ => (None, None),
        };

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client,
            profile,
            audit_log,
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
        if reddit::is_reddit_url(url) {
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

fs.writeFileSync("src/engine/fetcher.rs", parentCode, "utf8");
console.log("Rewrote src/engine/fetcher.rs cleanly!");
