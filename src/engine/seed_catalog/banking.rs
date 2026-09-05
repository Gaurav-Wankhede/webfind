//! Banking & Central Banks — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "banking",
    name: "Banking & Central Banks",
    topics: &[
        "banking",
        "central bank",
        "monetary",
        "regulation",
        "interest",
        "bank",
    ],
    sources: &[
        SeedSource {
            url: "https://www.bis.org/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.fdic.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.ecb.europa.eu/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.occ.gov/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.federalreserve.gov/bankinforeg/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.bankofengland.co.uk/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.bankofjapan.co.jp/en/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.rbi.org.in/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.imf.org/en/Topics/financial-sector",
            recrawl: Recrawl::Weekly,
        },
    ],
};
