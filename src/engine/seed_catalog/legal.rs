//! Law & Regulation — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "legal",
    name: "Law & Regulation",
    topics: &["law", "legal", "regulation", "court", "statute", "policy"],
    sources: &[
        SeedSource {
            url: "https://www.supremecourt.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.law.cornell.edu/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.regulations.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.ftc.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.justice.gov/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.sec.gov/rules",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.echr.coe.int/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.icj-cij.org/",
            recrawl: Recrawl::Monthly,
        },
    ],
};
