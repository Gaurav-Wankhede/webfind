//! Business & Entrepreneurship — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "business",
    name: "Business & Entrepreneurship",
    topics: &[
        "business",
        "startup",
        "entrepreneurship",
        "management",
        "venture",
    ],
    sources: &[
        SeedSource {
            url: "https://www.forbes.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.inc.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://hbr.org/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.businessinsider.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
