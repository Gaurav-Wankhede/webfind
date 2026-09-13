use std::fmt;
use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use governor::clock::DefaultClock;
use governor::state::keyed::DefaultKeyedStateStore;
use governor::{Quota, RateLimiter};
use reqwest::{Client, Proxy};

use super::device_profile::{DeviceProfile, SessionManager};
use super::fingerprint::{FingerprintAuditLog, FingerprintGenerator, fingerprint_headers};
use super::proxy_pool::ProxyPool;
use super::util;

/// Human-like HTTP client: rotating User-Agent, privacy fingerprints, proxy pool,
/// per-domain pacing, and consistent device fingerprints per domain/session.
pub struct HumanClient {
    client: Client,
    rate_limiter: Option<RateLimiter<String, DefaultKeyedStateStore<String>, DefaultClock>>,
    proxy_pool: Option<ProxyPool>,
    session_manager: Option<SessionManager>,
    fingerprint_generator: Option<FingerprintGenerator>,
    audit_log: Option<Arc<dyn FingerprintAuditLog>>,
    rotate_ua: bool,
    default_profile: Option<DeviceProfile>,
}

/// Errors from the human client.
#[derive(Debug)]
pub enum HumanClientError {
    RateLimitBlocked(String),
    NoProxyAvailable,
    Request(reqwest::Error),
}

impl fmt::Display for HumanClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            HumanClientError::RateLimitBlocked(domain) => {
                write!(f, "rate limit blocked for domain {}", domain)
            }
            HumanClientError::NoProxyAvailable => write!(f, "no proxy available in pool"),
            HumanClientError::Request(e) => write!(f, "request error: {}", e),
        }
    }
}

