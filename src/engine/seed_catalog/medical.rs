//! Medical & Health — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "medical",
    name: "Medical & Health",
    topics: &[
        "medical",
        "health",
        "disease",
        "clinical",
        "drug",
        "treatment",
        "medicine",
    ],
    sources: &[
        SeedSource {
            url: "https://www.who.int/news-room",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.cdc.gov/media/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.nih.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.fda.gov/drugs",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.mayoclinic.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.hopkinsmedicine.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.clevelandclinic.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.nejm.org/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.thelancet.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://medlineplus.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.nhs.uk/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.ema.europa.eu/en/medicines",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.who.int/emergencies/diseases",
            recrawl: Recrawl::Weekly,
        },
    ],
};
