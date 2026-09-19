//! URL and snippet helpers shared by engine adapters: tracker-URL decoding,
//! canonicalization for fusion dedup, date extraction from snippets, and
//! query-keyword extraction for relevance-aware fusion ranking.

use std::collections::HashSet;
use std::sync::LazyLock;

use base64::Engine;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use regex::Regex;
use url::Url;

/// Bing `/ck/a` redirect prefix: 2-char marker + URL-safe base64 destination.
const BING_TRACKER_PREFIX: usize = 2;

/// Absolute dates that appear at the start of SERP snippets, optionally
/// followed by a separator ("Jan 15, 2025 - ...", "2025-01-15 · ...").
/// Group 1 captures the date text.
static ABSOLUTE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:\w{3}\s+\d{1,2},\s+\d{4}|\d{4}-\d{2}-\d{2}))(?:\s*[·—–-])?")
        .expect("static regex is valid")
});

/// Relative dates that appear at the start of SERP snippets ("3 days ago").
static RELATIVE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d+)\s+(day|hour|minute|week|month)s?\s+ago(?:\s*[·—–-])?")
        .expect("static regex is valid")
});

/// Query parameters stripped from canonical URLs before fusion dedup.
const TRACKING_PARAMS: &[&str] = &[
    "utm_source",
    "utm_medium",
    "utm_campaign",
    "utm_term",
    "utm_content",
    "fbclid",
    "gclid",
    "mc_cid",
    "mc_eid",
];

/// Decode a Bing `/ck/a` tracker URL to its destination.
///
/// Protocol-relative hrefs (`//www.bing.com/...`) are resolved against `https:`
/// first. Returns the input unchanged when the URL is not a Bing tracker link
/// or the payload cannot be decoded.
#[must_use]
pub fn decode_bing_tracker_url(href: &str) -> String {
    let absolute = absolute_url(href);
    let Ok(parsed) = Url::parse(&absolute) else {
        return href.to_string();
    };
    let is_bing = parsed.host_str().is_some_and(|h| h.ends_with("bing.com"));
    if !is_bing || parsed.path() != "/ck/a" {
        return href.to_string();
    }
    let Some(encoded) = parsed
        .query_pairs()
        .find(|(key, _)| key == "u")
        .map(|(_, value)| value.into_owned())
    else {
        return href.to_string();
    };
    if encoded.len() < BING_TRACKER_PREFIX + 1 {
        return href.to_string();
    }
    let trimmed = &encoded[BING_TRACKER_PREFIX..];
    let Ok(decoded) = base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(trimmed) else {
        return href.to_string();
    };
    let Ok(decoded_str) = String::from_utf8(decoded) else {
        return href.to_string();
    };
    if Url::parse(&decoded_str).is_ok() {
        decoded_str
    } else {
        href.to_string()
    }
}

/// Decode a DuckDuckGo `/l/?uddg=` redirect URL to its destination.
///
/// Protocol-relative hrefs (`//duckduckgo.com/...`) are resolved against
/// `https:` first. Returns the input unchanged when the URL is not a DDG
/// redirect or the destination is not a valid URL.
#[must_use]
pub fn decode_ddg_redirect_url(href: &str) -> String {
    let absolute = absolute_url(href);
    let Ok(parsed) = Url::parse(&absolute) else {
        return href.to_string();
    };
    let is_ddg = parsed
        .host_str()
        .is_some_and(|h| h.ends_with("duckduckgo.com"));
    if !is_ddg || parsed.path() != "/l/" {
        return href.to_string();
    }
    let Some(destination) = parsed
        .query_pairs()
        .find(|(key, _)| key == "uddg")
        .map(|(_, value)| value.into_owned())
    else {
        return href.to_string();
    };
    if Url::parse(&destination).is_ok() {
        destination
    } else {
        href.to_string()
    }
}

/// Resolve a protocol-relative href (`//host/...`) against `https:`.
fn absolute_url(href: &str) -> String {
    if href.starts_with("//") {
        format!("https:{href}")
    } else {
        href.to_string()
    }
}

