// webfind-trainer: Physical socket probe harvester querying webfind.db and live domains.
// Probes across the full metadata extraction pyramid:
// Tier 1: /llms.txt, /llms-full.txt, .well-known/ai-catalog.json
// Tier 2: /sitemap.xml, /robots.txt, /openapi.json
// Tier 3: JSON-LD and OpenGraph embedded metadata

use crate::dataset::ProbeItem;
use anyhow::{Context, Result};
use reqwest::Client;
use rusqlite::Connection;
use std::time::Instant;
use url::Url;

/// Physical multi-tier probe result for a single domain.
#[derive(Debug, Clone)]
pub struct HarvesterProbeResult {
    pub domain: String,
    // Tier 1: Machine Manifests
    pub has_full: bool,
    pub has_txt: bool,
    pub has_catalog: bool,
    // Tier 2: Site Topology & API Contracts
    pub has_sitemap: bool,
    pub has_robots: bool,
    pub has_openapi: bool,
    pub has_rss: bool,
    // Tier 3: Embedded Head Semantic Metadata
    pub has_json_ld: bool,
    pub has_open_graph: bool,
    // Operational Network Metrics
    pub is_doc_subdomain: bool,
    pub requires_js: bool,
    pub bot_challenge: bool,
    pub latency_ms: f32,
    pub quality_prior: f32,
    pub protocol_class: usize,
}

/// Harvest unique domain candidates from local webfind.db url_nodes table.
pub fn fetch_domains_from_db(db_path: &str, limit: usize) -> Result<Vec<String>> {
    let conn = Connection::open(db_path)
        .with_context(|| format!("Failed to open database at {db_path}"))?;

    let mut stmt = conn.prepare(
        "SELECT DISTINCT domain FROM url_nodes 
         WHERE domain != '' AND domain NOT LIKE '%.local' 
         ORDER BY priority DESC LIMIT ?1",
    )?;

    let domain_iter = stmt.query_map([limit as i64], |row| row.get::<_, String>(0))?;
    let mut domains = Vec::new();
    for d in domain_iter {
        domains.push(d?);
    }
    Ok(domains)
}

