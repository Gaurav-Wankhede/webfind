//! Auto-discovery of seed URLs for `webfind_research` when no seed is provided.
//!
//! Discovery is layered:
//! 1. Search the existing WebFind index for relevant URLs.
//! 2. Search the crawl graph for discovered-but-not-yet-indexed URLs.
//! 3. Query live search engines (DuckDuckGo, Bing) and fuse their results.
//!
//! No websites are hardcoded beyond the curated seed catalog. Discovery relies
//! on the existing WebFind data and live search-engine results.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};

use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::search_engine::SearchEngine;
use crate::engine::web_index::{EngineOptions, LiveIndex, extract_keywords};

/// Minimum relevance score (0.0–1.0) for an auto-discovered seed to be accepted.
const MIN_RELEVANCE: f64 = 0.55;

/// Hard floor for fallback seeds when nothing reaches `MIN_RELEVANCE`.
const FALLBACK_RELEVANCE: f64 = 0.25;

/// Maximum number of validated seeds to return.
const MAX_SEEDS: usize = 5;

/// Hard ceiling on total seed-discovery time so research requests don't hang.
/// Budget for the live seed-discovery fan-out. With 11 engines queried
/// concurrently (each with a 10s per-request timeout), 25s leaves headroom for
/// the slowest engine plus fusion.
const DISCOVERY_TIMEOUT_SECS: u64 = 25;

