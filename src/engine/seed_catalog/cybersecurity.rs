//! Cybersecurity & Infosec — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "cybersecurity",
    name: "Cybersecurity & Infosec",
    topics: &[
        "security",
        "cybersecurity",
        "vulnerability",
        "threat",
        "exploit",
        "cve",
    ],
    sources: &[
        SeedSource {
            url: "https://nvd.nist.gov/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.kb.cert.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://cve.mitre.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.sans.org/newsletters/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://securelist.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://blog.cloudflare.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.mandiant.com/resources/blog",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://unit42.paloaltonetworks.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.kaspersky.com/blog/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://krebsonsecurity.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