impl std::error::Error for HumanClientError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            HumanClientError::Request(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for HumanClientError {
    fn from(e: reqwest::Error) -> Self {
        HumanClientError::Request(e)
    }
}

impl HumanClient {
    /// Build a default human-like client with reasonable politeness:
    /// 1 req/s per domain, burst of 3, realistic browser UA.
    pub fn new() -> Result<Self> {
        Self::builder().build()
    }

    /// Start a builder.
    pub fn builder() -> HumanClientBuilder {
        HumanClientBuilder::default()
    }

    /// Inject an audit sink after construction.
    pub fn set_audit_log(&mut self, audit_log: Arc<dyn FingerprintAuditLog>) {
        self.audit_log = Some(audit_log);
    }

    /// Perform a GET request with rotation and rate limiting applied.
    pub async fn get(&self, url: &str) -> Result<reqwest::Response, HumanClientError> {
        let domain = util::extract_domain(url).unwrap_or_else(|| "unknown".to_string());

        if let Some(ref limiter) = self.rate_limiter {
            limiter.until_key_ready(&domain).await;
        }

        // Select proxy for this domain/session.
        let proxy_url =
            self.proxy_pool
                .as_ref()
                .and_then(|pool| match pool.select(Some(&domain)) {
                    Ok(Some(ep)) => Some(ep.url),
                    Ok(None) => None,
                    Err(e) => {
                        tracing::warn!("proxy select error for domain {}: {}", domain, e);
                        None
                    }
                });

        // Select or reuse device profile for this domain.
        let profile = if let Some(ref mgr) = self.session_manager {
            let session = mgr.session_for(&domain, proxy_url.clone());
            session.profile
        } else if self.rotate_ua {
            DeviceProfile::random()
        } else {
            self.default_profile
                .clone()
                .unwrap_or_else(DeviceProfile::random)
        };

        // Generate a fresh privacy fingerprint per request when enabled.
        // CPU-bound work is moved off the async reactor.
        let fingerprint = if let Some(generator) = self.fingerprint_generator.as_ref() {
            tokio::task::spawn_blocking({
                let generator = generator.clone();
                move || generator.generate_for_tool("request").ok()
            })
            .await
            .unwrap_or_default()
        } else {
            None
        };

        // Build a client for this specific proxy if one was selected.
        let client = if let Some(ref proxy_str) = proxy_url {
            match Proxy::all(proxy_str) {
                Ok(proxy) => Client::builder()
                    .proxy(proxy)
                    .timeout(Duration::from_secs(30))
                    .connect_timeout(Duration::from_secs(10))
                    .gzip(true)
                    .build()
                    .map_err(HumanClientError::Request)?,
                Err(e) => return Err(HumanClientError::Request(e)),
            }
        } else {
            self.client.clone()
        };

        let req = client.get(url);
        let req = if let Some(ref fp) = fingerprint {
            let headers = fingerprint_headers(fp);
            let mut builder = headers
                .iter()
                .fold(req, |b, (key, value)| b.header(key.clone(), value.clone()));
            builder = builder
                .header("sec-fetch-dest", "document")
                .header("sec-fetch-mode", "navigate")
                .header("sec-fetch-site", "none")
                .header("upgrade-insecure-requests", "1")
                .header("accept-encoding", "gzip, deflate, br");
            builder
        } else {
            profile.apply_headers(req)
        };
        let resp = req.send().await?;

        // Track success/failure in proxy pool.
        if let Some(ref pool) = self.proxy_pool
            && let Some(ref pid) = proxy_url {
                let result = if resp.status().is_success() {
                    pool.report_success(pid, 0)
                } else if resp.status().as_u16() == 403 || resp.status().as_u16() == 429 {
                    pool.report_banned(pid)
                } else {
                    pool.report_failure(pid)
                };
                if let Err(e) = result {
                    tracing::warn!("failed to report proxy status: {e}");
                }
            }

        // Track fingerprint health.
        let status_code = resp.status().as_u16();
        let is_success = resp.status().is_success();
        if let Some(ref fp) = fingerprint {
            if let Some(ref generator) = self.fingerprint_generator {
                if is_success {
                    generator.report_success(fp);
                } else {
                    generator.report_failure(fp, &format!("HTTP {status_code}"));
                }
            }
            if let Some(ref audit) = self.audit_log {
                audit
                    .log_fingerprint_use(
                        fp,
                        Some(status_code),
                        if is_success {
                            None
                        } else {
                            Some("non-success status")
                        },
                    )
                    .await;
            }
        }

        Ok(resp)
    }
}

/// Builder for `HumanClient`.
pub struct HumanClientBuilder {
    timeout: Duration,
    requests_per_second: Option<NonZeroU32>,
    burst_size: u32,
    proxy_pool: Option<ProxyPool>,
    session_manager: Option<SessionManager>,
    fingerprint_generator: Option<FingerprintGenerator>,
    audit_log: Option<Arc<dyn FingerprintAuditLog>>,
    rotate_ua: bool,
    default_profile: Option<DeviceProfile>,
}

impl HumanClientBuilder {
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set per-domain request rate.
    pub fn requests_per_second(mut self, rps: NonZeroU32) -> Self {
        self.requests_per_second = Some(rps);
        self
    }

    /// Set burst size for the token bucket.
    pub fn burst_size(mut self, burst: u32) -> Self {
        self.burst_size = burst;
        self
    }

    /// Provide a proxy pool for rotation.
    pub fn proxy_pool(mut self, pool: ProxyPool) -> Self {
        self.proxy_pool = Some(pool);
        self
    }

    /// Provide a session manager for sticky domain+device sessions.
    pub fn session_manager(mut self, mgr: SessionManager) -> Self {
        self.session_manager = Some(mgr);
        self
    }

    /// Provide a fingerprint generator for privacy (random residential IP + matching UA per request).
    pub fn fingerprint_generator(mut self, generator: FingerprintGenerator) -> Self {
        self.fingerprint_generator = Some(generator);
        self
    }

    /// Attach an audit sink that records every generated fingerprint use.
    pub fn audit_log(mut self, audit: Arc<dyn FingerprintAuditLog>) -> Self {
        self.audit_log = Some(audit);
        self
    }

    /// Whether to rotate User-Agent per request.
    pub fn rotate_ua(mut self, rotate: bool) -> Self {
        self.rotate_ua = rotate;
        self
    }

    /// Pin every request to a specific device profile.
    pub fn default_profile(mut self, profile: DeviceProfile) -> Self {
        self.default_profile = Some(profile);
        self
    }

    pub fn build(self) -> Result<HumanClient> {
        let client_builder = Client::builder()
            .timeout(self.timeout)
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(90))
            .gzip(true)
            .http2_prior_knowledge()
            .danger_accept_invalid_certs(false);

        let client = client_builder
            .build()
            .context("failed to build human client")?;

        let rate_limiter = self.requests_per_second.map(|rps| {
            let burst =
                NonZeroU32::new(self.burst_size.max(1)).expect("burst_size.max(1) is always >= 1");
            let quota = Quota::per_second(rps).allow_burst(burst);
            RateLimiter::keyed(quota)
        });

        Ok(HumanClient {
            client,
            rate_limiter,
            proxy_pool: self.proxy_pool,
            session_manager: self.session_manager,
            fingerprint_generator: self.fingerprint_generator,
            audit_log: self.audit_log,
            rotate_ua: self.rotate_ua,
            default_profile: self.default_profile,
        })
    }
}

impl Default for HumanClientBuilder {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            requests_per_second: NonZeroU32::new(1),
            burst_size: 3,
            proxy_pool: None,
            session_manager: None,
            fingerprint_generator: None,
            audit_log: None,
            rotate_ua: true,
            default_profile: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::util::extract_domain;

    #[test]
    fn test_build_default_client() {
        let client = HumanClient::new().unwrap();
        assert!(client.rotate_ua);
        assert!(client.rate_limiter.is_some());
    }

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            extract_domain("https://blog.example.com/post"),
            Some("blog.example.com".to_string())
        );
    }
}
