use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;

use anyhow::{Context, Result};

/// Extract the registrable domain from a URL.
/// Returns lowercase host string with any leading `www.` stripped, or `None` on invalid URLs.
pub fn extract_domain(url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(url).ok()?;
    parsed.host_str().map(|h| {
        let h = h.to_lowercase();
        h.strip_prefix("www.").map(String::from).unwrap_or(h)
    })
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

/// Parse a comma-separated proxy URL string into a `ProxyPool`.
pub fn parse_proxy_pool(
    proxies_str: &str,
    pool: &crate::engine::proxy_pool::ProxyPool,
) -> Result<()> {
    for url in proxies_str.split(',') {
        let url = url.trim().to_string();
        if !url.is_empty() {
            pool.add(
                crate::engine::proxy_pool::ProxyEndpoint::from_url(&url)?,
            );
        }
    }
    Ok(())
}

/// Extract top keywords from text by frequency, filtering out stop words.
pub fn extract_top_words(text: &str, top_n: usize) -> Vec<String> {
    let stop_words: HashSet<String> = stop_words::get(stop_words::LANGUAGE::English)
        .into_iter()
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
    pairs.sort_by(|a, b| b.1.cmp(&a.1));
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
