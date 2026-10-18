use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

use anyhow::{Context, Result};

/// Hex nibble lookup table for fast byte → hex encoding.


/// Canonical outbound User-Agent for every WebFind HTTP client.
///
/// Derived from `CARGO_PKG_VERSION` so it can never drift from the crate
/// version again. Before this constant existed the version was hardcoded in
/// six places and had drifted to three different values (`0.3.2`, `0.3.4`,
/// and the real `0.5.0`), which made server-side logs misattribute traffic.
///
/// Engines that deliberately spoof a browser UA (e.g. Semantic Scholar) keep
/// their own constant — that is intentional, not drift.
pub const USER_AGENT: &str = concat!(
    "webfind/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Gaurav-Wankhede/webfind)"
);

/// Generate a stable 32-hex-char ID from a URL using BLAKE3.
///
/// Used by TursoStore, search engines, and background workers as the
/// canonical URL → primary-key mapping. BLAKE3 is 3-5× faster than SHA-256
/// on small inputs (URLs are typically <200 bytes) and produces the same
/// 32-byte output. We truncate to the first 16 bytes (128 bits) for a
/// 32-hex-char identifier — collision probability is negligible for
/// web-scale datasets (<100M URLs).
///
/// The ID is always exactly 32 hex chars (128 bits). It is **not** a
/// 16-char ID: 16 bytes encode as 32 hex digits, and `{:016x}` on a `u128`
/// is a *minimum* width that would otherwise yield a variable-length string.
///
/// # Examples
///
/// ```
/// use webfind::engine::util::url_id;
/// let id = url_id("https://example.com/path");
/// assert_eq!(id.len(), 32); // always 32 hex chars (16 bytes)
/// assert_eq!(id, url_id("https://example.com/path")); // deterministic
/// assert_ne!(id, url_id("https://other.com/")); // different URL → different ID
/// ```
pub fn url_id(url: &str) -> String {
    let hash = blake3::hash(url.as_bytes());
    let digest = hash.as_bytes();

    // Manual hex-encode of the first 16 bytes via a lookup table into a fixed
    // 32-byte buffer, then a single validated UTF-8 conversion. This avoids the
    // `format!("{:032x}", u128)` formatting machinery, which dominates the cost
    // on short URL inputs (it is the shared step that dilutes BLAKE3's hashing
    // advantage — see PRD §7.5).
    hex::encode(&digest[..16])
}

/// Extract the registrable domain from a URL.
/// Returns lowercase host string with any leading `www.` stripped, or `None` on invalid URLs.
pub fn extract_domain(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed.host_str().map(|h| {
        let h = h.to_lowercase();
        h.strip_prefix("www.").map(String::from).unwrap_or(h)
    })
}

/// Extract the canonical registrable root domain (eTLD+1) using the Public Suffix List crate.
pub fn extract_root_domain(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    let host = parsed.host_str()?.to_lowercase();
    let host_clean = host.strip_prefix("www.").unwrap_or(&host);
    if let Some(domain) = psl::domain(host_clean.as_bytes()) {
        std::str::from_utf8(domain.as_bytes()).ok().map(String::from)
    } else {
        Some(host_clean.to_string())
    }
}

/// Parse a CIDR notation string `"x.x.x.x/y"` into `(Ipv4Addr, prefix)`.
pub fn parse_cidr(cidr: &str) -> Result<(Ipv4Addr, u8)> {
    let parts: Vec<&str> = cidr.split('/').collect();
    if parts.len() != 2 {
        anyhow::bail!("CIDR must be in form x.x.x.x/y");
    }
    let addr: Ipv4Addr = parts[0].parse().context("invalid IPv4 address")?;
    let prefix: u8 = parts[1].parse().context("invalid prefix length")?;
    if prefix > 32 {
        anyhow::bail!("prefix must be <= 32");
    }
    Ok((addr, prefix))
}

/// Split a comma-separated string into trimmed, non-empty strings.
pub fn split_comma(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parse a comma-separated proxy URL string into a `ProxyPool`.
pub fn parse_proxy_pool(
    proxies_str: &str,
    pool: &crate::engine::proxy_pool::ProxyPool,
) -> Result<()> {
    for url in proxies_str.split(',') {
        let url = url.trim().to_string();
        if !url.is_empty() {
            pool.add(crate::engine::proxy_pool::ProxyEndpoint::from_url(&url)?)?;
        }
    }
    Ok(())
}

/// Extract top keywords from text by frequency, filtering out stop words.
pub fn extract_top_words(text: &str, top_n: usize) -> Vec<String> {
    let stop_words: HashSet<String> = stop_words::get(stop_words::Language::English)
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let mut counts: HashMap<String, usize> = HashMap::new();
    for word in text.split(|c: char| !c.is_alphanumeric()) {
        let w = word.to_lowercase();
        if w.len() < 3 || stop_words.contains(&w) {
            continue;
        }
        *counts.entry(w).or_insert(0) += 1;
    }

    let mut pairs: Vec<(String, usize)> = counts.into_iter().collect();
    pairs.sort_by_key(|a| std::cmp::Reverse(a.1));
    pairs.into_iter().take(top_n).map(|(w, _)| w).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_domain() {
        assert_eq!(
            extract_domain("https://example.com/path"),
            Some("example.com".into())
        );
        assert_eq!(
            extract_domain("http://Sub.Example.COM/page"),
            Some("sub.example.com".into())
        );
        assert_eq!(extract_domain("not-a-url"), None);
    }

    #[test]
    fn test_parse_cidr() {
        let (addr, prefix) = parse_cidr("10.0.0.0/24").unwrap();
        assert_eq!(addr, Ipv4Addr::new(10, 0, 0, 0));
        assert_eq!(prefix, 24);
        assert!(parse_cidr("invalid").is_err());
    }
}