/// Execute a live physical socket probe on a domain across all 5 extraction tiers.
pub async fn probe_domain(client: &Client, domain: &str) -> Option<HarvesterProbeResult> {
    let base_url = if domain.starts_with("http://") || domain.starts_with("https://") {
        domain.to_string()
    } else {
        format!("https://{domain}")
    };

    let start = Instant::now();
    let root_resp = client.get(&base_url).send().await.ok()?;
    let latency_ms = start.elapsed().as_millis() as f32;

    let status = root_resp.status().as_u16();
    let headers = root_resp.headers();

    // Check bot challenges (Cloudflare Turnstile, AWS WAF, 403/429)
    let bot_challenge = status == 403
        || status == 429
        || headers.contains_key("cf-mitigated")
        || headers.contains_key("x-amzn-waf-action");

    let is_doc_subdomain = domain.starts_with("docs.")
        || domain.starts_with("doc.")
        || domain.contains("documentation")
        || domain.contains("api.");

    // Inspect first chunk / body for Tier 3 embedded metadata (JSON-LD, OpenGraph)
    let body_sample = root_resp.text().await.unwrap_or_default();
    let has_json_ld = body_sample.contains("application/ld+json");
    let has_open_graph = body_sample.contains("og:title") || body_sample.contains("og:description");
    let requires_js = body_sample.contains("You need to enable JavaScript")
        || body_sample.contains("__NEXT_DATA__")
        || body_sample.contains("window.__INITIAL_STATE__");

    // Tier 1 Probes: Machine Manifests
    let llms_txt_url = format!("{base_url}/llms.txt");
    let llms_full_url = format!("{base_url}/llms-full.txt");
    let ai_catalog_url = format!("{base_url}/.well-known/ai-catalog.json");

    let has_txt = client
        .head(&llms_txt_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let has_full = if has_txt {
        client
            .head(&llms_full_url)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    } else {
        false
    };

    let has_catalog = client
        .head(&ai_catalog_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    // Tier 2 Probes: Site Topology & API Specs
    let sitemap_url = format!("{base_url}/sitemap.xml");
    let robots_url = format!("{base_url}/robots.txt");
    let openapi_url = format!("{base_url}/openapi.json");

    let has_sitemap = client
        .head(&sitemap_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let has_robots = client
        .head(&robots_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let has_openapi = client
        .head(&openapi_url)
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let has_rss = body_sample.contains("application/rss+xml") || body_sample.contains("application/atom+xml");

    // Protocol class resolution across the 5 tiers:
    // 0: ManifestDirect
    // 1: StructuredMetadata (sitemap, openapi, or json-ld)
    // 2: StaticFast
    // 3: CdpDynamic
    // 4: DropOrBypass
    let protocol_class = if bot_challenge {
        4 // DropOrBypass
    } else if has_full || has_txt || has_catalog {
        0 // ManifestDirect
    } else if has_sitemap || has_openapi || has_json_ld {
        1 // StructuredMetadata
    } else if is_doc_subdomain || !requires_js {
        2 // StaticFast
    } else {
        3 // CdpDynamic
    };

    let quality_prior = if has_full || has_txt {
        0.98
    } else if has_openapi || has_json_ld {
        0.90
    } else if has_sitemap || is_doc_subdomain {
        0.85
    } else if bot_challenge {
        0.05
    } else {
        0.65
    };

    Some(HarvesterProbeResult {
        domain: domain.to_string(),
        has_full,
        has_txt,
        has_catalog,
        has_sitemap,
        has_robots,
        has_openapi,
        has_rss,
        has_json_ld,
        has_open_graph,
        is_doc_subdomain,
        requires_js,
        bot_challenge,
        latency_ms,
        quality_prior,
        protocol_class,
    })
}

/// Convert a live physical probe result into a 12-dimensional training sample.
pub fn to_probe_item(result: &HarvesterProbeResult) -> ProbeItem {
    let early_term = if result.protocol_class <= 1 { 0.95 } else { 0.10 };
    let structural_density = if result.is_doc_subdomain || result.has_full || result.has_openapi {
        0.90
    } else {
        0.50
    };

    ProbeItem {
        features: [
            if result.has_full { 1.0 } else { 0.0 },
            if result.has_txt { 1.0 } else { 0.0 },
            if result.has_catalog { 1.0 } else { 0.0 },
            if result.has_sitemap { 1.0 } else { 0.0 },
            if result.has_robots { 1.0 } else { 0.0 },
            if result.has_openapi { 1.0 } else { 0.0 },
            if result.has_rss { 1.0 } else { 0.0 },
            if result.has_json_ld { 1.0 } else { 0.0 },
            if result.has_open_graph { 1.0 } else { 0.0 },
            if result.is_doc_subdomain { 1.0 } else { 0.0 },
            if result.requires_js { 1.0 } else { 0.0 },
            if result.bot_challenge { 1.0 } else { 0.0 },
        ],
        target_protocol: result.protocol_class,
        target_scores: [result.quality_prior, structural_density, early_term],
    }
}

/// Extract up to `limit` child URLs from a site's `/sitemap.xml`.
pub async fn extract_sitemap_urls(client: &Client, base_url: &str, limit: usize) -> Vec<String> {
    let sitemap_url = if let Ok(parsed) = Url::parse(base_url) {
        let scheme = parsed.scheme();
        let host = parsed.host_str().unwrap_or_default();
        format!("{scheme}://{host}/sitemap.xml")
    } else {
        format!("{base_url}/sitemap.xml")
    };

    let resp = match client.get(&sitemap_url).send().await {
        Ok(r) if r.status().is_success() => r,
        _ => return Vec::new(),
    };

    let xml = resp.text().await.unwrap_or_default();
    let mut urls = Vec::new();

    // Fast zero-dependency XML <loc> tag extraction
    let mut rest = &xml[..];
    while let Some(start_loc) = rest.find("<loc>") {
        let after_start = &rest[start_loc + 5..];
        if let Some(end_loc) = after_start.find("</loc>") {
            let extracted = after_start[..end_loc].trim();
            if extracted.starts_with("http://") || extracted.starts_with("https://") {
                urls.push(extracted.to_string());
                if urls.len() >= limit {
                    break;
                }
            }
            rest = &after_start[end_loc + 6..];
        } else {
            break;
        }
    }

    urls
}

/// Extract up to `limit` internal documentation hyperlinks from an HTML body string.
pub fn extract_internal_doc_links(base_url: &str, html: &str, limit: usize) -> Vec<String> {
    let parsed_base = match Url::parse(base_url) {
        Ok(u) => u,
        Err(_) => return Vec::new(),
    };

    let host = match parsed_base.host_str() {
        Some(h) => h,
        None => return Vec::new(),
    };

    let mut links = Vec::new();
    let mut rest = html;

    while let Some(href_pos) = rest.find("href=\"") {
        let after_href = &rest[href_pos + 6..];
        if let Some(quote_pos) = after_href.find('"') {
            let href = &after_href[..quote_pos].trim();
            if !href.is_empty()
                && !href.starts_with('#')
                && !href.starts_with("javascript:")
                && let Ok(resolved) = parsed_base.join(href)
                && resolved.host_str() == Some(host)
            {
                let path = resolved.path();
                // Filter for high-signal documentation sub-paths
                if path.contains("doc")
                    || path.contains("api")
                    || path.contains("guide")
                    || path.contains("spec")
                    || path.contains("manual")
                    || path.contains("reference")
                    || path.contains("rfc")
                    || path.contains("book")
                {
                    let resolved_str = resolved.to_string();
                    if !links.contains(&resolved_str) && resolved_str != base_url {
                        links.push(resolved_str);
                        if links.len() >= limit {
                            break;
                        }
                    }
                }
            }
            rest = &after_href[quote_pos + 1..];
        } else {
            break;
        }
    }

    links
}
