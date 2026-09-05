//! Research & Academic — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "research",
    name: "Research & Academic",
    topics: &[
        "research",
        "paper",
        "arxiv",
        "preprint",
        "publication",
        "academic",
    ],
    sources: &[
        SeedSource {
            url: "https://arxiv.org/list/cs.AI/recent",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://pubmed.ncbi.nlm.nih.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.semanticscholar.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.nature.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.science.org/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://scholar.google.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.researchgate.net/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.plos.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.frontiersin.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://academic.oup.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.springer.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://ieeexplore.ieee.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://aclanthology.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://openreview.net/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
