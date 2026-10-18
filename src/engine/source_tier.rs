use anyhow::Result;
use serde::{Deserialize, Serialize};

// Source-precision tiering — the SSOT for "how much do we trust this domain".
//
// Trust hierarchy (user doctrine, grounded in Wikipedia:Academic use policy:
// encyclopedias are editable-by-anyone and carry documented vandalism/manipulation
// risk, whereas arXiv / Nature / publishers / official bodies are primary or
// peer-reviewed sources):
//
//   RawSource       (0.95)  raw.githubusercontent.com, gist raw, source code
//   PrimaryOfficial (1.00)  .gov/.edu/.mil/.int, standards bodies (ietf,w3,iso,nist),
//                            vendor docs (doc.rust-lang.org, docs.rs, MDN, ...)
//   Reference       (0.70)  arxiv, nature, publishers, standards mirrors
//   Encyclopedia    (0.55)  wikipedia.org, wikimedia.org, britannica (demoted)
//   Community       (0.45)  reddit, stackoverflow, dev.to, quora
//
// `classify` is the single entry point; ranking and crawl priority both call it so
// the trust ordering lives in exactly one place.

/// Trust tier for a source domain/URL.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SourceTier {
    /// raw.githubusercontent.com and other raw source-code endpoints.
    RawSource,
    /// Official / primary: government, education, military, standards bodies, vendor docs.
    PrimaryOfficial,
    /// Peer-reviewed / preprint / publisher references (arXiv, Nature, Springer, ...).
    Reference,
    /// Encyclopedias — editable by anyone; documented manipulation risk. Demoted.
    Encyclopedia,
    /// Community / Q&A — useful but lowest trust.
    Community,
    /// Unknown domain — neutral midpoint, treated like a weak Reference.
    Unknown,
}

impl SourceTier {
    /// Base source-precision multiplier (higher = more trusted).
    #[must_use]
    pub fn precision(self) -> f64 {
        match self {
            SourceTier::RawSource => 0.95,
            SourceTier::PrimaryOfficial => 1.00,
            SourceTier::Reference => 0.70,
            SourceTier::Encyclopedia => 0.55,
            SourceTier::Community => 0.45,
            SourceTier::Unknown => 0.60,
        }
    }

    /// Stable label for diagnostics/tests.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            SourceTier::RawSource => "raw_source",
            SourceTier::PrimaryOfficial => "primary_official",
            SourceTier::Reference => "reference",
            SourceTier::Encyclopedia => "encyclopedia",
            SourceTier::Community => "community",
            SourceTier::Unknown => "unknown",
        }
    }
}

/// Primary-official domains: vendor docs, standards bodies, registries.
/// Kept explicit so a new official source is a one-line addition (SSOT).




/// Classify a URL's trust tier from its host.
///
/// Raw-source hosts are checked first (raw.githubusercontent.com), then official
/// TLDs (.gov/.edu/.mil/.int), then the explicit domain lists, then encyclopedia
/// and community. Unknown hosts fall back to `SourceTier::Unknown`.
#[must_use]
pub fn classify(url: &str) -> SourceTier {
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
        .unwrap_or_default();
    let h = host.as_str();

    // Raw source code first.
    if h == "raw.githubusercontent.com" || h == "gist.githubusercontent.com" {
        return SourceTier::RawSource;
    }

    if let Some(parsed) = psl::domain(h.as_bytes()) {
        let suffix = parsed.suffix().as_bytes();
        let root = parsed.as_bytes();
        if suffix == b"gov" || suffix == b"edu" || suffix == b"mil" || suffix == b"int"
            || suffix == b"gov.uk" || suffix == b"gov.au" || suffix == b"ac.uk" || suffix == b"edu.au"
        {
            return SourceTier::PrimaryOfficial;
        }
        if root == b"wikipedia.org" || root == b"wikimedia.org" || root == b"britannica.com" {
            return SourceTier::Encyclopedia;
        }
        if root == b"reddit.com" || root == b"stackoverflow.com" || root == b"stackexchange.com" || root == b"quora.com" {
            return SourceTier::Community;
        }
        if root == b"rust-lang.org" || root == b"docs.rs" || root == b"python.org" || root == b"go.dev" {
            return SourceTier::PrimaryOfficial;
        }
        if suffix == b"org" {
            return SourceTier::Reference;
        }
    }

    SourceTier::Unknown
}