/// Resolve engine redirect URLs to their destinations.
///
/// Returns the input unchanged when no known redirect format matches.
#[must_use]
pub fn normalize_result_url(href: &str) -> String {
    let ddg = decode_ddg_redirect_url(href);
    if ddg != href {
        return ddg;
    }
    decode_bing_tracker_url(href)
}

/// Canonical form of a URL used as the fusion dedup key: fragment stripped,
/// tracking parameters removed, query re-encoded.
#[must_use]
pub fn canonical_url(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_string();
    };
    parsed.set_fragment(None);

    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(key, _)| !TRACKING_PARAMS.contains(&key.as_ref()))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();

    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        let mut serializer = url::form_urlencoded::Serializer::new(String::new());
        for (key, value) in &kept {
            serializer.append_pair(key, value);
        }
        parsed.set_query(Some(&serializer.finish()));
    }
    parsed.to_string()
}

/// Positional relevance of a result: 1.0 for the first result, decaying to 0.0.
#[must_use]
pub fn positional_relevance(index: usize, total: usize) -> f64 {
    if total == 0 {
        return 0.0;
    }
    let index = u32::try_from(index).unwrap_or(u32::MAX);
    let total = u32::try_from(total).unwrap_or(u32::MAX);
    1.0 - f64::from(index) / f64::from(total)
}

/// Extract a publication date from a SERP snippet prefix, if present.
///
/// Handles absolute dates ("Jan 15, 2025", "2025-01-15") and relative dates
/// ("3 days ago"), with or without a trailing separator.
#[must_use]
pub fn parse_date_from_snippet(snippet: &str) -> Option<DateTime<Utc>> {
    let trimmed = snippet.trim();
    if let Some(captures) = ABSOLUTE_DATE.captures(trimmed) {
        let date_str = captures.get(1)?.as_str();
        if let Ok(naive) = NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            return naive.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
        }
        if let Ok(naive) = NaiveDate::parse_from_str(date_str, "%b %d, %Y") {
            return naive.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc());
        }
        return None;
    }
    if let Some(captures) = RELATIVE_DATE.captures(trimmed) {
        let amount: i64 = captures.get(1)?.as_str().parse().ok()?;
        let unit = captures.get(2)?.as_str();
        let duration = match unit {
            "day" => Duration::days(amount),
            "hour" => Duration::hours(amount),
            "minute" => Duration::minutes(amount),
            "week" => Duration::weeks(amount),
            "month" => Duration::days(amount.saturating_mul(30)),
            _ => return None,
        };
        return Some(Utc::now() - duration);
    }
    None
}

/// Strip a leading date prefix (and its separator) from a snippet, leaving
/// the clean descriptive text.
#[must_use]
pub fn strip_date_prefix(snippet: &str) -> String {
    let trimmed = snippet.trim();
    let stripped = ABSOLUTE_DATE.replace(trimmed, "");
    let stripped = RELATIVE_DATE.replace(&stripped, "");
    stripped.trim().to_string()
}

/// Stop words removed from query keywords.
const STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "in",
    "on",
    "at",
    "to",
    "for",
    "of",
    "with",
    "by",
    "from",
    "as",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "must",
    "shall",
    "can",
    "need",
    "dare",
    "ought",
    "used",
    "this",
    "that",
    "these",
    "those",
    "i",
    "you",
    "he",
    "she",
    "it",
    "we",
    "they",
    "what",
    "which",
    "who",
    "when",
    "where",
    "why",
    "how",
    "all",
    "any",
    "both",
    "each",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "no",
    "nor",
    "not",
    "only",
    "own",
    "same",
    "so",
    "than",
    "too",
    "very",
    "just",
    "now",
    "then",
    "also",
    "about",
    "up",
    "out",
    "if",
    "because",
    "until",
    "while",
    "during",
    "before",
    "after",
    "above",
    "below",
    "between",
    "into",
    "through",
    "over",
    "under",
    "again",
    "further",
    "once",
    "here",
    "there",
    "everywhere",
    "anywhere",
    "somewhere",
    "get",
    "me",
    "my",
    "your",
    "his",
    "her",
    "its",
    "our",
    "their",
    "what's",
    "how's",
    "where's",
    "who's",
    "when's",
    "why's",
    "latest",
    "new",
    "best",
    "top",
    "guide",
    "overview",
    "introduction",
    "vs",
    "versus",
    "compare",
    "comparison",
    "difference",
    "between",
    "2020",
    "2021",
    "2022",
    "2023",
    "2024",
    "2025",
    "2026",
    "2027",
];

