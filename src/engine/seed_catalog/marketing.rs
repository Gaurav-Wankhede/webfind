//! Marketing & SEO — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "marketing",
    name: "Marketing & SEO",
    topics: &[
        "marketing",
        "seo",
        "advertising",
        "content marketing",
        "brand",
    ],
    sources: &[
        SeedSource {
            url: "https://moz.com/blog",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.searchenginejournal.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.semrush.com/blog/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://blog.hubspot.com/marketing",
            recrawl: Recrawl::Daily,
        },
    ],
};