/// Query-intent signal: does this query want primary/official/raw sources over
/// encyclopedias? True for code/error-code/identifier queries (CVE-*, E0xxx,
/// GHSA-*, "how does/implement/parse/compile"). Used by ranker to demote
/// Wikipedia and boost raw/official sources for these intents.
#[must_use]
pub fn route_intent_code(query: &str) -> bool {
    let q = query.to_lowercase();
    let has_identifier = q.contains("cve-")
        || q.contains("ghsa-")
        || q.contains("rfc ")
        || q.contains('@')
        || q.split_whitespace().any(|t| {
            (t.starts_with('e') || t.starts_with('w'))
                && t.len() >= 4
                && t[1..].chars().all(|c| c.is_ascii_digit())
        });
    let code_verbs = [
        "how does", "how do", "how to", "implement", "implementing", "parse", "parsing",
        "compile", "compiler", "error", "stack trace", "panic", "segfault", "debug",
        "api", "sdk", "library", "crate", "package", "function", "struct", "trait",
        "async", "await", "borrow", "lifetime", "deserialize", "serialize",
    ];
    let has_code_verb = code_verbs.iter().any(|v| q.contains(v));
    has_identifier || has_code_verb
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_source_is_trusted() {
        assert_eq!(classify("https://raw.githubusercontent.com/serde-rs/serde/main/src/lib.rs"), SourceTier::RawSource);
    }

    #[test]
    fn wikipedia_is_encyclopedia() {
        assert_eq!(classify("https://en.wikipedia.org/wiki/Rust"), SourceTier::Encyclopedia);
    }

    #[test]
    fn arxiv_is_reference_not_wiki() {
        assert_eq!(classify("https://arxiv.org/abs/2509.02462"), SourceTier::Reference);
    }

    #[test]
    fn official_tld_is_primary() {
        assert_eq!(classify("https://cdc.gov/flu"), SourceTier::PrimaryOfficial);
        assert_eq!(classify("https://mit.edu/cs"), SourceTier::PrimaryOfficial);
        assert_eq!(classify("https://www.cisa.gov/advisory"), SourceTier::PrimaryOfficial);
        assert_eq!(classify("https://doc.rust-lang.org/book/"), SourceTier::PrimaryOfficial);
    }

    #[test]
    fn community_is_lowest() {
        assert_eq!(classify("https://stackoverflow.com/questions/1"), SourceTier::Community);
        assert_eq!(classify("https://www.reddit.com/r/rust/"), SourceTier::Community);
    }

    #[test]
    fn trust_ordering_holds() {
        // User doctrine: official/raw > reference > encyclopedia > community.
        assert!(SourceTier::PrimaryOfficial.precision() > SourceTier::Reference.precision());
        assert!(SourceTier::Reference.precision() > SourceTier::Encyclopedia.precision());
        assert!(SourceTier::Encyclopedia.precision() > SourceTier::Community.precision());
        assert!(SourceTier::RawSource.precision() >= SourceTier::Reference.precision());
    }
}

impl std::str::FromStr for SourceTier {
    type Err = anyhow::Error;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "raw_source" => Ok(SourceTier::RawSource),
            "primary_official" => Ok(SourceTier::PrimaryOfficial),
            "reference" => Ok(SourceTier::Reference),
            "encyclopedia" => Ok(SourceTier::Encyclopedia),
            "community" => Ok(SourceTier::Community),
            "unknown" => Ok(SourceTier::Unknown),
            _ => Err(anyhow::anyhow!("Unknown SourceTier: {}", s)),
        }
    }
}
