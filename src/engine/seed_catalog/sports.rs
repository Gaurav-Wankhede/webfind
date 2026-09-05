//! Sports & Fitness — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "sports",
    name: "Sports & Fitness",
    topics: &["sports", "fitness", "health", "training", "olympics"],
    sources: &[
        SeedSource {
            url: "https://www.espn.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.sports-reference.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.bodybuilding.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.healthline.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
