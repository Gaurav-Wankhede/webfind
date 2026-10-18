pub(crate) fn extract_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}

pub(crate) fn title_from_url(url: &str) -> String {
    let path = url
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .split('?')
        .next()
        .unwrap_or(url);
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .map(percent_decode)
        .unwrap_or_else(|| extract_domain(url))
        .trim()
        .to_string()
}

/// Decode common percent-encoded characters in a URL fragment.
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let a = chars.next();
            let b = chars.next();
            if let (Some(a), Some(b)) = (a, b)
                && let Ok(byte) = u8::from_str_radix(&format!("{}{}", a, b), 16) {
                    out.push(byte as char);
                    continue;
                }
        }
        out.push(c);
    }
    out
}

/// Derive a site name from the registered domain, or fall back to the full host.
pub(crate) fn extract_site_name(url: &str) -> Option<String> {
    let host = extract_domain(url);
    if host.is_empty() {
        return None;
    }
    // Strip a leading "www." again and return the domain without port.
    host.split(':')
        .next()
        .map(|h| h.trim_start_matches("www.").to_string())
        .filter(|h| !h.is_empty())
}

/// Minimal HTML-escape for fallback plain text blocks.
pub(crate) fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

