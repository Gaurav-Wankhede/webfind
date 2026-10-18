//! Shared HTTP client for engine adapters: user-agent rotation, per-request
//! timeouts, and anti-bot challenge detection.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use super::{EngineOptions, Error};

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

/// Shared, cloneable HTTP client with dynamic user-agent generation.
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
            .timeout(Duration::from_secs(12))
            .connect_timeout(Duration::from_secs(3))
            .tcp_nodelay(true)
            .pool_max_idle_per_host(10)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .map_err(Error::Transport)?;
        Ok(Self {
            http,
            ua_counter: Arc::new(AtomicUsize::new(0)),
        })
    }

    /// Return the next user agent generated dynamically using `ua_generator`.
    #[must_use]
    pub fn next_user_agent(&self) -> String {
        let _ = self.ua_counter.fetch_add(1, Ordering::Relaxed);
        ua_generator::ua::spoof_ua().to_string()
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
        user_agent: impl AsRef<str>,
        opts: &EngineOptions,
        accept: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<String, Error> {
        let user_agent = user_agent.as_ref();
        let (sec_ch_ua, sec_ch_ua_platform) = if user_agent.contains("Chrome") {
            (
                r#""Not/A)Brand";v="8", "Chromium";v="126", "Google Chrome";v="126""#,
                if user_agent.contains("Macintosh") {
                    r#""macOS""#
                } else if user_agent.contains("Windows") {
                    r#""Windows""#
                } else {
                    r#""Linux""#
                },
            )
        } else {
            ("", "")
        };

        let mut request = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, user_agent)
            .header(
                reqwest::header::ACCEPT_LANGUAGE,
                opts.language.as_deref().unwrap_or("en-US,en;q=0.9"),
            )
            .header(reqwest::header::ACCEPT, accept)
            .header(
                reqwest::header::UPGRADE_INSECURE_REQUESTS,
                "1",
            )
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-site", "none")
            .header("sec-fetch-user", "?1")
            .timeout(Duration::from_millis(opts.timeout_ms));

        if !sec_ch_ua.is_empty() {
            request = request
                .header("sec-ch-ua", sec_ch_ua)
                .header("sec-ch-ua-mobile", "?0")
                .header("sec-ch-ua-platform", sec_ch_ua_platform);
        }

        for (key, value) in extra_headers {
            request = request.header(*key, *value);
        }

        // Retry-with-fresh-identity on anti-bot block: a 403/429 from one request
        // often clears on a second request with a rotated user-agent and a short
        // randomized backoff (engines throttle by IP+UA fingerprint, not just IP).
        // The first attempt uses the caller's UA; a blocked retry rotates to the
        // next UA and jitters its delay so a burst of concurrent engine requests
        // do not all retry in lockstep. Source: proxy/UA rotation best practice.
        let mut retry_ua = user_agent.to_string();
        let max_attempts = 3u32;
        let mut last_response = None;
        for attempt in 0..max_attempts {
            let mut attempt_req = self
                .http
                .get(url)
                .header(reqwest::header::USER_AGENT, &retry_ua)
                .header(
                    reqwest::header::ACCEPT_LANGUAGE,
                    opts.language.as_deref().unwrap_or("en-US,en;q=0.9"),
                )
                .header(reqwest::header::ACCEPT, accept)
                .header(reqwest::header::UPGRADE_INSECURE_REQUESTS, "1")
                .header("sec-fetch-dest", "document")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-site", "none")
                .header("sec-fetch-user", "?1")
                .timeout(Duration::from_millis(opts.timeout_ms));
            if !sec_ch_ua.is_empty() {
                attempt_req = attempt_req
                    .header("sec-ch-ua", sec_ch_ua)
                    .header("sec-ch-ua-mobile", "?0")
                    .header("sec-ch-ua-platform", sec_ch_ua_platform);
            }
            for (key, value) in extra_headers {
                attempt_req = attempt_req.header(*key, *value);
            }

            let response = match attempt_req.send().await {
                Ok(response) => response,
                Err(e) if e.is_timeout() => return Err(Error::Timeout(opts.timeout_ms)),
                Err(e) => return Err(Error::Transport(e)),
            };

            let status = response.status();
            if status == reqwest::StatusCode::FORBIDDEN
                || status == reqwest::StatusCode::TOO_MANY_REQUESTS
            {
                last_response = Some(response);
                if attempt + 1 < max_attempts {
                    // Rotate identity and back off with exponential jitter before retrying.
                    retry_ua = self.next_user_agent();
                    let exp_ms = 200u64.saturating_mul(2u64.pow(attempt));
                    let jitter = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.subsec_nanos() as u64)
                        .unwrap_or(0))
                        % 350;
                    tokio::time::sleep(Duration::from_millis(exp_ms + jitter)).await;
                    continue;
                }
                return Err(Error::Blocked);
            }
            if !status.is_success() {
                return Err(Error::HttpStatus(status.as_u16()));
            }

            return response.text().await.map_err(Error::Transport);
        }
        // Unreachable: the loop either returns or continues; if it exits, treat as blocked.
        if last_response.is_some() {
            return Err(Error::Blocked);
        }
        Err(Error::Blocked)
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
        user_agent: impl AsRef<str>,
        opts: &EngineOptions,
    ) -> Result<String, Error> {
        let body = self
            .fetch(
                url,
                user_agent.as_ref(),
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
        assert!(!first.is_empty());
        assert!(!second.is_empty());
        assert!(first.contains("Mozilla") || first.contains("AppleWebKit") || first.contains("Gecko"));
    }
}
