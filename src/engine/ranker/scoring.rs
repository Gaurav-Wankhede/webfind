use super::Ranker;
use crate::schema::response::SearchResult;
use std::collections::HashMap;
use super::funnel::FRESHNESS_HALF_LIFE_DAYS;

impl Ranker {
    /// Compute domain-level authority as a combination of graph authority and institutional TLD/domain heuristics.
    pub(crate) fn domain_authority(
        graph_scores: Option<&HashMap<String, f64>>,
        results: &[SearchResult],
    ) -> ahash::AHashMap<String, f64> {
        let mut sums: ahash::AHashMap<String, (f64, usize)> = ahash::AHashMap::new();
        for r in results {
            let page_auth = graph_scores
                .and_then(|scores| scores.get(&r.url))
                .copied()
                .unwrap_or(0.0);
            let entry = sums.entry(r.domain.clone()).or_insert((0.0, 0));
            entry.0 = entry.0.max(page_auth);
            entry.1 += 1;
        }
        sums.into_iter()
            .map(|(domain, (sum, _count))| {
                let max_graph = sum;
                let institutional_boost = Self::institutional_authority_boost(&domain);
                (domain, (max_graph + institutional_boost).clamp(0.0, 1.0))
            })
            .collect()
    }

    /// Baseline authority boost based on top-level domain and verified institutional domains.
    pub fn institutional_authority_boost(domain: &str) -> f64 {
        let d = domain.to_ascii_lowercase();

        // 1. Sovereign Government, Military & Intergovernmental (.gov, .mil, .int, sovereign ccTLDs)
        if d.ends_with(".gov")
            || d.ends_with(".gov.uk")
            || d.ends_with(".mil")
            || d.ends_with(".gov.au")
            || d.ends_with(".int")
            || d.ends_with(".europa.eu")
        {
            return 0.35;
        }

        // 2. Accredited Higher Education & National Research Institutes (.edu, .ac.uk, .edu.au)
        if d.ends_with(".edu") || d.ends_with(".ac.uk") || d.ends_with(".edu.au") {
            return 0.30;
        }

        // 3. Primary Reference, Global Health, Central Banks & Scientific Repositories
        const PRIMARY_INSTITUTIONS: &[&str] = &[
            // Encyclopedias & Heritage
            "wikipedia.org", "wikimedia.org", "archive.org", "gutenberg.org", "jstor.org",
            // Physics, Math & Computer Science
            "arxiv.org", "semanticscholar.org", "ieee.org", "acm.org", "cern.ch",
            // Life Sciences & Medicine
            "nih.gov", "ncbi.nlm.nih.gov", "nature.com", "science.org", "cell.com",
            "thelancet.com", "nejm.org", "plos.org", "biorxiv.org", "medrxiv.org",
            "cochranelibrary.com", "mayoclinic.org", "who.int", "cdc.gov",
            // Academic Publishers & University Presses
            "springer.com", "wiley.com", "oup.com", "cambridge.org", "sciencedirect.com", "pnas.org", "iop.org", "acs.org",
            // Central Banking & Macroeconomics
            "federalreserve.gov", "stlouisfed.org", "ecb.europa.eu", "bankofengland.co.uk", "bis.org", "imf.org", "worldbank.org", "oecd.org",
            // Standards Organizations
            "w3.org", "ietf.org", "iso.org", "nist.gov",
        ];
        if PRIMARY_INSTITUTIONS.iter().any(|&inst| d == inst || d.ends_with(&format!(".{inst}"))) {
            return 0.28;
        }

        // 4. Recognized Primary Journalistic, Investigative & Wire Services
        const REPUTABLE_PRESS: &[&str] = &[
            // Global Wire Services
            "reuters.com", "apnews.com", "afp.com", "upi.com",
            // Investigative Non-Profits
            "propublica.org", "icij.org", "bellingcat.com", "theintercept.com",
            // Major Investigative Newspapers & Periodicals
            "bbc.com", "bbc.co.uk", "bloomberg.com", "wsj.com", "ft.com", "economist.com",
            "theguardian.com", "nytimes.com", "washingtonpost.com", "theatlantic.com",
            "newyorker.com", "foreignaffairs.com",
            // Fact-Checking & Consumer Verification
            "snopes.com", "factcheck.org", "politifact.com", "consumerreports.org",
        ];
        if REPUTABLE_PRESS.iter().any(|&press| d == press || d.ends_with(&format!(".{press}"))) {
            return 0.22;
        }

        // 5. Artisan, Culinary Science & Authority Portals
        const AUTHORITY_CRAFT: &[&str] = &[
            "seriouseats.com", "kingarthurbaking.com", "americastestkitchen.com",
        ];
        if AUTHORITY_CRAFT.iter().any(|&craft| d == craft || d.ends_with(&format!(".{craft}"))) {
            return 0.16;
        }

        // 6. Standard non-profit organizational registry (.org)
        if d.ends_with(".org") {
            return 0.08;
        }

        0.0
    }


    /// Freshness score (0.0–1.0) based on page age.
    /// Exponential decay with 180-day half-life.
    pub fn freshness_score(crawled_at: chrono::DateTime<chrono::Utc>) -> f64 {
        let age_days = (chrono::Utc::now() - crawled_at).num_days().max(0) as f64;
        (-age_days / FRESHNESS_HALF_LIFE_DAYS).exp()
    }

    /// Quality proxy from readability metrics.
    pub fn quality_score(reading_ease: f64, grade_level: f64) -> f64 {
        let ease_norm = (reading_ease / 100.0).clamp(0.0, 1.0);
        let grade_penalty = if (6.0..=12.0).contains(&grade_level) {
            1.0
        } else if grade_level < 6.0 {
            grade_level / 6.0
        } else {
            (20.0 - grade_level) / 8.0
        }
        .clamp(0.0, 1.0);

        0.7 * ease_norm + 0.3 * grade_penalty
    }
}
