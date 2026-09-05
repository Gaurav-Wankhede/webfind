//! Programming & Developer Docs — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "programming",
    name: "Programming & Developer Docs",
    topics: &[
        "programming",
        "developer",
        "documentation",
        "api",
        "language",
        "rust",
        "python",
        "go",
        "javascript",
    ],
    sources: &[
        SeedSource {
            url: "https://doc.rust-lang.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://docs.python.org/3/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://go.dev/doc/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://developer.mozilla.org/en-US/docs/Web",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://nodejs.org/en/docs",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://docs.rs/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://github.blog/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://dev.to/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://stackoverflow.com/questions",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://docs.oracle.com/javase/tutorial/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://learn.microsoft.com/en-us/dotnet/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://ruby-doc.org/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.swift.org/documentation/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://docs.docker.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://kubernetes.io/docs/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://docs.aws.amazon.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://cloud.google.com/docs",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://developer.hashicorp.com/terraform/docs",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.postgresql.org/docs/",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://dev.mysql.com/doc/",
            recrawl: Recrawl::Monthly,
        },
    ],
};
