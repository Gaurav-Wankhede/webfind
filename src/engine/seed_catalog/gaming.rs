//! Gaming & Esports — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "gaming",
    name: "Gaming & Esports",
    topics: &["gaming", "esports", "games", "console", "vr"],
    sources: &[
        SeedSource {
            url: "https://www.ign.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.gamespot.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.polygon.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.esportsinsider.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
