//! Auto-discovery of seed URLs for `webfind_research` when no seed is provided.
//!
//! Discovery is layered:
//! 1. Search the existing WebFind index for relevant URLs.
//! 2. Search the crawl graph for discovered-but-not-yet-indexed URLs.
//! 3. Generate query-driven domain candidates, validate them, and score relevance.
//!
//! No websites are hardcoded. Discovery relies only on the existing WebFind data
//! and URL patterns derived from the query itself.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use tokio::net::lookup_host;
use tokio::sync::Semaphore;

use crate::engine::crawl_graph::CrawlGraphStore;
use crate::engine::fetcher::Fetcher;
use crate::engine::search_engine::SearchEngine;
use crate::schema::content::StructuredContent;

/// Minimum relevance score (0.0–1.0) for an auto-discovered seed to be accepted.
const MIN_RELEVANCE: f64 = 0.55;

/// Hard floor for fallback seeds when nothing reaches `MIN_RELEVANCE`.
const FALLBACK_RELEVANCE: f64 = 0.25;

/// Maximum number of validated seeds to return.
const MAX_SEEDS: usize = 5;

/// Maximum number of candidate URLs to generate from a query.
const MAX_CANDIDATES: usize = 20;

/// DNS resolution timeout per candidate.
const DNS_TIMEOUT_SECS: u64 = 2;

/// HTTP validation timeout per candidate.
const HTTP_TIMEOUT_SECS: u64 = 2;

/// Maximum parallel validation tasks during discovery.
const DISCOVERY_CONCURRENCY: usize = 8;

/// Hard ceiling on total seed-discovery time so research requests don't hang.
const DISCOVERY_TIMEOUT_SECS: u64 = 12;

/// TLDs to try, in priority order.
const TLDS: &[&str] = &[
    "com", "org", "io", "ai", "dev", "app", "net", "co", "tech", "software", "tools", "blog",
    "news", "info", "xyz", "me", "sh", "so", "to", "us", "eu", "in",
];

/// Stop words removed from query keywords.
const STOP_WORDS: &[&str] = &[
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "in",
    "on",
    "at",
    "to",
    "for",
    "of",
    "with",
    "by",
    "from",
    "as",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "must",
    "shall",
    "can",
    "need",
    "dare",
    "ought",
    "used",
    "this",
    "that",
    "these",
    "those",
    "i",
    "you",
    "he",
    "she",
    "it",
    "we",
    "they",
    "what",
    "which",
    "who",
    "when",
    "where",
    "why",
    "how",
    "all",
    "any",
    "both",
    "each",
    "few",
    "more",
    "most",
    "other",
    "some",
    "such",
    "no",
    "nor",
    "not",
    "only",
    "own",
    "same",
    "so",
    "than",
    "too",
    "very",
    "just",
    "now",
    "then",
    "also",
    "about",
    "up",
    "out",
    "if",
    "because",
    "until",
    "while",
    "during",
    "before",
    "after",
    "above",
    "below",
    "between",
    "into",
    "through",
    "over",
    "under",
    "again",
    "further",
    "once",
    "here",
    "there",
    "everywhere",
    "anywhere",
    "somewhere",
    "get",
    "me",
    "my",
    "your",
    "his",
    "her",
    "its",
    "our",
    "their",
    "what's",
    "how's",
    "where's",
    "who's",
    "when's",
    "why's",
    "latest",
    "new",
    "best",
    "top",
    "guide",
    "overview",
    "introduction",
    "vs",
    "versus",
    "compare",
    "comparison",
    "difference",
    "between",
    "2020",
    "2021",
    "2022",
    "2023",
    "2024",
    "2025",
    "2026",
    "2027",
];

