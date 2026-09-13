//! Shared HTTP client for engine adapters: user-agent rotation, per-request
//! timeouts, and anti-bot challenge detection.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{EngineOptions, Error};

/// Realistic desktop user agents, rotated per request so engines see varied
/// fingerprints and a blocked request can retry with a fresh identity.
const USER_AGENTS: &[&str] = &[
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36",
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/125.0.0.0 Safari/537.36",
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64; rv:127.0) Gecko/20100101 Firefox/127.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:127.0) Gecko/20100101 Firefox/127.0",
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Safari/605.1.15",
];

/// Body markers that indicate an anti-bot challenge page instead of results.
const BLOCK_MARKERS: &[&str] = &[
    "captcha",
    "unusual traffic",
    "access denied",
    "are you a robot",
    "just a moment",
    "verify you are human",
    "cf-challenge",
    "anomaly.js",
];

/// Shared, cloneable HTTP client with user-agent rotation.
///
/// Clones share the rotation counter, so concurrent searches never repeat the
/// same user agent in lockstep.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    ua_counter: Arc<AtomicUsize>,
}

impl Client {
    /// Build the shared client.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Transport`] when the underlying HTTP client cannot be
    /// constructed (e.g. invalid TLS configuration).
    pub fn new() -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            ua_counter: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Return the next user agent in the rotation.
    #[must_use]
    pub fn next_user_agent(&self) -> &'static str {
        let idx = self.ua_counter.fetch_add(1, Ordering::Relaxed) % USER_AGENTS.len();
        USER_AGENTS[idx]
    }

    /// Fetch a URL with a per-request timeout, returning the raw body.
    ///
    /// JSON/XML API adapters pass their own `accept` type and any extra
    /// headers (e.g. Marginalia's `API-Key: public`); HTML adapters use
    /// [`Self::fetch_html`] which additionally detects challenge pages.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Blocked`] on 403/429, [`Error::Timeout`] on deadline
    /// expiry, [`Error::HttpStatus`] for other non-success statuses, and
    /// [`Error::Transport`] for transport failures.
    pub async fn fetch(
        &self,
        url: &str,
        user_agent: &str,
        opts: &EngineOptions,
        accept: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<String, Error> {
        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, user_agent)
            .header(
                reqwest::header::ACCEPT_LANGUAGE,
                opts.language.as_deref().unwrap_or("en-US,en;q=0.9"),
            )
            .header(reqwest::header::ACCEPT, accept)
            .timeout(Duration::from_millis(opts.timeout_ms));
        for (key, value) in extra_headers {
            request = request.header(*key, *value);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(e) if e.is_timeout() => return Err(Error::Timeout(opts.timeout_ms)),
            Err(e) => return Err(Error::Transport(e)),
        };

        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN
            || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        {
            return Err(Error::Blocked);
        }
        if !status.is_success() {
            return Err(Error::HttpStatus(status.as_u16()));
        }

        response.text().await.map_err(Error::Transport)
    }

    /// Fetch a URL as HTML with a per-request timeout and challenge detection.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Blocked`] on 403/429 or a challenge body,
    /// [`Error::Timeout`] on deadline expiry, [`Error::HttpStatus`] for other
    /// non-success statuses, and [`Error::Transport`] for transport failures.
    pub async fn fetch_html(
        &self,
        url: &str,
        user_agent: &str,
        opts: &EngineOptions,
    ) -> Result<String, Error> {
        let body = self
            .fetch(
                url,
                user_agent,
                opts,
                "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
                &[],
            )
            .await?;
        if looks_blocked(&body) {
            return Err(Error::Blocked);
        }
        Ok(body)
    }
}

/// Detect anti-bot challenge pages by body markers.
fn looks_blocked(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    BLOCK_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_markers_are_detected() {
        assert!(looks_blocked("<html>Just a moment...</html>"));
        assert!(looks_blocked("cf-challenge detected"));
        assert!(looks_blocked("Please verify you are human"));
    }

    #[test]
    fn clean_body_is_not_blocked() {
        assert!(!looks_blocked(
            "<html><body>Search results here</body></html>"
        ));
        assert!(!looks_blocked(""));
    }

    #[test]
    fn user_agents_rotate_and_wrap() {
        let client = Client::new().expect("client builds");
        let first = client.next_user_agent();
        let second = client.next_user_agent();
        assert_ne!(first, second);
        for _ in 0..USER_AGENTS.len() - 2 {
            let _ = client.next_user_agent();
        }
        assert_eq!(client.next_user_agent(), first);
    }
}