/// Discover high-quality seed URLs for a query.
///
/// Returns an error if no seed reaches the relevance threshold.
pub async fn discover_seeds(
    indexer: &(dyn SearchEngine + Send + Sync),
    graph: Option<&(dyn CrawlGraphStore + Send + Sync)>,
    query: &str,
) -> Result<Vec<String>> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut final_seeds: Vec<DiscoveredSeed> = Vec::new();

    // Layer 3 runs FIRST: live search engines return real, query-specific
    // URLs, so they are the primary seed source. The curated catalog, index,
    // and graph fill remaining slots as offline fallbacks — without this
    // ordering, accumulated graph memory (e.g. ruby-doc.org from an old Ruby
    // query) fills the cap and displaces live results for the current query.
    let generated = match tokio::time::timeout(
        std::time::Duration::from_secs(DISCOVERY_TIMEOUT_SECS),
        discover_from_live(query, &seen),
    )
    .await
    {
        Ok(Ok(seeds)) => seeds,
        Ok(Err(e)) => {
            tracing::warn!("live seed discovery failed: {}", e);
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(
                "live seed discovery timed out after {}s",
                DISCOVERY_TIMEOUT_SECS
            );
            Vec::new()
        }
    };
    for seed in generated {
        if seen.insert(seed.url.clone()) {
            final_seeds.push(seed);
        }
    }

    // Offline layers fill remaining slots: curated catalog (topic-matched
    // official sources), existing index, then crawl graph. Sort by relevance
    // so the most specific (catalog / URL-matched) sources win the cap.
    let mut offline: Vec<DiscoveredSeed> = Vec::new();
    offline.extend(discover_from_catalog(query));
    offline.extend(discover_from_index(indexer, query).await?);
    if let Some(graph) = graph {
        offline.extend(discover_from_graph(graph, query).await);
    }
    offline.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for seed in offline {
        if seen.insert(seed.url.clone()) {
            final_seeds.push(seed);
            if final_seeds.len() >= MAX_SEEDS {
                break;
            }
        }
    }

    // If live discovery found nothing (offline/blocked), fall back to the most
    // obvious query-derived domain candidate so the caller can still attempt a
    // crawl instead of receiving a hard "no seeds" error.
    if final_seeds.is_empty() {
        let keywords = extract_keywords(query);
        if let Some(first) = keywords.first() {
            let fallback = format!("https://{}.com", first);
            if !seen.contains(&fallback) {
                final_seeds.push(DiscoveredSeed {
                    url: fallback,
                    relevance: 0.1,
                });
            }
        }
    }

    // Prefer high-quality seeds. If none reach the quality bar, fall back to
    // the best candidates rather than erroring. The fallback seed above
    // survives this filtering so an empty index still yields a crawlable URL.
    let high_quality: Vec<DiscoveredSeed> = final_seeds
        .iter()
        .filter(|s| s.relevance >= MIN_RELEVANCE)
        .cloned()
        .collect();
    if !high_quality.is_empty() {
        final_seeds = high_quality;
    } else {
        let mut best: Vec<DiscoveredSeed> = final_seeds
            .iter()
            .filter(|s| s.relevance >= FALLBACK_RELEVANCE)
            .cloned()
            .collect();
        if !best.is_empty() {
            best.sort_by(|a, b| {
                b.relevance
                    .partial_cmp(&a.relevance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            best.truncate(MAX_SEEDS);
            final_seeds = best;
        }
        // else: keep the fallback seed(s) so the caller can still attempt a
        // crawl instead of receiving a hard "no seeds" error.
    }

    if final_seeds.is_empty() {
        anyhow::bail!(
            "No reachable seeds discovered for query '{}'. The index/graph is empty and no candidate URLs could be validated. Provide a specific seed URL.",
            query,
        );
    }

    Ok(final_seeds.into_iter().map(|s| s.url).collect())
}

#[derive(Debug, Clone)]
struct DiscoveredSeed {
    url: String,
    relevance: f64,
}

/// Layer 0: consult the curated, non-Wikipedia seed catalog. Returns topic-
/// matched authoritative source URLs (e.g. doc.rust-lang.org for "rust"),
/// ranked so sources whose URL/domain actually contains a query keyword come
/// first. Without per-source ranking every source in a matched domain scored
/// identically, so a "rust" query surfaced go.dev / nodejs / MDN before
/// doc.rust-lang.org (the reported quality bug).
fn discover_from_catalog(query: &str) -> Vec<DiscoveredSeed> {
    let keywords = extract_keywords(query);
    if keywords.is_empty() {
        return Vec::new();
    }

    let mut seeds: Vec<DiscoveredSeed> = Vec::new();
    for domain in crate::engine::seed_catalog::DOMAINS {
        // Match catalog topics against the query keywords. A topic matches a
        // keyword when one is a substring of the other OR they share a common
        // root (>=4 chars) — this lets "secure" match the "security" topic
        // while rejecting false substrings like "chain" ⊃ "ai". Substring-only
        // matching previously both missed real matches and pulled unrelated
        // domains.
        let domain_matched = domain
            .topics
            .iter()
            .any(|t| keywords.iter().any(|k| topics_match(k, t)));
        if !domain_matched {
            continue;
        }
        for source in domain.sources {
            let url = source.url.to_lowercase();
            // Score by keyword overlap with the source URL/domain: sources that
            // literally mention a query keyword (doc.rust-lang.org for "rust")
            // outrank generic-but-related sources (go.dev for "rust"). Domain
            // match alone keeps the source in the pool.
            let url_keyword_hits = keywords.iter().filter(|k| url.contains(k.as_str())).count();
            let relevance = if url_keyword_hits > 0 {
                // Strong, direct match.
                let base = 0.95 + 0.05 * ((url_keyword_hits - 1) as f64).min(1.0);
                // Reddit is community content, not primary sources — slight
                // penalty so authoritative docs (doc.rust-lang.org, etc.) win ties.
                if crate::engine::reddit::is_reddit_url(&source.url) {
                    (base - 0.03).max(0.75)
                } else {
                    base
                }
            } else {
                // Domain-related but no literal URL match: authoritative but
                // below the high-quality bar so it only wins if no direct
                // match exists.
                0.75
            };
            seeds.push(DiscoveredSeed {
                url: source.url.to_string(),
                relevance,
            });
        }
    }

    // Deduplicate by URL, keep the highest relevance.
    let mut by_url: HashMap<String, f64> = HashMap::new();
    for s in seeds {
        by_url
            .entry(s.url)
            .and_modify(|r| *r = (*r).max(s.relevance))
            .or_insert(s.relevance);
    }
    let mut seeds: Vec<DiscoveredSeed> = by_url
        .into_iter()
        .map(|(url, relevance)| DiscoveredSeed { url, relevance })
        .collect();
    // Most-relevant sources first so `truncate` keeps the best matches.
    seeds.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    seeds.truncate(MAX_SEEDS);
    seeds
}

/// Discover seeds from the existing WebFind index.
async fn discover_from_index(
    indexer: &(dyn SearchEngine + Send + Sync),
    query: &str,
) -> Result<Vec<DiscoveredSeed>> {
    let mut results = Vec::new();

    // BM25 search.
    let bm25 = indexer
        .search_bm25(query, MAX_SEEDS * 3)
        .await
        .context("index BM25 search failed")?;
    for r in bm25 {
        let relevance = (r.score / (r.score + 1.0)).clamp(0.0, 1.0);
        results.push(DiscoveredSeed {
            url: r.url,
            relevance,
        });
    }

    // Optional vector search.
    let vector = indexer
        .search_vector(query, MAX_SEEDS * 3)
        .await
        .unwrap_or_default();
    for (url, score) in vector {
        let relevance = (score / (score + 1.0)).clamp(0.0, 1.0);
        results.push(DiscoveredSeed { url, relevance });
    }

    // Deduplicate by URL, keeping highest relevance.
    let mut by_url: HashMap<String, DiscoveredSeed> = HashMap::new();
    for seed in results {
        by_url
            .entry(seed.url.clone())
            .and_modify(|e| {
                if seed.relevance > e.relevance {
                    *e = seed.clone();
                }
            })
            .or_insert(seed);
    }

    let mut seeds: Vec<DiscoveredSeed> = by_url.into_values().collect();
    seeds.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    seeds.truncate(MAX_SEEDS * 2);
    Ok(seeds)
}

/// Discover seeds from the crawl graph (URLs already seen but not necessarily indexed).
async fn discover_from_graph(graph: &dyn CrawlGraphStore, query: &str) -> Vec<DiscoveredSeed> {
    let keywords = extract_keywords(query);
    let nodes = graph.get_urls().await;
    let links = graph.get_all_links().await;

    // Build inbound-link counts and the anchor-text index in one pass over the
    // edges. Looking up per-node anchor text by scanning all links would be
    // O(nodes × edges) — minutes on a large graph.
    let mut inbound: HashMap<String, usize> = HashMap::new();
    let mut anchors: HashMap<String, String> = HashMap::new();
    for edge in &links {
        *inbound.entry(edge.to.clone()).or_insert(0) += 1;
        if let Some(text) = &edge.anchor_text {
            anchors
                .entry(edge.to.clone())
                .and_modify(|t| {
                    t.push(' ');
                    t.push_str(text);
                })
                .or_insert_with(|| text.clone());
        }
    }

    let mut seeds: Vec<DiscoveredSeed> = nodes
        .into_iter()
        .filter_map(|node| {
            let anchor_text = anchors.get(&node.url).map(String::as_str).unwrap_or("");
            let text = format!("{} {} {}", node.url, node.domain, anchor_text).to_lowercase();
            let relevance = keyword_relevance(&keywords, &text);
            if relevance > 0.0 {
                let authority =
                    (*inbound.get(&node.url).unwrap_or(&0) as f64) / (links.len().max(1) as f64);
                let combined = (relevance * 0.7 + authority * 0.3).clamp(0.0, 1.0);
                Some(DiscoveredSeed {
                    url: node.url,
                    relevance: combined,
                })
            } else {
                None
            }
        })
        .collect();

    seeds.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    seeds.truncate(MAX_SEEDS * 2);
    seeds
}

/// Discover seeds by querying live search engines and fusing their results.
///
/// Replaces the old query→domain-guess→DNS-validate pipeline: search engines
/// already return real, query-relevant URLs, so the crawler starts at relevant
/// pages instead of guessed domains. Relevance blends keyword matches from the
/// title/snippet with the fused cross-engine score.
async fn discover_from_live(query: &str, exclude: &HashSet<String>) -> Result<Vec<DiscoveredSeed>> {
    let keywords = extract_keywords(query);
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let index = LiveIndex::new().context("failed to build live search index")?;
    let opts = EngineOptions {
        max_results: 10,
        ..EngineOptions::default()
    };
    let outcome = index.search(query, &opts).await;
    if outcome.engines_failed > 0 {
        tracing::warn!(
            "live seed discovery: {}/{} engines failed",
            outcome.engines_failed,
            outcome.total_engines
        );
    }
    if outcome.fused.is_empty() {
        return Ok(Vec::new());
    }

    let mut seeds: Vec<DiscoveredSeed> = Vec::new();
    for hit in &outcome.fused {
        if exclude.contains(&hit.url) {
            continue;
        }
        let relevance =
            live_hit_relevance(&keywords, &hit.title, &hit.snippet, hit.score, opts.rrf_k);
        if relevance <= 0.0 {
            continue;
        }
        seeds.push(DiscoveredSeed {
            url: hit.url.clone(),
            relevance,
        });
    }

    seeds.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    seeds.truncate(MAX_SEEDS);
    Ok(seeds)
}

/// Relevance of a live search hit: keyword matches in the title/snippet
/// dominate, and the normalized fused score (cross-engine agreement) boosts.
fn live_hit_relevance(
    keywords: &[String],
    title: &str,
    snippet: &str,
    fused_score: f64,
    rrf_k: u32,
) -> f64 {
    let text = format!("{} {}", title, snippet).to_lowercase();
    let keyword_rel = keyword_relevance(keywords, &text);
    if keyword_rel <= 0.0 {
        return 0.0;
    }
    // Normalize the fused RRF score to 0..1: a rank-1 single-engine hit scores
    // 1/(k+1), so multiplying by k maps it near 1.0; cross-engine agreement
    // pushes it past 1.0 and is clamped.
    let fused_norm = (fused_score * f64::from(rrf_k.max(1))).clamp(0.0, 1.0);
    (keyword_rel * 0.8 + fused_norm * 0.2).clamp(0.0, 1.0)
}

/// Whether a query keyword matches a catalog topic.
///
/// A match is a direct substring either way OR a shared morphological root of
/// at least 4 chars, so "secure" matches the "security" topic without "chain"
/// matching the "ai" topic (a false substring). All inputs are lowercase.
fn topics_match(keyword: &str, topic: &str) -> bool {
    if keyword.contains(topic) || topic.contains(keyword) {
        return true;
    }
    // Shared root of >=4 chars (e.g. "secure" / "security" → "secur").
    let min = keyword.len().min(topic.len());
    let prefix = (0..min)
        .take_while(|&i| keyword.as_bytes()[i] == topic.as_bytes()[i])
        .count();
    prefix >= 4
}

/// Compute keyword relevance score for a text blob.
fn keyword_relevance(keywords: &[String], text: &str) -> f64 {
    if keywords.is_empty() {
        return 0.0;
    }
    let text_lower = text.to_lowercase();
    let matches = keywords
        .iter()
        .filter(|k| text_lower.contains(&k.to_lowercase()))
        .count();
    (matches as f64 / keywords.len() as f64).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_keywords() {
        let kw = extract_keywords("What is the latest Rust async runtime in 2026?");
        assert!(kw.contains(&"rust".to_string()));
        assert!(kw.contains(&"async".to_string()));
        assert!(kw.contains(&"runtime".to_string()));
    }

    #[test]
    fn test_live_hit_relevance_blends_keywords_and_fusion() {
        let keywords = vec![
            "rust".to_string(),
            "async".to_string(),
            "runtime".to_string(),
        ];
        // Title+snippet match all keywords → clears the quality bar.
        let high = live_hit_relevance(
            &keywords,
            "Rust async runtime",
            "Tokio is a Rust async runtime",
            0.0328, // shared rank-1 across two engines
            60,
        );
        assert!(
            high >= 0.55,
            "full keyword match must clear the bar: {high}"
        );
        // No keyword match → zero, regardless of fused score.
        let zero = live_hit_relevance(&keywords, "Cooking recipes", "Pasta and sauce", 0.5, 60);
        assert_eq!(zero, 0.0);
        // Cross-engine agreement boosts over a single-engine hit.
        let shared = live_hit_relevance(&keywords, "Rust async", "runtime", 0.0328, 60);
        let single = live_hit_relevance(&keywords, "Rust async", "runtime", 0.0164, 60);
        assert!(shared > single);
    }

    #[test]
    fn test_discover_from_catalog_prefers_authoritative_sources() {
        // "rust" must resolve to curated official docs, not a domain guess.
        let seeds = discover_from_catalog("rust programming language 2026");
        assert!(
            !seeds.is_empty(),
            "catalog should return seeds for a programming query"
        );
        assert!(
            seeds.iter().any(|s| s.url.contains("rust")),
            "expected a rust-specific authoritative source, got: {:?}",
            seeds
        );
        // A rust-specific source must rank first: doc.rust-lang.org outranks
        // generic programming sources because its URL contains "rust".
        assert!(
            seeds
                .first()
                .map(|s| s.url.contains("rust"))
                .unwrap_or(false),
            "rust-specific source should be first, got: {:?}",
            seeds.first()
        );
        assert!(
            seeds.iter().all(|s| s.url.starts_with("https://")),
            "all catalog seeds must be https"
        );
        // Catalog seeds are authoritative: above the high-quality bar.
        assert!(
            seeds.iter().all(|s| s.relevance >= MIN_RELEVANCE),
            "catalog seeds must clear the high-quality threshold"
        );
    }

    #[test]
    fn test_discover_from_catalog_ranks_exact_url_matches_first() {
        // For a rust query, doc.rust-lang.org (URL contains "rust") must be
        // ranked above unrelated-but-programming sources like go.dev or MDN.
        let seeds = discover_from_catalog("rust async runtime tokio");
        assert!(
            seeds
                .first()
                .map(|s| s.url.contains("rust"))
                .unwrap_or(false),
            "rust URL match must rank first: {:?}",
            seeds.first()
        );
    }

    #[test]
    fn test_discover_from_catalog_ignores_unrelated_topics() {
        // A finance-only query must not pull programming docs.
        let seeds = discover_from_catalog("interest rate central bank");
        assert!(
            seeds.iter().all(|s| !s.url.contains("rust.org")),
            "unrelated catalog domains should not match"
        );
    }

    #[test]
    fn test_discover_from_catalog_catches_security_query() {
        // A supply-chain / security query must resolve to cybersecurity
        // sources, not off-target AI/blog seeds.
        let seeds = discover_from_catalog("secure software supply chain practices 2026");
        assert!(
            seeds.iter().any(|s| s.url.contains("securelist")
                || s.url.contains("krebsonsecurity")
                || s.url.contains("nvd.nist.gov")
                || s.url.contains("cloudflare.com")
                || s.url.contains("mandiant")),
            "expected cybersecurity sources for a security query, got: {:?}",
            seeds
        );
    }
}
