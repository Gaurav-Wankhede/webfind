//! Open Source & Dev Tools — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "open-source",
    name: "Open Source & Dev Tools",
    topics: &[
        "open source",
        "github",
        "developer tools",
        "license",
        "contribution",
    ],
    sources: &[
        SeedSource {
            url: "https://opensource.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://github.com/trending",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.fsf.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.eclipse.org/news/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
