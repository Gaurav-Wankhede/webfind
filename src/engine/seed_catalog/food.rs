//! Food & Nutrition — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "food",
    name: "Food & Nutrition",
    topics: &["food", "nutrition", "cooking", "recipe", "diet"],
    sources: &[
        SeedSource {
            url: "https://www.eater.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.foodnetwork.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.nutrition.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.seriouseats.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
