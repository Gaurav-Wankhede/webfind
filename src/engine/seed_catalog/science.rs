//! Science & Space — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "science",
    name: "Science & Space",
    topics: &[
        "science",
        "space",
        "physics",
        "nasa",
        "astronomy",
        "climate",
    ],
    sources: &[
        SeedSource {
            url: "https://www.nasa.gov/news/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.esa.int/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://home.cern/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.nsf.gov/news/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.ipcc.ch/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.noaa.gov/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.nist.gov/news-events",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.energy.gov/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
