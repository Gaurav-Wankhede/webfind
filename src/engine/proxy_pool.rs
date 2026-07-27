use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rand::RngExt;
use rand::seq::IndexedRandom;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use super::util;

/// A single proxy endpoint with metadata and health state.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyEndpoint {
    pub id: String,
    /// Proxy URL: http://host:port, https://host:port, socks5://host:port
    pub url: String,
    pub protocol: ProxyProtocol,
    pub source: ProxySource,
    pub country: Option<String>,
    pub asn: Option<String>,
    pub proxy_type: ProxyType,
    pub added_at: chrono::DateTime<chrono::Utc>,

    #[serde(skip)]
    pub health: Arc<RwLock<ProxyHealth>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProxyProtocol {
    Http,
    Https,
    Socks5,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProxyType {
    Datacenter,
    Residential,
    Isp,
    Mobile,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ProxySource {
    StaticFile,
    SubscriptionUrl,
    Api,
    Manual,
    Generated,
}

#[derive(Debug, Clone, Default)]
pub struct ProxyHealth {
    pub last_checked: Option<Instant>,
    pub last_success: Option<Instant>,
    pub latency_ms: Option<u64>,
    pub consecutive_failures: u32,
    pub total_requests: u64,
    pub total_failures: u64,
    pub banned: bool,
    pub cooldown_until: Option<Instant>,
}

impl ProxyEndpoint {
    pub fn from_url(url: &str) -> Result<Self> {
        let parsed = reqwest::Url::parse(url).context("invalid proxy URL")?;
        let protocol = match parsed.scheme() {
            "http" => ProxyProtocol::Http,
            "https" => ProxyProtocol::Https,
            "socks5" => ProxyProtocol::Socks5,
            other => anyhow::bail!("unsupported proxy protocol: {}", other),
        };
        let id = format!("{}://{}", parsed.scheme(), parsed.host_str().unwrap_or(""));
        Ok(Self {
            id,
            url: url.to_string(),
            protocol,
            source: ProxySource::Manual,
            country: None,
            asn: None,
            proxy_type: ProxyType::Datacenter,
            added_at: chrono::Utc::now(),
            health: Arc::new(RwLock::new(ProxyHealth::default())),
        })
    }

    /// Generate a proxy endpoint from a random IP within a CIDR block.
    ///
    /// This is useful when you operate your own proxy fleet or residential subnet:
    /// each request gets a different IP from the configured range.
    pub fn from_random_ip(cidr: &str, port: u16, protocol: ProxyProtocol) -> Result<Self> {
        let (network, prefix) = util::parse_cidr(cidr)?;
        let host_count = if prefix >= 31 {
            1
        } else {
            (1u32 << (32 - prefix)) - 2
        };
        if host_count == 0 {
            anyhow::bail!("CIDR {} has no usable host IPs", cidr);
        }

        let mut rng = rand::rng();
        let offset = rng.random_range(1..=host_count);
        let ip = u32::from(network) + offset;
        let addr = Ipv4Addr::from(ip);

        let scheme = match protocol {
            ProxyProtocol::Http => "http",
            ProxyProtocol::Https => "https",
            ProxyProtocol::Socks5 => "socks5",
        };
        let url = format!("{}://{}:{}", scheme, addr, port);
        let id = url.clone();

        Ok(Self {
            id,
            url,
            protocol,
            source: ProxySource::Generated,
            country: None,
            asn: None,
            proxy_type: ProxyType::Datacenter,
            added_at: chrono::Utc::now(),
            health: Arc::new(RwLock::new(ProxyHealth::default())),
        })
    }

    /// True if the proxy is currently available (not banned or in cooldown).
    pub fn is_available(&self) -> bool {
        let h = self.health.read().unwrap();
        if h.banned {
            return false;
        }
        if let Some(until) = h.cooldown_until {
            if until > Instant::now() {
                return false;
            }
        }
        h.consecutive_failures < 3
    }

    pub fn score(&self) -> f64 {
        if !self.is_available() {
            return 0.0;
        }
        let h = self.health.read().unwrap();
        let base = match self.proxy_type {
            ProxyType::Residential => 1.0,
            ProxyType::Isp => 0.9,
            ProxyType::Mobile => 0.85,
            ProxyType::Datacenter => 0.6,
        };
        let latency_penalty = h
            .latency_ms
            .map(|ms| (ms as f64 / 1000.0).min(0.5))
            .unwrap_or(0.0);
        let failure_penalty = (h.consecutive_failures as f64 * 0.15).min(0.5);
        (base - latency_penalty - failure_penalty).max(0.05)
    }
}

/// Strategy for picking the next proxy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RotationStrategy {
    /// Pick randomly from available pool.
    #[default]
    Random,
    /// Round-robin across all available proxies.
    RoundRobin,
    /// Weighted by health score (residential > ISP > datacenter).
    Weighted,
    /// Consistent hashing on a session key (domain/account).
    Sticky,
}

/// A managed pool of proxy endpoints.
#[derive(Debug)]
pub struct ProxyPool {
    endpoints: RwLock<Vec<ProxyEndpoint>>,
    strategy: RotationStrategy,
    round_robin_counter: AtomicUsize,
    health_check_url: String,
    max_failures: u32,
    cooldown: Duration,
}

impl ProxyPool {
    /// Create an empty pool.
    pub fn new() -> Self {
        Self {
            endpoints: RwLock::new(Vec::new()),
            strategy: RotationStrategy::default(),
            round_robin_counter: AtomicUsize::new(0),
            health_check_url: "https://httpbin.org/ip".to_string(),
            max_failures: 3,
            cooldown: Duration::from_secs(300),
        }
    }

    pub fn with_strategy(mut self, strategy: RotationStrategy) -> Self {
        self.strategy = strategy;
        self
    }

    pub fn with_health_check(mut self, url: impl Into<String>) -> Self {
        self.health_check_url = url.into();
        self
    }

    /// Load proxies from a comma-separated list.
    pub fn from_list(list: &[String]) -> Result<Self> {
        let pool = Self::new();
        for url in list {
            pool.add(ProxyEndpoint::from_url(url)?);
        }
        Ok(pool)
    }

    /// Generate a pool of random proxy endpoints from a CIDR block.
    ///
    /// Use this when you control a subnet of proxy IPs and want each request to
    /// exit from a random address inside that subnet. Duplicate random IPs are
    /// deduplicated, so the resulting pool may be smaller than `count` if the
    /// CIDR is small.
    pub fn from_cidr(cidr: &str, port: u16, protocol: ProxyProtocol, count: usize) -> Result<Self> {
        let pool = Self::new();
        let attempts = (count * 10).max(1);
        for _ in 0..attempts {
            if pool.len() >= count {
                break;
            }
            pool.add(ProxyEndpoint::from_random_ip(cidr, port, protocol)?);
        }
        Ok(pool)
    }

    pub fn add(&self, endpoint: ProxyEndpoint) {
        let mut eps = self.endpoints.write().unwrap();
        // Replace existing by ID.
        if let Some(idx) = eps.iter().position(|e| e.id == endpoint.id) {
            eps[idx] = endpoint;
        } else {
            eps.push(endpoint);
        }
    }

    /// Return a snapshot of all endpoints.
    pub fn endpoints(&self) -> Vec<ProxyEndpoint> {
        self.endpoints.read().unwrap().clone()
    }

    pub fn len(&self) -> usize {
        self.endpoints.read().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Select the best proxy for the given session key (domain or account).
    pub fn select(&self, session_key: Option<&str>) -> Option<ProxyEndpoint> {
        let eps = self.endpoints.read().unwrap();
        let available: Vec<&ProxyEndpoint> = eps.iter().filter(|e| e.is_available()).collect();
        if available.is_empty() {
            return None;
        }

        match self.strategy {
            RotationStrategy::Random => available.choose(&mut rand::rng()).map(|e| (*e).clone()),
            RotationStrategy::RoundRobin => {
                let idx =
                    self.round_robin_counter.fetch_add(1, Ordering::Relaxed) % available.len();
                Some(available[idx].clone())
            }
            RotationStrategy::Weighted => {
                let total: f64 = available.iter().map(|e| e.score()).sum();
                if total <= 0.0 {
                    return available.choose(&mut rand::rng()).map(|e| (*e).clone());
                }
                let mut pick = rand::rng().random::<f64>() * total;
                for ep in &available {
                    pick -= ep.score();
                    if pick <= 0.0 {
                        return Some((*ep).clone());
                    }
                }
                available.last().map(|e| (*e).clone())
            }
            RotationStrategy::Sticky => {
                let key = session_key.unwrap_or("default");
                let idx = stable_hash(key) as usize % available.len();
                Some(available[idx].clone())
            }
        }
    }

    /// Returns true if the pool generated from CIDR should regenerate dead proxies.
    pub fn has_generated_endpoints(&self) -> bool {
        self.endpoints
            .read()
            .unwrap()
            .iter()
            .any(|e| e.source == ProxySource::Generated)
    }

    /// Mark a proxy as failed; cooldown after max_failures consecutive failures.
    pub fn report_failure(&self, endpoint_id: &str) {
        let eps = self.endpoints.read().unwrap();
        if let Some(ep) = eps.iter().find(|e| e.id == endpoint_id) {
            let mut h = ep.health.write().unwrap();
            h.consecutive_failures += 1;
            h.total_failures += 1;
            if h.consecutive_failures >= self.max_failures {
                h.cooldown_until = Some(Instant::now() + self.cooldown);
                warn!(
                    "proxy {} entered cooldown after {} failures",
                    endpoint_id, h.consecutive_failures
                );
            }
        }
    }

    pub fn report_success(&self, endpoint_id: &str, latency_ms: u64) {
        let eps = self.endpoints.read().unwrap();
        if let Some(ep) = eps.iter().find(|e| e.id == endpoint_id) {
            let mut h = ep.health.write().unwrap();
            h.consecutive_failures = 0;
            h.total_requests += 1;
            h.last_success = Some(Instant::now());
            h.latency_ms = Some(latency_ms);
        }
    }

    /// Mark a proxy as permanently banned (e.g. captcha page returned).
    pub fn report_banned(&self, endpoint_id: &str) {
        let eps = self.endpoints.read().unwrap();
        if let Some(ep) = eps.iter().find(|e| e.id == endpoint_id) {
            let mut h = ep.health.write().unwrap();
            h.banned = true;
            warn!("proxy {} marked banned", endpoint_id);
        }
    }

    /// Async health check of all proxies using a lightweight HTTP request.
    pub async fn health_check_all(&self) {
        use reqwest::Client;

        let eps = {
            let guard = self.endpoints.read().unwrap();
            guard.clone()
        };

        for ep in eps {
            let start = Instant::now();
            let proxy = match reqwest::Proxy::all(&ep.url) {
                Ok(p) => p,
                Err(_) => {
                    self.report_failure(&ep.id);
                    continue;
                }
            };
            let client = match Client::builder()
                .proxy(proxy)
                .timeout(Duration::from_secs(10))
                .build()
            {
                Ok(c) => c,
                Err(_) => {
                    self.report_failure(&ep.id);
                    continue;
                }
            };
            match client.get(&self.health_check_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    let ms = start.elapsed().as_millis() as u64;
                    self.report_success(&ep.id, ms);
                    debug!("proxy {} healthy ({} ms)", ep.id, ms);
                }
                _ => {
                    self.report_failure(&ep.id);
                }
            }
        }
    }
}

impl Default for ProxyPool {
    fn default() -> Self {
        Self::new()
    }
}

/// A stable 64-bit hash for sticky session selection.
fn stable_hash(s: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    s.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_round_robin() {
        let pool = ProxyPool::new().with_strategy(RotationStrategy::RoundRobin);
        pool.add(ProxyEndpoint::from_url("http://a:8080").unwrap());
        pool.add(ProxyEndpoint::from_url("http://b:8080").unwrap());

        let first = pool.select(None).unwrap();
        let second = pool.select(None).unwrap();
        assert_ne!(first.id, second.id);
    }

    #[test]
    fn test_pool_sticky_same_key() {
        let pool = ProxyPool::new().with_strategy(RotationStrategy::Sticky);
        pool.add(ProxyEndpoint::from_url("http://a:8080").unwrap());
        pool.add(ProxyEndpoint::from_url("http://b:8080").unwrap());

        let a1 = pool.select(Some("example.com")).unwrap();
        let a2 = pool.select(Some("example.com")).unwrap();
        assert_eq!(a1.id, a2.id);
    }

    #[test]
    fn test_failure_cooldown() {
        let pool = ProxyPool::new().with_strategy(RotationStrategy::Random);
        pool.add(ProxyEndpoint::from_url("http://bad:8080").unwrap());

        for _ in 0..3 {
            pool.report_failure("http://bad");
        }
        assert!(pool.select(None).is_none());
    }

    #[test]
    fn test_cidr_generation() {
        let pool = ProxyPool::from_cidr("10.0.0.0/24", 8080, ProxyProtocol::Http, 5).unwrap();
        assert_eq!(pool.len(), 5);
        let ep = pool.select(None).unwrap();
        assert!(ep.url.starts_with("http://10.0.0."));
        assert!(ep.url.ends_with(":8080"));
    }
}
