use std::time::Duration;

use anyhow::{Context, Result};
use chromiumoxide::browser::{Browser, BrowserConfig};
use futures::StreamExt;

/// Renders a URL in headless Chromium and returns the final HTML.
pub struct DynamicFetcher;

impl DynamicFetcher {
    pub fn new() -> Self {
        Self
    }

    /// Render a URL in Chromium and return the rendered HTML plus final URL.
    ///
    /// * `wait_ms` — time to wait after navigation for JS to execute.
    /// * `chrome_path` — optional executable path; falls back to `WEBFIND_CHROME_PATH`
    ///   env var, then to chromiumoxide's automatic detection.
    pub async fn render(&self, url: &str, wait_ms: u64) -> Result<(String, String)> {
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
        let _ = page.goto(url).await?;
        tokio::time::sleep(Duration::from_millis(wait_ms)).await;

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
