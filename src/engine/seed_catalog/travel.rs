//! Travel & Hospitality — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "travel",
    name: "Travel & Hospitality",
    topics: &["travel", "tourism", "hospitality", "airline", "hotel"],
    sources: &[
        SeedSource {
            url: "https://www.lonelyplanet.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.travelandleisure.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.nomadicmatt.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.skift.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
