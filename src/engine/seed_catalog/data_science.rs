//! Data Science & Analytics — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "data-science",
    name: "Data Science & Analytics",
    topics: &["data science", "analytics", "statistics", "data", "mlops"],
    sources: &[
        SeedSource {
            url: "https://www.kaggle.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://towardsdatascience.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.datasciencecentral.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.kdnuggets.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
