//! Education & E-learning — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "education",
    name: "Education & E-learning",
    topics: &[
        "education",
        "elearning",
        "online learning",
        "courses",
        "university",
    ],
    sources: &[
        SeedSource {
            url: "https://www.coursera.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.edx.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.khanacademy.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.education.com/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
