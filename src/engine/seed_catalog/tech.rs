//! Technology & Industry — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "tech",
    name: "Technology & Industry",
    topics: &[
        "technology",
        "software",
        "cloud",
        "ai",
        "startup",
        "semiconductor",
    ],
    sources: &[
        SeedSource {
            url: "https://techcrunch.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://arstechnica.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.theverge.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.wired.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://openai.com/news/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://research.google/blog/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.anthropic.com/news",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.deepmind.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://ai.meta.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://cloud.google.com/blog",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://azure.microsoft.com/en-us/blog/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.zdnet.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://spectrum.ieee.org/",
            recrawl: Recrawl::Daily,
        },
    ],
};
