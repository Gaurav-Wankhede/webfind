use std::net::{IpAddr, ToSocketAddrs};
use url::Url;

/// Security validation error when a candidate seed or URL violates network policies.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SecurityGateError {
    #[error("invalid URL format: {0}")]
    InvalidUrl(String),

    #[error("unsupported scheme '{0}': only http and https are permitted")]
    DisallowedScheme(String),

    #[error("missing host in URL: {0}")]
    MissingHost(String),

    #[error("SSRF blocked: target IP {0} is in a forbidden private/local network range")]
    ForbiddenIp(IpAddr),

    #[error("DNS resolution failed or yielded no IP addresses: {0}")]
    DnsResolutionFailed(String),
}

/// Checks whether an IP address belongs to a forbidden/private network.
///
/// Disallows:
/// - Loopback addresses (127.0.0.0/8, ::1)
/// - RFC 1918 private subnets (10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16)
/// - Link-local addresses (169.254.0.0/16, fe80::/10)
/// - Cloud metadata service (169.254.169.254)
/// - Broadcast & Unspecified (0.0.0.0, 255.255.255.255, ::)
pub fn is_forbidden_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_broadcast()
                || ipv4.is_unspecified()
                || ipv4.octets() == [169, 254, 169, 254]
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback() || ipv6.is_unspecified() || ipv6.segments()[0] & 0xffc0 == 0xfe80
        }
    }
}

/// Pre-flight security validation for a seed URL.
///
/// Verifies:
/// 1. URL parsing succeeds.
/// 2. Scheme is strictly HTTP or HTTPS.
/// 3. Host is present.
/// 4. If host is an IP literal or resolves via DNS, none of the addresses are in forbidden ranges.
pub fn validate_url_security(raw_url: &str) -> Result<Url, SecurityGateError> {
    let parsed = Url::parse(raw_url).map_err(|e| SecurityGateError::InvalidUrl(e.to_string()))?;

    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(SecurityGateError::DisallowedScheme(scheme));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| SecurityGateError::MissingHost(raw_url.to_string()))?;

    // Check if host is direct IP literal.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_forbidden_ip(ip) {
            return Err(SecurityGateError::ForbiddenIp(ip));
        }
    } else {
        // Resolve hostname to inspect resolved IPs for SSRF defense.
        let port = parsed.port_or_known_default().unwrap_or(80);
        let socket_addr_str = format!("{}:{}", host, port);
        if let Ok(addrs) = socket_addr_str.to_socket_addrs() {
            for addr in addrs {
                if is_forbidden_ip(addr.ip()) {
                    return Err(SecurityGateError::ForbiddenIp(addr.ip()));
                }
            }
        }
    }

    Ok(parsed)
}

/// Normalizes a URL by stripping tracking parameters, query clutter, and fragments.
pub fn sanitize_url(raw_url: &str) -> Result<String, SecurityGateError> {
    let mut parsed = validate_url_security(raw_url)?;

    // Remove fragments.
    parsed.set_fragment(None);

    // Strip common tracking and telemetry query parameters.
    if parsed.query().is_some() {
        let clean_pairs: Vec<(String, String)> = parsed
            .query_pairs()
            .filter(|(k, _)| {
                let key = k.to_lowercase();
                !key.starts_with("utm_")
                    && key != "fbclid"
                    && key != "gclid"
                    && key != "ref"
                    && key != "source"
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();

        if clean_pairs.is_empty() {
            parsed.set_query(None);
        } else {
            let mut serializer = url::form_urlencoded::Serializer::new(String::new());
            for (k, v) in clean_pairs {
                serializer.append_pair(&k, &v);
            }
            parsed.set_query(Some(&serializer.finish()));
        }
    }

    Ok(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disallow_non_http() {
        assert!(validate_url_security("file:///etc/passwd").is_err());
        assert!(validate_url_security("ftp://example.com").is_err());
        assert!(validate_url_security("gopher://example.com").is_err());
    }

    #[test]
    fn test_disallow_forbidden_ips() {
        assert!(validate_url_security("http://127.0.0.1:8080").is_err());
        assert!(validate_url_security("http://localhost:3000").is_err());
        assert!(validate_url_security("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_url_security("http://10.0.0.1/admin").is_err());
        assert!(validate_url_security("http://192.168.1.1").is_err());
    }

    #[test]
    fn test_allow_public_http() {
        assert!(validate_url_security("https://example.com/test").is_ok());
        assert!(validate_url_security("https://docs.rs/tokio/latest/tokio/").is_ok());
    }

    #[test]
    fn test_sanitize_tracking_params() {
        let cleaned = sanitize_url(
            "https://example.com/page?utm_source=twitter&utm_medium=cpc&id=42&fbclid=xyz#section",
        )
        .unwrap();
        assert_eq!(cleaned, "https://example.com/page?id=42");
    }
}