/// Extract informative keywords from a query.
///
/// Lowercases, strips punctuation, removes stop words, deduplicates while
/// preserving order, and caps at 6 keywords so relevance signals stay focused.
#[must_use]
pub fn extract_keywords(query: &str) -> Vec<String> {
    let stop_set: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let mut keywords: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty() && !stop_set.contains(s) && s.len() > 1)
        .map(|s| s.to_string())
        .collect();

    // Deduplicate while preserving order.
    let mut seen = HashSet::new();
    keywords.retain(|k| seen.insert(k.clone()));
    keywords.truncate(6);
    keywords
}

/// Count query keywords present in a title/snippet text.
///
/// Tokens are matched as whole words or prefixes, so morphological variants
/// count ("asynchronous" matches "async", "runtimes" matches "runtime",
/// hyphenated compounds like "pyo3-async-runtimes" split into matchable
/// tokens). The prefix rule accepts rare false positives ("rusty" matches
/// "rust") in exchange for catching those variants — acceptable for a
/// tie-break signal.
#[must_use]
pub fn keyword_match_count(keywords: &[String], title: &str, snippet: &str) -> usize {
    if keywords.is_empty() {
        return 0;
    }
    let title_lower = title.to_lowercase();
    let snippet_lower = snippet.to_lowercase();
    let tokens: HashSet<&str> = title_lower
        .split(|c: char| !c.is_alphanumeric())
        .chain(snippet_lower.split(|c: char| !c.is_alphanumeric()))
        .filter(|t| !t.is_empty())
        .collect();
    keywords
        .iter()
        .filter(|k| {
            tokens
                .iter()
                .any(|t| *t == k.as_str() || t.starts_with(k.as_str()))
        })
        .count()
}

