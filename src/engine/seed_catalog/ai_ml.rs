//! AI & Machine Learning — curated official sources (non-Wikipedia).

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "ai-ml",
    name: "AI & Machine Learning",
    topics: &[
        "ai",
        "machine learning",
        "llm",
        "deep learning",
        "model",
        "neural network",
    ],
    sources: &[
        SeedSource {
            url: "https://paperswithcode.com/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://huggingface.co/blog",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.deeplearning.ai/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://openai.com/blog/",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://ai.meta.com/blog/",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.fast.ai/",
            recrawl: Recrawl::Weekly,
        },
    ],
};
