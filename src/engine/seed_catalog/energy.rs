//! Energy & Climate — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "energy",
    name: "Energy & Climate",
    topics: &[
        "energy",
        "climate",
        "renewable",
        "oil",
        "grid",
        "solar",
        "emissions",
    ],
    sources: &[
        SeedSource {
            url: "https://www.iea.org/news",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.irena.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.eia.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://unfccc.int/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.iea.org/energy-system",
            recrawl: Recrawl::Weekly,
        },
    ],
};