/// Coarse grouping key for fusion aggregation: host + first path segment.
///
/// Engines return different URLs for the same concept (tokio.rs/,
/// tokio.rs/tokio/tutorial/async), so per-URL RRF scores stay flat and the
/// URL tie-break decides arbitrarily. Grouping by this key accumulates
/// evidence across URL variants. Homepages (no path) key on the host alone so
/// they can merge into the same-host section group.
#[must_use]
pub fn aggregation_key(url: &str) -> String {
    let Ok(parsed) = Url::parse(url) else {
        return url.to_string();
    };
    let Some(host) = parsed.host_str() else {
        return url.to_string();
    };
    let host = host.trim_start_matches("www.");
    let first_segment = parsed
        .path()
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("");
    if first_segment.is_empty() {
        host.to_string()
    } else {
        format!("{host}/{first_segment}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bing_tracker_decodes_valid_url() {
        let destination = "https://example.com/page?q=1";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(destination);
        let href = format!("https://www.bing.com/ck/a?u=a1{encoded}&p=1");
        assert_eq!(decode_bing_tracker_url(&href), destination);
    }

    #[test]
    fn bing_tracker_returns_input_for_non_bing() {
        let href = "https://example.com/ck/a?u=whatever";
        assert_eq!(decode_bing_tracker_url(href), href);
    }

    #[test]
    fn bing_tracker_returns_input_for_invalid_payload() {
        let href = "https://www.bing.com/ck/a?u=a1!!!not-base64!!!";
        assert_eq!(decode_bing_tracker_url(href), href);
    }

    #[test]
    fn bing_tracker_returns_input_for_short_payload() {
        let href = "https://www.bing.com/ck/a?u=a1";
        assert_eq!(decode_bing_tracker_url(href), href);
    }

    #[test]
    fn ddg_redirect_decodes_destination() {
        let destination = "https://example.org/doc";
        let href = format!("https://duckduckgo.com/l/?uddg={destination}&rut=abc");
        assert_eq!(decode_ddg_redirect_url(&href), destination);
    }

    #[test]
    fn ddg_redirect_decodes_protocol_relative_href() {
        let destination = "https://example.org/doc";
        let href = format!("//duckduckgo.com/l/?uddg={destination}&rut=abc");
        assert_eq!(decode_ddg_redirect_url(&href), destination);
    }

    #[test]
    fn bing_tracker_decodes_protocol_relative_href() {
        let destination = "https://example.com/page?q=1";
        let encoded = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(destination);
        let href = format!("//www.bing.com/ck/a?u=a1{encoded}&p=1");
        assert_eq!(decode_bing_tracker_url(&href), destination);
    }

    #[test]
    fn ddg_redirect_returns_input_for_non_ddg() {
        let href = "https://example.com/l/?uddg=https://example.org";
        assert_eq!(decode_ddg_redirect_url(href), href);
    }

    #[test]
    fn normalize_resolves_known_redirects() {
        let destination = "https://example.org/doc";
        let ddg = format!("https://duckduckgo.com/l/?uddg={destination}");
        assert_eq!(normalize_result_url(&ddg), destination);
        let plain = "https://example.org/plain";
        assert_eq!(normalize_result_url(plain), plain);
    }

    #[test]
    fn canonical_url_strips_fragment_and_tracking() {
        let url = "https://example.com/path?utm_source=x&q=1&fbclid=y#section";
        assert_eq!(canonical_url(url), "https://example.com/path?q=1");
    }

    #[test]
    fn canonical_url_keeps_plain_urls() {
        let url = "https://example.com/path";
        assert_eq!(canonical_url(url), url);
    }

    #[test]
    fn canonical_url_handles_invalid_input() {
        let url = "not a url";
        assert_eq!(canonical_url(url), url);
    }

    #[test]
    fn positional_relevance_decays_and_clamps() {
        assert_eq!(positional_relevance(0, 4), 1.0);
        assert_eq!(positional_relevance(3, 4), 0.25);
        assert_eq!(positional_relevance(0, 0), 0.0);
        assert!(positional_relevance(1, 2) < positional_relevance(1, 10));
    }

    #[test]
    fn strip_date_prefix_removes_absolute_and_relative() {
        assert_eq!(strip_date_prefix("2025-01-15 · Snippet one"), "Snippet one");
        assert_eq!(
            strip_date_prefix("Jan 15, 2025 - Snippet two"),
            "Snippet two"
        );
        assert_eq!(
            strip_date_prefix("3 days ago · Snippet three"),
            "Snippet three"
        );
        assert_eq!(strip_date_prefix("Plain snippet"), "Plain snippet");
        assert_eq!(strip_date_prefix(""), "");
    }

    #[test]
    fn snippet_dates_parse_absolute_iso() {
        let dt = parse_date_from_snippet("2025-01-15 · Some snippet").expect("date parses");
        assert_eq!(dt.date_naive().to_string(), "2025-01-15");
    }

    #[test]
    fn snippet_dates_parse_iso_alone() {
        let dt = parse_date_from_snippet("2025-01-15").expect("date parses");
        assert_eq!(dt.date_naive().to_string(), "2025-01-15");
    }

    #[test]
    fn snippet_dates_parse_month_name() {
        let dt = parse_date_from_snippet("Jan 15, 2025 - Some snippet").expect("date parses");
        assert_eq!(dt.date_naive().to_string(), "2025-01-15");
    }

    #[test]
    fn snippet_dates_parse_relative() {
        let dt = parse_date_from_snippet("3 days ago - Some snippet").expect("date parses");
        let expected = Utc::now() - Duration::days(3);
        assert!((dt - expected).num_seconds().abs() < 2);
    }

    #[test]
    fn snippet_dates_return_none_without_date() {
        assert!(parse_date_from_snippet("Just a plain snippet").is_none());
        assert!(parse_date_from_snippet("").is_none());
        assert!(parse_date_from_snippet("2025 results found").is_none());
    }

    #[test]
    fn extract_keywords_filters_stop_words_and_dedups() {
        let kw = extract_keywords("What is the latest Rust async runtime in 2026?");
        assert!(kw.contains(&"rust".to_string()));
        assert!(kw.contains(&"async".to_string()));
        assert!(kw.contains(&"runtime".to_string()));
        assert!(!kw.contains(&"the".to_string()));
        assert!(!kw.contains(&"latest".to_string()));
        assert!(!kw.contains(&"2026".to_string()));
        // Dedup preserves order and caps at 6.
        let dup = extract_keywords("alpha alpha beta beta gamma delta delta delta");
        assert_eq!(dup, vec!["alpha", "beta", "gamma", "delta"]);
    }

    #[test]
    fn keyword_match_count_counts_whole_words_and_prefixes() {
        let keywords = vec![
            "engine".to_string(),
            "search".to_string(),
            "network".to_string(),
        ];
        // Whole words and hyphenated compounds count.
        assert_eq!(
            keyword_match_count(&keywords, "Search - An efficient network engine", ""),
            3
        );
        assert_eq!(
            keyword_match_count(&keywords, "fast-search-engine", "network protocol"),
            3
        );
        // Morphological variants count via prefix ("networking" → "network").
        assert_eq!(
            keyword_match_count(&keywords, "An advanced networking search engine", ""),
            3
        );
        // Empty keywords never match.
        assert_eq!(keyword_match_count(&[], "Search network", ""), 0);
    }

    #[test]
    fn aggregation_key_groups_by_host_and_first_segment() {
        assert_eq!(aggregation_key("https://service.example/"), "service.example");
        assert_eq!(
            aggregation_key("https://service.example/docs/tutorial/core"),
            "service.example/docs"
        );
        assert_eq!(
            aggregation_key("https://www.service.example/docs/tutorial/core"),
            "service.example/docs"
        );
        assert_eq!(
            aggregation_key("https://arxiv.org/abs/2602.07455"),
            "arxiv.org/abs"
        );
        assert_eq!(
            aggregation_key("https://arxiv.org/pdf/2608.20677"),
            "arxiv.org/pdf"
        );
        // Invalid URLs fall back to the input unchanged.
        assert_eq!(aggregation_key("not a url"), "not a url");
    }

    use proptest::prelude::*;

    proptest! {
        #[test]
        fn proptest_positional_relevance_monotonicity(
            total in 1usize..500,
            rank in 0usize..500
        ) {
            let actual_rank = rank % total;
            let score = positional_relevance(actual_rank, total);
            prop_assert!((0.0..=1.0).contains(&score), "Score {} out of [0, 1]", score);

            if actual_rank > 0 {
                let prev_score = positional_relevance(actual_rank - 1, total);
                prop_assert!(prev_score >= score, "Previous rank {} must have higher or equal score than current {}", prev_score, score);
            }
        }

        #[test]
        fn proptest_extract_keywords_bounds(
            text in "[a-zA-Z0-9 _-]{1,100}"
        ) {
            let kws = extract_keywords(&text);
            prop_assert!(kws.len() <= 6, "Keywords len {} exceeded max limit 6", kws.len());
            for kw in &kws {
                prop_assert!(kw.len() >= 2, "Keyword {} shorter than 2 chars", kw);
                prop_assert!(
                    kw.starts_with(|c: char| c.is_alphanumeric()) && kw.ends_with(|c: char| c.is_alphanumeric()),
                    "Keyword {} must start and end with alphanumeric chars",
                    kw
                );
            }
        }
    }
}
