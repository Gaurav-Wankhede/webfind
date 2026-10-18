//! Curated, non-Wikipedia seed catalog for background crawling.
//!
//! The daemon pre-loads these authoritative source URLs so the local index has
//! topic awareness from the start — official documentation, standards bodies,
//! primary registries, and vendor docs rather than encyclopedia mirrors. Each
//! source carries a recrawl interval so fresh content is re-indexed
//! continuously, giving the local index the "fresh publications" behavior of a
//! paid retrieval service without the cloud dependency.
//!
//! Design rule: **never seed Wikipedia.** Prefer official docs, RFCs, standards
//! bodies, and primary sources so extracted content is authoritative and the
//! crawled HTML is dense, high-signal, and LLM-friendly.
//!
//! The catalog is modular: each domain lives in its own `*.rs` submodule so
//! sources can be curated and expanded independently without touching the
//! aggregate. Add a new domain by (1) creating `seed_catalog/<slug>.rs` that
//! defines `pub const DOMAIN: CuratedDomain`, then (2) registering it in the
//! `DOMAINS` aggregate in this file.

use std::time::Duration;

pub mod ai_ml;
pub mod automotive;
pub mod banking;
pub mod business;
pub mod crypto;
pub mod cybersecurity;
pub mod data_science;
pub mod ecommerce;
pub mod education;
pub mod energy;
pub mod finance;
pub mod food;
pub mod gaming;
pub mod government;
pub mod legal;
pub mod marketing;
pub mod medical;
pub mod open_source;
pub mod programming;
pub mod reddit;
pub mod research;
pub mod science;
pub mod sports;
pub mod tech;
pub mod travel;

/// A curated source URL for one domain.
#[derive(Debug, Clone, Copy)]
pub struct SeedSource {
    /// Official / primary URL to crawl.
    pub url: &'static str,
    /// How often to re-crawl this source to pick up fresh content.
    pub recrawl: Recrawl,
}

/// Re-crawl cadence for a seed source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recrawl {
    /// Daily — high-churn sources (news, finance).
    Daily,
    /// Weekly — medium churn (docs, blogs, registries).
    Weekly,
    /// Monthly — stable primary sources (standards, RFCs).
    Monthly,
}