/// Discover high-quality seed URLs for a query.
///
/// Returns an error if no seed reaches the relevance threshold.
pub async fn discover_seeds(
    indexer: &(dyn SearchEngine + Send + Sync),
    graph: Option<&(dyn CrawlGraphStore + Send + Sync)>,
    query: &str,
) -> Result<Vec<String>> {
    let mut seeds: Vec<DiscoveredSeed> = Vec::new();

    // Layer 1: existing index.
    seeds.extend(discover_from_index(indexer, query).await?);

    // Layer 2: crawl graph.
    if let Some(graph) = graph {
        seeds.extend(discover_from_graph(graph, query).await);
    }

    // If we already have enough quality seeds from existing data, return them.
    let mut seen: HashSet<String> = seeds.iter().map(|s| s.url.clone()).collect();
    let mut final_seeds: Vec<DiscoveredSeed> = seeds
        .iter()
        .filter(|s| s.relevance >= MIN_RELEVANCE)
        .take(MAX_SEEDS)
        .cloned()
        .collect();

    // Layer 3: query-driven domain inference with a hard time ceiling.
    let generated = match tokio::time::timeout(
        std::time::Duration::from_secs(DISCOVERY_TIMEOUT_SECS),
        discover_from_query(query, &seen),
    )
    .await
    {
        Ok(Ok(seeds)) => seeds,
        Ok(Err(e)) => {
            tracing::warn!("query-driven seed discovery failed: {}", e);
            Vec::new()
        }
        Err(_) => {
            tracing::warn!(
                "query-driven seed discovery timed out after {}s",
                DISCOVERY_TIMEOUT_SECS
            );
            Vec::new()
        }
    };

    // If discovery timed out or found nothing, fall back to the most obvious
    // query-derived domain candidate so the caller can still attempt a crawl.
    if generated.is_empty() {
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
    } else {
        // Merge query-derived candidates into the seed pool (dedup by URL).
        for seed in generated {
            if seen.insert(seed.url.clone()) {
                final_seeds.push(seed);
                if final_seeds.len() >= MAX_SEEDS {
                    break;
                }
            }
        }
    }

    // Prefer high-quality seeds. If none reach the quality bar, fall back to
    // the best query-derived candidates rather than erroring. The fallback
    // seed above survives this filtering so an empty index still yields a
    // crawlable URL.
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

    // Build inbound-link counts.
    let mut inbound: HashMap<String, usize> = HashMap::new();
    for edge in &links {
        *inbound.entry(edge.to.clone()).or_insert(0) += 1;
    }

    let mut seeds: Vec<DiscoveredSeed> = nodes
        .into_iter()
        .filter_map(|node| {
            let text = format!(
                "{} {} {}",
                node.url,
                node.domain,
                anchor_text_for_url(&links, &node.url)
            )
            .to_lowercase();
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

fn anchor_text_for_url(links: &[crate::engine::crawl_graph::LinkEdge], url: &str) -> String {
    links
        .iter()
        .filter(|e| e.to == url)
        .filter_map(|e| e.anchor_text.clone())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Discover seeds by generating URL candidates from the query and validating them.
async fn discover_from_query(
    query: &str,
    exclude: &HashSet<String>,
) -> Result<Vec<DiscoveredSeed>> {
    let keywords = extract_keywords(query);
    if keywords.is_empty() {
        return Ok(Vec::new());
    }

    let candidates: Vec<String> = generate_candidates(&keywords)
        .into_iter()
        .filter(|u| !exclude.contains(u))
        .collect();

    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(HTTP_TIMEOUT_SECS))
        .redirect(reqwest::redirect::Policy::limited(2))
        .gzip(true)
        .build()
        .context("failed to build validation client")?;
    let client = Arc::new(client);
    let keywords = Arc::new(keywords);

    let semaphore = Arc::new(Semaphore::new(DISCOVERY_CONCURRENCY));
    let seen = Arc::new(tokio::sync::Mutex::new(HashSet::<String>::new()));

    let mut tasks = Vec::with_capacity(candidates.len());
    for url in candidates {
        let client = client.clone();
        let keywords = keywords.clone();
        let semaphore = semaphore.clone();
        let seen = seen.clone();
        tasks.push(tokio::spawn(async move {
            let _permit = semaphore.acquire().await.ok()?;
            // Deduplicate inside the worker too.
            {
                let mut guard = seen.lock().await;
                if !guard.insert(url.clone()) {
                    return None;
                }
            }
            let host = extract_host(&url)?;
            if !dns_resolves(&host).await {
                return None;
            }
            let fetcher = Fetcher::from_client((*client).clone()).ok()?;
            validate_url(&fetcher, &url, &keywords).await
        }));
    }

    let mut validated: Vec<DiscoveredSeed> = Vec::new();
    for task in tasks {
        if validated.len() >= MAX_SEEDS {
            break;
        }
        if let Ok(Some(seed)) = task.await {
            validated.push(seed);
        }
    }

    validated.sort_by(|a, b| {
        b.relevance
            .partial_cmp(&a.relevance)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    validated.truncate(MAX_SEEDS);
    Ok(validated)
}

/// Validate a candidate URL by fetching it and computing relevance to the query.
async fn validate_url(fetcher: &Fetcher, url: &str, keywords: &[String]) -> Option<DiscoveredSeed> {
    let content = fetcher.fetch_url(url).await.ok()?;

    if !content.is_valid_content {
        return None;
    }

    let relevance = compute_page_relevance(&content, keywords);
    Some(DiscoveredSeed {
        url: content.final_url,
        relevance,
    })
}

/// Compute relevance of extracted content to query keywords (0.0–1.0).
fn compute_page_relevance(content: &StructuredContent, keywords: &[String]) -> f64 {
    let title = content.title.to_lowercase();
    let excerpt = content.excerpt.to_lowercase();
    let description = content.description.as_deref().unwrap_or("").to_lowercase();
    let text = content.content_text.to_lowercase();

    let title_score = keyword_density(&title, keywords) * 0.35;
    let excerpt_score = keyword_density(&excerpt, keywords) * 0.25;
    let description_score = keyword_density(&description, keywords) * 0.15;
    let body_score = keyword_density(&text, keywords) * 0.20;

    let mut score = (title_score + excerpt_score + description_score + body_score).clamp(0.0, 1.0);

    // Penalize very short or suspicious pages.
    if content.word_count < 50 {
        score *= 0.5;
    }
    if is_parked_or_generic(&content) {
        score *= 0.1;
    }

    score
}

/// Heuristic detection of parked/generic pages.
fn is_parked_or_generic(content: &StructuredContent) -> bool {
    let title_lower = content.title.to_lowercase();
    let excerpt_lower = content.excerpt.to_lowercase();
    let indicators = [
        "domain for sale",
        "buy this domain",
        "parked free",
        "coming soon",
        "under construction",
        "403 forbidden",
        "404 not found",
        "503 service unavailable",
    ];
    indicators
        .iter()
        .any(|i| title_lower.contains(i) || excerpt_lower.contains(i))
}

/// Generate URL candidates from keywords.
///
/// Candidates are interleaved by TLD so that the most important top-level
/// domains (com, org, io, ai, ...) get both single-keyword and multi-word
/// phrase coverage before falling back to less common TLDs. Generation stops
/// at `MAX_CANDIDATES`.
fn generate_candidates(keywords: &[String]) -> Vec<String> {
    let mut candidates: Vec<String> = Vec::with_capacity(MAX_CANDIDATES);

    // Most productive patterns; avoid combinatorial explosion.
    const PHRASE_PATTERNS: &[&str] = &[
        "https://{k}.{tld}",
        "https://www.{k}.{tld}",
        "https://get{k}.{tld}",
        "https://the{k}.{tld}",
    ];

    let single: Vec<String> = keywords.iter().take(4).cloned().collect();
    let pairs: Vec<String> = keywords
        .windows(2)
        .take(3)
        .flat_map(|w| vec![w.join("-"), w.join("")])
        .collect();
    let triple: Option<String> = if keywords.len() >= 3 {
        Some(keywords[..3].join("-"))
    } else {
        None
    };

    for tld in TLDS {
        for phrase in &single {
            for pattern in PHRASE_PATTERNS {
                add_url(&mut candidates, pattern, phrase, tld);
            }
        }
        for phrase in &pairs {
            for pattern in PHRASE_PATTERNS {
                add_url(&mut candidates, pattern, phrase, tld);
            }
        }
        if let Some(phrase) = &triple {
            for pattern in PHRASE_PATTERNS {
                add_url(&mut candidates, pattern, phrase, tld);
            }
        }

        if candidates.len() >= MAX_CANDIDATES {
            break;
        }
    }

    candidates.truncate(MAX_CANDIDATES);
    candidates
}

fn add_url(candidates: &mut Vec<String>, pattern: &str, phrase: &str, tld: &str) {
    let normalized = phrase
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_lowercase();
    if normalized.is_empty() || normalized.len() > 40 {
        return;
    }
    let url = pattern.replace("{k}", &normalized).replace("{tld}", tld);
    candidates.push(url);
}

/// Extract informative keywords from a query.
fn extract_keywords(query: &str) -> Vec<String> {
    let stop_set: HashSet<&str> = STOP_WORDS.iter().copied().collect();
    let mut keywords: Vec<String> = query
        .to_lowercase()
        .split_whitespace()
        .map(|s| s.trim_matches(|c: char| !c.is_alphanumeric()))
        .filter(|s| !s.is_empty() && !stop_set.contains(s) && s.len() > 1)
        .map(|s| s.to_string())
        .collect();

    // Deduplicate while preserving order.
    let mut seen = HashSet::new();
    keywords.retain(|k| seen.insert(k.clone()));
    keywords.truncate(6);
    keywords
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

/// Compute keyword density (matches weighted by position/title).
fn keyword_density(text: &str, keywords: &[String]) -> f64 {
    if keywords.is_empty() || text.is_empty() {
        return 0.0;
    }
    let text_lower = text.to_lowercase();
    let words: Vec<&str> = text_lower.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }

    let mut matched_words = 0usize;
    let mut keyword_hits = 0usize;
    for word in &words {
        for kw in keywords {
            if word.contains(kw) || kw.contains(word) {
                matched_words += 1;
                keyword_hits += 1;
                break;
            }
        }
    }

    let density = matched_words as f64 / words.len() as f64;
    let hit_rate = keyword_hits as f64 / keywords.len() as f64;

    (density * 2.0 + hit_rate * 0.5).clamp(0.0, 1.0)
}

/// Check if a hostname resolves via DNS.
async fn dns_resolves(host: &str) -> bool {
    let host = host.to_string();
    let timeout = tokio::time::Duration::from_secs(DNS_TIMEOUT_SECS);
    match tokio::time::timeout(timeout, lookup_host(format!("{}:80", host))).await {
        Ok(Ok(mut iter)) => iter.next().is_some(),
        _ => false,
    }
}

/// Extract host from URL.
fn extract_host(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .map(|u| u.host_str().unwrap_or("").to_string())
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
    fn test_generate_candidates() {
        let candidates = generate_candidates(&["rust".to_string(), "async".to_string()]);
        assert!(candidates.contains(&"https://rust.com".to_string()));
        assert!(candidates.contains(&"https://rust-async.com".to_string()));
    }

    #[test]
    fn test_compute_page_relevance() {
        let content = StructuredContent {
            url: "https://rust-lang.org".to_string(),
            final_url: "https://rust-lang.org".to_string(),
            status_code: 200,
            title: "Rust Programming Language".to_string(),
            description: Some(
                "A language empowering everyone to build reliable software.".to_string(),
            ),
            canonical_url: None,
            language: "en".to_string(),
            language_confidence: 0.95,
            published_at: None,
            modified_at: None,
            author: None,
            site_name: None,
            content_text: "Rust is a systems programming language.".to_string(),
            content_html: "<p>Rust</p>".to_string(),
            content_markdown: "Rust".to_string(),
            excerpt: "Rust is a systems programming language.".to_string(),
            word_count: 100,
            char_count: 100,
            sentence_count: 5,
            reading_time_seconds: 30,
            reading_ease: 60.0,
            grade_level: 8.0,
            keywords: vec![],
            open_graph: None,
            twitter_card: None,
            json_ld: vec![],
            schema_type: None,
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: "Rust".to_string(),
            fetched_at: chrono::Utc::now(),
            fetch_duration_ms: 100,
            html_size_bytes: 1000,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            is_paywalled: false,
            is_valid_content: true,
            content_type: "text/html".to_string(),
            content_type_header: "text/html".to_string(),
            entities: crate::schema::content::Entities::default(),
        };
        let relevance = compute_page_relevance(&content, &["rust".to_string()]);
        assert!(relevance > 0.5, "relevance should be high for Rust content");
    }
}
