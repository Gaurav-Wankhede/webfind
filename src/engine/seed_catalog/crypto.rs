//! Crypto & Blockchain — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "crypto",
    name: "Crypto & Blockchain",
    topics: &["crypto", "blockchain", "bitcoin", "defi", "web3"],
    sources: &[
        SeedSource {
            url: "https://cointelegraph.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.coindesk.com/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://ethereum.org/en/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://bitcoin.org/en/",
            recrawl: Recrawl::Monthly,
        },
    ],
};
