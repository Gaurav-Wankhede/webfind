use std::collections::HashSet;
use std::sync::LazyLock;
use regex::Regex;
use crate::schema::content::{CodeBlockEntity, Entities, ExampleEntity, MathEntity};

static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9_.+-]+@[a-zA-Z0-9-]+.[a-zA-Z0-9-.]+").unwrap()
});

static RE_PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\+?\d{1,3}[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}").unwrap()
});

static RE_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b\d{1,5}\s+[a-z0-9\s.,]{3,30}\s+(?:street|st|avenue|ave|road|rd|boulevard|blvd|drive|dr|lane|ln|way)\b").unwrap()
});

static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\s/$.?#].[^\s]*").unwrap()
});

static RE_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bv?(?:[0-9]+\.){1,3}[0-9]+(?:-[a-z0-9.]+)?\b").unwrap()
});

static RE_LICENSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:MIT|Apache-2\.0|GPL-[0-9.]+|BSD-[0-9]?-Clause|MPL-2\.0|ISC|Unlicense|CC-BY-[0-9.]+|AGPL-[0-9.]+)\b").unwrap()
});

static RE_PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[$€£¥]\s*[0-9]+(?:,[0-9]{3})*(?:\.[0-9]{2})?|[0-9]+(?:\.[0-9]{2})?\s*(?:USD|EUR|GBP|JPY|CAD|AUD))\b").unwrap()
});

static RE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:20\d{2}|19\d{2})[-/.](?:0[1-9]|1[0-2])[-/.](?:0[1-9]|[12]\d|3[01])\b|\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\s+\d{1,2},?\s+(?:20\d{2}|19\d{2})\b").unwrap()
});

static RE_IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b|(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}\b").unwrap()
});

static RE_SOCIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\A|\s)(@[a-zA-Z0-9_]{3,30})\b").unwrap()
});

static RE_MATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\$\$[\s\S]+?\$\$|\$[^\$\n]+?\$|\\\[[\s\S]+?\\\]|\\\([^\n]+?\\\))").unwrap()
});

pub(crate) fn extract_entities(text: &str) -> Entities {
    fn unique(v: Vec<String>) -> Vec<String> {
        let mut seen = HashSet::new();
        v.into_iter()
            .filter(|s| !s.trim().is_empty())
            .filter(|s| seen.insert(s.to_lowercase()))
            .collect()
    }

    let emails = unique(
        RE_EMAIL
            .find_iter(text)
            .map(|m| m.as_str().trim_end_matches('.').to_string())
            .collect(),
    );
    let phones = unique(
        RE_PHONE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let addresses = unique(
        RE_ADDRESS
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let urls = unique(
        RE_URL
            .find_iter(text)
            .map(|m| {
                m.as_str()
                    .trim_end_matches(['.', ')', ';', ',', '"', '\'', '>', ']'].as_slice())
                    .to_string()
            })
            .collect(),
    );
    let repository_urls = urls
        .iter()
        .filter(|u| {
            u.contains("github.com/")
                || u.contains("gitlab.com/")
                || u.contains("crates.io/crates/")
                || u.contains("npmjs.com/package/")
        })
        .cloned()
        .collect();

    let package_versions = unique(
        RE_VERSION
            .find_iter(text)
            .map(|m| m.as_str().trim())
            .filter(|v| {
                let clean = v.strip_prefix('v').unwrap_or(v);
                semver::Version::parse(clean).is_ok() || clean.split('.').count() == 2
            })
            .map(|s| s.to_string())
            .collect(),
    );
    let licenses = unique(
        RE_LICENSE
            .find_iter(text)
            .map(|m| m.as_str().trim())
            .map(|lic| {
                // Normalize to canonical SPDX identifier if recognized by the official spdx crate
                if let Some(id) = spdx::license_id(lic) {
                    id.name.to_string()
                } else {
                    lic.to_string()
                }
            })
            .collect(),
    );

    let prices = unique(
        RE_PRICE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let dates = unique(
        RE_DATE
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let ip_addresses = unique(
        RE_IP
            .find_iter(text)
            .map(|m| m.as_str().trim().to_string())
            .collect(),
    );
    let social_handles = unique(
        RE_SOCIAL
            .captures_iter(text)
            .filter_map(|c| c.get(1).map(|m| m.as_str().trim().to_string()))
            .collect(),
    );
    let maths = {
        let mut seen = HashSet::new();
        RE_MATH
            .find_iter(text)
            .filter_map(|m| {
                let raw = m.as_str().trim().to_string();
                if raw.len() < 6 || !seen.insert(raw.to_lowercase()) {
                    return None;
                }
                let display = raw.starts_with("$$") || raw.starts_with("\\[");
                // Parenthesized `\(...\)` needs an operator, otherwise it is
                // prose in brackets (false-positive guard). Display forms are
                // unambiguous by delimiter alone.
                if !display
                    && !raw.contains(['=', '^', '_', '+', '*', '/', '\\'])
                {
                    return None;
                }
                Some(MathEntity { tex: raw, display })
            })
            .take(200)
            .collect()
    };

    let parsed_examples = crate::engine::ranker::Ranker::extract_examples(text);
    let code_blocks = parsed_examples
        .iter()
        .map(|e| CodeBlockEntity {
            language: e.language.clone(),
            code: e.snippet.clone(),
            line_count: e.snippet.lines().count(),
        })
        .collect();
    let examples = parsed_examples
        .into_iter()
        .map(|e| ExampleEntity {
            context_heading: e.context,
            language: e.language,
            code: e.snippet,
        })
        .collect();

    Entities {
        code_blocks,
        tables: Vec::new(),
        headings: Vec::new(),
        diagrams: Vec::new(),
        examples,
        faqs: Vec::new(),
        steps: Vec::new(),
        maths,
        callouts: Vec::new(),
        package_versions,
        licenses,
        repository_urls,
        emails,
        phones,
        addresses,
        urls,
        prices,
        dates,
        ip_addresses,
        social_handles,
    }
}

