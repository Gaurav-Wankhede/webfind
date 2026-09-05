//! Automotive & EV — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "automotive",
    name: "Automotive & EV",
    topics: &[
        "automotive",
        "electric vehicle",
        "ev",
        "car",
        "transportation",
    ],
    sources: &[
        SeedSource {
            url: "https://www.caranddriver.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.tesla.com/blog",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://electrek.co/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.autoblog.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
