//! Government & Public Policy — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "government",
    name: "Government & Public Policy",
    topics: &[
        "government",
        "public policy",
        "congress",
        "legislation",
        "election",
    ],
    sources: &[
        SeedSource {
            url: "https://www.congress.gov/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.whitehouse.gov/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.usa.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://data.gov/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
