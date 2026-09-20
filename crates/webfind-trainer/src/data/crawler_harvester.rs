// webfind-trainer: Source 1 - Physical Network Boundary Ground Truth Harvester.
// Reads unique domains from webfind.db and runs async HTTP manifest & bot-challenge probes.

use crate::dataset::ProbeItem;
use anyhow::{Context, Result};
use reqwest::Client;
use rusqlite::Connection;
use std::time::Instant;

/// Physical probe result for a single domain.
#[derive(Debug, Clone)]
pub struct HarvesterProbeResult {
    pub domain: String,
    pub has_full: bool,
    pub has_txt: bool,
    pub has_catalog: bool,
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

/// Execute a live physical socket probe on a domain to determine manifest presence and bot boundaries.
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

    // Probe manifests
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

    // Protocol class resolution based on physical evidence
    let (protocol_class, requires_js) = if bot_challenge {
        (3, false) // DropOrBypass
    } else if has_full || has_txt || has_catalog {
        (0, false) // ManifestDirect
    } else if is_doc_subdomain {
        (1, false) // StaticFast
    } else {
        (2, true)  // CdpDynamic
    };

    let quality_prior = if has_full || has_txt {
        0.95
    } else if is_doc_subdomain {
        0.85
    } else if bot_challenge {
        0.15
    } else {
        0.65
    };

    Some(HarvesterProbeResult {
        domain: domain.to_string(),
        has_full,
        has_txt,
        has_catalog,
        is_doc_subdomain,
        requires_js,
        bot_challenge,
        latency_ms,
        quality_prior,
        protocol_class,
    })
}

/// Convert physical probe result into a typed ProbeItem for Burn tensor training.
pub fn to_probe_item(r: &HarvesterProbeResult) -> ProbeItem {
    let features = [
        if r.has_full { 1.0 } else { 0.0 },
        if r.has_txt { 1.0 } else { 0.0 },
        if r.has_catalog { 1.0 } else { 0.0 },
        if r.is_doc_subdomain { 1.0 } else { 0.0 },
        if r.requires_js { 1.0 } else { 0.0 },
        if r.bot_challenge { 1.0 } else { 0.0 },
        (r.latency_ms / 1000.0).clamp(0.0, 5.0),
        r.quality_prior,
    ];

    let target_scores = match r.protocol_class {
        0 => [0.95, 0.92, 0.0],
        1 => [0.80, 0.85, 0.0],
        2 => [0.65, 0.60, 0.0],
        _ => [0.10, 0.10, 1.0], // DropOrBypass: terminate = 1.0
    };

    ProbeItem {
        features,
        target_protocol: r.protocol_class,
        target_scores,
    }
}