impl Recrawl {
    pub fn interval(&self) -> Duration {
        match self {
            Recrawl::Daily => Duration::from_secs(24 * 3600),
            Recrawl::Weekly => Duration::from_secs(7 * 24 * 3600),
            Recrawl::Monthly => Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// A curated domain (research, programming, finance, ...) and its sources.
#[derive(Debug, Clone)]
pub struct CuratedDomain {
    /// Stable slug used as the crawl topic and config key.
    pub slug: &'static str,
    /// Human-readable label.
    pub name: &'static str,
    /// Topics used for content-aware link prioritization during the crawl.
    pub topics: &'static [&'static str],
    /// Authoritative, non-Wikipedia seed URLs.
    pub sources: &'static [SeedSource],
}

/// All curated domains. Order matters: it defines the daemon's crawl priority.
/// Register new domains here after adding their `*.rs` module.
pub const DOMAINS: &[CuratedDomain] = &[
    research::DOMAIN,
    programming::DOMAIN,
    medical::DOMAIN,
    finance::DOMAIN,
    banking::DOMAIN,
    tech::DOMAIN,
    cybersecurity::DOMAIN,
    legal::DOMAIN,
    science::DOMAIN,
    energy::DOMAIN,
    ai_ml::DOMAIN,
    data_science::DOMAIN,
    marketing::DOMAIN,
    business::DOMAIN,
    ecommerce::DOMAIN,
    education::DOMAIN,
    government::DOMAIN,
    gaming::DOMAIN,
    automotive::DOMAIN,
    sports::DOMAIN,
    travel::DOMAIN,
    food::DOMAIN,
    open_source::DOMAIN,
    crypto::DOMAIN,
    reddit::DOMAIN,
];

/// Look up a curated domain by slug. Returns `None` for unknown slugs.
pub fn domain_by_slug(slug: &str) -> Option<&'static CuratedDomain> {
    DOMAINS.iter().find(|d| d.slug == slug)
}

/// All source URLs across every curated domain.
pub fn all_source_urls() -> Vec<(&'static str, Recrawl)> {
    DOMAINS
        .iter()
        .flat_map(|d| d.sources.iter().map(|s| (s.url, s.recrawl)))
        .collect()
}

/// Total number of curated source URLs.
pub fn source_count() -> usize {
    all_source_urls().len()
}

/// All source URLs ordered by source-precision tier (highest trust first):
/// PrimaryOfficial/RawSource before Reference before Encyclopedia/Community.
/// The curation daemon consumes this so the local index fills with authoritative
/// content first. Ties keep catalog order (stable sort).
#[must_use]
pub fn priority_ordered_sources() -> Vec<(&'static str, Recrawl, crate::engine::source_tier::SourceTier)> {
    let mut v: Vec<(&'static str, Recrawl, crate::engine::source_tier::SourceTier)> = DOMAINS
        .iter()
        .flat_map(|d| d.sources.iter().map(|s| {
            (s.url, s.recrawl, crate::engine::source_tier::classify(s.url))
        }))
        .collect();
    // Sort by precision DESC, stable so equal-tier sources keep catalog order.
    v.sort_by(|a, b| b.2.precision().total_cmp(&a.2.precision()));
    v
}

/// Human-readable summary of the curated catalog.
pub fn catalog_summary() -> String {
    let mut out = String::from("Curated seed catalog (non-Wikipedia, official sources)\n");
    for d in DOMAINS {
        out.push_str(&format!(
            "  {:>14}  {:>4} sources  —  {}\n",
            d.slug,
            d.sources.len(),
            d.name
        ));
    }
    out.push_str(&format!("  {:>14}  {:>4} total\n", "total", source_count()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_catalog_has_many_domains() {
        assert!(
            DOMAINS.len() >= 20,
            "catalog should be broad, got {}",
            DOMAINS.len()
        );
        assert!(
            source_count() >= 100,
            "catalog should be deep, got {}",
            source_count()
        );
    }

    #[test]
    fn test_no_wikipedia_sources() {
        let bad: Vec<&str> = all_source_urls()
            .iter()
            .filter(|(u, _)| u.contains("wikipedia.org"))
            .map(|(u, _)| *u)
            .collect();
        assert!(
            bad.is_empty(),
            "catalog must never seed Wikipedia, found: {:?}",
            bad
        );
    }

    #[test]
    fn test_sources_are_https() {
        let bad: Vec<&str> = all_source_urls()
            .iter()
            .filter(|(u, _)| !u.starts_with("https://"))
            .map(|(u, _)| *u)
            .collect();
        assert!(
            bad.is_empty(),
            "all sources must be https, found: {:?}",
            bad
        );
    }

    #[test]
    fn test_all_slugs_unique() {
        let mut seen = std::collections::HashSet::new();
        for d in DOMAINS {
            assert!(seen.insert(d.slug), "duplicate slug: {}", d.slug);
            assert!(!d.sources.is_empty(), "domain {} has no sources", d.slug);
        }
    }

    #[test]
    fn test_domain_by_slug() {
        assert!(domain_by_slug("finance").is_some());
        assert!(domain_by_slug("medical").is_some());
        assert!(domain_by_slug("nonexistent").is_none());
    }

    #[test]
    fn priority_order_is_non_increasing() {
        let ordered = priority_ordered_sources();
        assert!(!ordered.is_empty());
        let mut prev = f64::INFINITY;
        for (_, _, tier) in &ordered {
            let p = tier.precision();
            assert!(p <= prev + 1e-9, "precision must be non-increasing: {} after {}", p, prev);
            prev = p;
        }
    }

    #[test]
    fn official_sources_sort_before_encyclopedia() {
        let ordered = priority_ordered_sources();
        let first_official = ordered
            .iter()
            .position(|(_, _, t)| *t == crate::engine::source_tier::SourceTier::PrimaryOfficial);
        let last_encyclopedia = ordered
            .iter()
            .rposition(|(_, _, t)| *t == crate::engine::source_tier::SourceTier::Encyclopedia);
        if let (Some(fo), Some(le)) = (first_official, last_encyclopedia) {
            assert!(fo < le, "official must precede encyclopedia in crawl priority");
        }
    }
}
