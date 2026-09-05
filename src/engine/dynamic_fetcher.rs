use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;

/// Renders a URL in headless Chromium and returns the final HTML.
///
/// Used as the CDP (browser) backend for JS-heavy / infinite-scroll pages that a
/// plain HTTP fetch cannot render. Supports stealth mode to bypass bot
/// detection and bounded infinite-scroll to load progressively-rendered content
/// (the deep-research browser mode).
pub struct DynamicFetcher;

/// How many times to scroll a page to trigger lazy / infinite content loads.
const MAX_SCROLLS: u32 = 10;
/// Pause between scroll steps so the page can fetch + render new content.
const SCROLL_PAUSE_MS: u64 = 700;

impl DynamicFetcher {
    pub fn new() -> Self {
        Self
    }

    /// Render a URL in Chromium and return the rendered HTML plus final URL.
    ///
    /// * `wait_ms` — initial settle time after navigation for JS to execute.
    /// * `deep` — when true, enable stealth mode + infinite-scroll so
    ///   progressively rendered pages are fully captured.
    /// * `chrome_path` — optional executable path; falls back to `WEBFIND_CHROME_PATH`
    ///   env var, then to chromiumoxide's automatic detection.
    pub async fn render(&self, url: &str, wait_ms: u64, deep: bool) -> Result<(String, String)> {
        let chrome_path = std::env::var("WEBFIND_CHROME_PATH").ok();
        let mut builder = BrowserConfig::builder()
            .disable_cache()
            .hide()
            .window_size(1366, 768)
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--no-sandbox");

        if let Some(path) = chrome_path {
            builder = builder.chrome_executable(path);
        }

        let config = builder
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build browser config: {}", e))?;

        let (mut browser, mut handler) = Browser::launch(config)
            .await
            .context("failed to launch Chromium")?;

        let handle = tokio::spawn(async move {
            while let Some(h) = handler.next().await {
                if h.is_err() {
                    break;
                }
            }
        });

        let page = browser.new_page("about:blank").await?;

        // Stealth: hide automation fingerprints so bot-protected sites
        // (Cloudflare, reCAPTCHA) serve real content instead of a challenge.
        if deep {
            let _ = page.enable_stealth_mode().await;
        }

        let _ = page.goto(url).await?;
        // Initial settle for JS execution / XHR hydration.
        tokio::time::sleep(Duration::from_millis(wait_ms.max(300))).await;

        // Bounded infinite-scroll: progressively scroll to bottom to trigger
        // lazy-loading / paginated feed content, then capture the full HTML.
        if deep {
            for _ in 0..MAX_SCROLLS {
                let scrolled = page
                    .evaluate_expression(
                        "(() => { const before = document.documentElement.scrollHeight; \
                         window.scrollTo(0, document.documentElement.scrollHeight); \
                         return before; })()",
                    )
                    .await
                    .is_ok();
                if !scrolled {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(SCROLL_PAUSE_MS)).await;
            }
        }

        let html = page.content().await?;
        let final_url = page.url().await?.unwrap_or_else(|| url.to_string());

        let _ = page.close().await;
        browser.close().await?;
        let _ = handle.await;

        Ok((html, final_url))
    }
}

impl Default for DynamicFetcher {
    fn default() -> Self {
        Self::new()
    }
}
