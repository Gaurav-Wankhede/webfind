//! Tier-2 Metadata Sniffer: detects machine-readable agent endpoints.
//!
//! Checks domains for machine-readable manifests (`/.well-known/ai-catalog.json`,
//! `/llms.txt`, `/llms-full.txt`, `/openapi.json`, `/swagger.json`) before falling
//! back to full DOM scraping or headless Chromium rendering.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use url::Url;

/// Machine-readable affordances discovered on a domain.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MachineAffordances {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llms_txt: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ai_catalog: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub openapi_spec: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ax_score: Option<f64>,
}

impl MachineAffordances {
    /// True when at least one machine-readable affordance is present.
    #[must_use]
    pub fn has_affordances(&self) -> bool {
        self.llms_txt.is_some()
            || self.ai_catalog.is_some()
            || self.openapi_spec.is_some()
            || self.mcp_endpoint.is_some()
    }
}

/// Metadata sniffer for agentic resource discovery.
pub struct MetadataSniffer {
    client: Client,
}

impl MetadataSniffer {
    /// Create a new metadata sniffer with the shared HTTP client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Probe a domain for agent-native metadata endpoints.
    pub async fn sniff_domain(&self, base_url: &str) -> MachineAffordances {
        let Ok(parsed) = Url::parse(base_url) else {
            return MachineAffordances::default();
        };

        let origin = match (parsed.scheme(), parsed.host_str(), parsed.port()) {
            (scheme, Some(host), Some(port)) => format!("{scheme}://{host}:{port}"),
            (scheme, Some(host), None) => format!("{scheme}://{host}"),
            _ => return MachineAffordances::default(),
        };

        let mut affordances = MachineAffordances::default();

        // 1. Probe /llms-full.txt or /llms.txt
        if let Ok(full_url) = Url::parse(&format!("{origin}/llms-full.txt"))
            && self.probe_endpoint(&full_url).await
        {
            affordances.llms_txt = Some(full_url.to_string());
        } else if let Ok(url) = Url::parse(&format!("{origin}/llms.txt"))
            && self.probe_endpoint(&url).await
        {
            affordances.llms_txt = Some(url.to_string());
        }

        // 2. Probe AI catalog variants: /.well-known/ai-catalog.json, /.well-known/ai-catelog.json, /ai-catalog.json, /ai-catelog.json
        let catalog_candidates = [
            format!("{origin}/.well-known/ai-catalog.json"),
            format!("{origin}/.well-known/ai-catelog.json"),
            format!("{origin}/ai-catalog.json"),
            format!("{origin}/ai-catelog.json"),
        ];
        for cand in &catalog_candidates {
            if let Ok(url) = Url::parse(cand)
                && self.probe_endpoint(&url).await
            {
                affordances.ai_catalog = Some(url.to_string());
                break;
            }
        }

        // 3. Probe /openapi.json
        if let Ok(url) = Url::parse(&format!("{origin}/openapi.json"))
            && self.probe_endpoint(&url).await
        {
            affordances.openapi_spec = Some(url.to_string());
        }

        affordances
    }

    /// Probe an endpoint with a lightweight HEAD/GET request.
    async fn probe_endpoint(&self, url: &Url) -> bool {
        // Fast timeout for sniffer probes (1.5s max)
        let resp = self
            .client
            .head(url.as_str())
            .timeout(std::time::Duration::from_millis(1500))
            .send()
            .await;

        if let Ok(res) = resp
            && res.status().is_success()
        {
            return true;
        }

        // Fallback to GET with small range if HEAD is blocked by origin
        let get_resp = self
            .client
            .get(url.as_str())
            .header(reqwest::header::RANGE, "bytes=0-1024")
            .timeout(std::time::Duration::from_millis(1500))
            .send()
            .await;

        if let Ok(res) = get_resp {
            res.status().is_success() || res.status() == reqwest::StatusCode::PARTIAL_CONTENT
        } else {
            false
        }
    }

    /// Fetch the full content of an affordance endpoint (e.g. /llms-full.txt or /llms.txt) as clean text.
    pub async fn fetch_manifest_content(&self, url_str: &str) -> Option<String> {
        let resp = self
            .client
            .get(url_str)
            .timeout(std::time::Duration::from_millis(3500))
            .send()
            .await
            .ok()?;

        if resp.status().is_success() {
            let body = resp.text().await.ok()?;
            if !body.trim().is_empty() && !body.to_ascii_lowercase().contains("<html") {
                return Some(body);
            }
        }
        None
    }
}
