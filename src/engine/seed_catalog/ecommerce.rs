//! E-commerce & Retail — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "ecommerce",
    name: "E-commerce & Retail",
    topics: &["ecommerce", "retail", "shopping", "d2c", "marketplace"],
    sources: &[
        SeedSource {
            url: "https://www.shopify.com/blog",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.oberlo.com/blog",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.nchannel.com/blog",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.ecommercefuel.com/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
