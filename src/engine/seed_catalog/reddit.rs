//! Reddit communities — curated seed catalog for background crawling.
//!
//! 100 high-value subreddits organized across 16 categories, sourced from the
//! "Reddit Marketing Mastery" guide.  Each subreddit is a `SeedSource` with a
//! recrawl cadence appropriate to its content velocity.
//!
//! Design principle: treat Reddit as a genuine person.  Descriptive UA,
//! respectful rate limits, no spam.

use super::{CuratedDomain, Recrawl, SeedSource};

pub const DOMAIN: CuratedDomain = CuratedDomain {
    slug: "reddit",
    name: "Reddit Communities",
    topics: &[
        "business",
        "technology",
        "finance",
        "health",
        "science",
        "food",
        "education",
        "beauty",
        "home improvement",
        "fashion",
        "programming",
        "gaming",
        "travel",
        "careers",
        "art",
        "music",
    ],
    sources: &[
        // ── Business (10) ─────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/smallbusiness",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Entrepreneur",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/startups",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/BusinessHub",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/marketing",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/AccountingDepartment",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/business",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/sales",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/ecommerce",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/freelance",
            recrawl: Recrawl::Weekly,
        },
        // ── Technology (6) ────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/QuantumComputing",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/ArtificialIntelligence",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/TechNews",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/CyberSecurity",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Gadgets",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/ExperiencedDevs",
            recrawl: Recrawl::Weekly,
        },
        // ── Finance (6) ───────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/personalfinance",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/FinancialPlanning",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/tax",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/retirement",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Investing",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Economics",
            recrawl: Recrawl::Daily,
        },
        // ── Health & Fitness (6) ──────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/nutrition",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/weightlifting",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/fitness",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/yoga",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/CrossFit",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/bodyweightfitness",
            recrawl: Recrawl::Weekly,
        },
        // ── Science (6) ───────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/askscience",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/futurology",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/science",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/chemistry",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/biology",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/physics",
            recrawl: Recrawl::Weekly,
        },
        // ── Food & Cooking (6) ────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/Cooking",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/askbaking",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/askculinary",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/baking",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/cookingforbeginners",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/seriouseats",
            recrawl: Recrawl::Weekly,
        },
        // ── Education (6) ─────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/history",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/askmath",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/philosophy",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/statistics",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/explainlikeimfive",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/education",
            recrawl: Recrawl::Weekly,
        },
        // ── Beauty (6) ────────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/NaturalBeauty",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/MakeupAddiction",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/indiemakeupandmore",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/BrownBeauty",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/AsianBeauty",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/SkinCareAddiction",
            recrawl: Recrawl::Daily,
        },
        // ── Home Improvement (6) ──────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/HomeImprovement",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/DIY",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/fixit",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/plumbing",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/HomeDecorating",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Home",
            recrawl: Recrawl::Weekly,
        },
        // ── Fashion (6) ───────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/femalefashionadvice",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/malefashionadvice",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/OUTFITS",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/streetwear",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/fashionadvice",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/FashionPlus",
            recrawl: Recrawl::Weekly,
        },
        // ── Programming (6) ───────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/learnprogramming",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/java",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/Python",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/cpp",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/cscareerquestions",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/coding",
            recrawl: Recrawl::Weekly,
        },
        // ── Gaming (6) ────────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/gaming",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/gamingsuggestions",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/RPG_gamers",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/gamernews",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/AskGames",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/patientgamers",
            recrawl: Recrawl::Weekly,
        },
        // ── Travel (6) ────────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/travel",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/travelhacks",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/wanderlust",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/backpacking",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/solotravel",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/roadtrip",
            recrawl: Recrawl::Weekly,
        },
        // ── Careers (6) ───────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/wfh",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/jobs",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/careerguidance",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/remotework",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/productivity",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/careeradvice",
            recrawl: Recrawl::Daily,
        },
        // ── Art (6) ───────────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/Art",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/alternativeart",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/artcrit",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/artstore",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/artcollecting",
            recrawl: Recrawl::Monthly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/AdorableArt",
            recrawl: Recrawl::Weekly,
        },
        // ── Music (6) ─────────────────────────────────────────────────
        SeedSource {
            url: "https://www.reddit.com/r/Music",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/popheads",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/indieheads",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/hiphopheads",
            recrawl: Recrawl::Daily,
        },
        SeedSource {
            url: "https://www.reddit.com/r/listentothis",
            recrawl: Recrawl::Weekly,
        },
        SeedSource {
            url: "https://www.reddit.com/r/LetsTalkMusic",
            recrawl: Recrawl::Weekly,
        },
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reddit_domain_has_100_sources() {
        assert_eq!(
            DOMAIN.sources.len(),
            100,
            "Reddit domain must have exactly 100 subreddits, got {}",
            DOMAIN.sources.len()
        );
    }

    #[test]
    fn test_reddit_domain_slug() {
        assert_eq!(DOMAIN.slug, "reddit");
    }

    #[test]
    fn test_all_sources_are_reddit() {
        for src in DOMAIN.sources {
            assert!(
                crate::engine::reddit::is_reddit_url(src.url),
                "source URL is not Reddit: {}",
                src.url
            );
        }
    }

    #[test]
    fn test_all_sources_are_https() {
        for src in DOMAIN.sources {
            assert!(
                src.url.starts_with("https://"),
                "source must be HTTPS: {}",
                src.url
            );
        }
    }

    #[test]
    fn test_recrawl_variety() {
        let has_daily = DOMAIN.sources.iter().any(|s| s.recrawl == Recrawl::Daily);
        let has_weekly = DOMAIN.sources.iter().any(|s| s.recrawl == Recrawl::Weekly);
        let has_monthly = DOMAIN.sources.iter().any(|s| s.recrawl == Recrawl::Monthly);
        assert!(has_daily, "must have at least one daily source");
        assert!(has_weekly, "must have at least one weekly source");
        assert!(has_monthly, "must have at least one monthly source");
    }
}
