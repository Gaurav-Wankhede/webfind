//! Finance & Markets — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "finance",
    name: "Finance & Markets",
    topics: &[
        "finance",
        "market",
        "stock",
        "economy",
        "investing",
        "equities",
        "fed",
    ],
    sources: &[
        SeedSource {
            url: "https://www.federalreserve.gov/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.sec.gov/newsroom",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.imf.org/en/News",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.worldbank.org/en/news",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.nasdaq.com/market-activity",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.bloomberg.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reuters.com/markets/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.cnbc.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://finance.yahoo.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.investopedia.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.marketwatch.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.ft.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://seekingalpha.com/",
            recrawl: Recrawl::Daily,
        },
    ],
};
