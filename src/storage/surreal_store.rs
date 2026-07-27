use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};
use surrealdb::Surreal;
use surrealdb::engine::any::{Any, connect};
use surrealdb::opt::auth::Root;
use surrealdb::types::Datetime;

use crate::engine::crawl_graph::{CrawlGraphStore, DiscoverySource, LinkEdge, UrlNode};
use crate::engine::fingerprint::{Fingerprint, FingerprintAuditLog};

/// SurrealDB-backed graph store for crawl data.
///
/// Persists discovered URLs as `url_node` records and inter-page links as
/// `link_edge` graph relations, enabling graph traversal and vector-aware
/// retrieval downstream.
pub struct SurrealStore {
    db: Surreal<Any>,
}

impl SurrealStore {
    /// Connect to a SurrealDB instance.
    ///
    /// Supported URLs:
    /// - `memory` (embedded, for tests; not shared across processes)
    /// - `surrealkv://path/to/db` (embedded file-backed, persists across processes)
    /// - `ws://host:port` / `wss://host:port` (production)
    pub async fn new(url: &str, user: &str, pass: &str, ns: &str, db_name: &str) -> Result<Self> {
        let db: Surreal<Any> = connect(url).await.context("connect to SurrealDB")?;

        // Only authenticate when credentials are provided. In-memory engines
        // typically run without root auth.
        if !user.is_empty() || !pass.is_empty() {
            db.signin(Root {
                username: user.to_string(),
                password: pass.to_string(),
            })
            .await
            .context("sign in to SurrealDB")?;
        }

        db.use_ns(ns)
            .use_db(db_name)
            .await
            .context("select SurrealDB namespace/database")?;

        let store = Self { db };
        store.init_schema().await?;
        Ok(store)
    }

    /// Idempotent schema setup for graph storage.
    async fn init_schema(&self) -> Result<()> {
        self.db
            .query(
                r#"
                DEFINE TABLE IF NOT EXISTS url_node SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS url          ON url_node TYPE string   ASSERT $value != NONE;
                DEFINE FIELD IF NOT EXISTS domain       ON url_node TYPE string;
                DEFINE FIELD IF NOT EXISTS source       ON url_node TYPE string;
                DEFINE FIELD IF NOT EXISTS depth        ON url_node TYPE number   DEFAULT 0;
                DEFINE FIELD IF NOT EXISTS priority     ON url_node TYPE number;
                DEFINE FIELD IF NOT EXISTS lastmod      ON url_node TYPE option<datetime>;
                DEFINE FIELD IF NOT EXISTS changefreq   ON url_node TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS discovered_at ON url_node TYPE datetime;
                DEFINE FIELD IF NOT EXISTS crawled      ON url_node TYPE bool     DEFAULT false;
                DEFINE INDEX IF NOT EXISTS url_idx      ON url_node COLUMNS url UNIQUE;

                DEFINE TABLE IF NOT EXISTS link_edge SCHEMAFULL TYPE RELATION;
                DEFINE FIELD IF NOT EXISTS in         ON link_edge TYPE record<url_node>;
                DEFINE FIELD IF NOT EXISTS out       ON link_edge TYPE record<url_node>;
                DEFINE FIELD IF NOT EXISTS anchor_text ON link_edge TYPE option<string>;

                DEFINE TABLE IF NOT EXISTS graph_meta SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS value ON graph_meta TYPE number DEFAULT 0;
                UPSERT graph_meta:version SET value = 0;

                DEFINE TABLE IF NOT EXISTS ip_health SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS ip               ON ip_health TYPE string  ASSERT $value != NONE;
                DEFINE FIELD IF NOT EXISTS fingerprint_id   ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS isp                ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS asn                ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS country            ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS user_agent         ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS accept_language    ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS device_class       ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS geo_region         ON ip_health TYPE string;
                DEFINE FIELD IF NOT EXISTS working            ON ip_health TYPE bool    DEFAULT true;
                DEFINE FIELD IF NOT EXISTS success_count      ON ip_health TYPE number  DEFAULT 0;
                DEFINE FIELD IF NOT EXISTS failure_count      ON ip_health TYPE number  DEFAULT 0;
                DEFINE FIELD IF NOT EXISTS last_used          ON ip_health TYPE option<datetime>;
                DEFINE FIELD IF NOT EXISTS last_error         ON ip_health TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS discarded_at       ON ip_health TYPE option<datetime>;
                DEFINE INDEX IF NOT EXISTS ip_idx             ON ip_health COLUMNS ip UNIQUE;

                DEFINE TABLE IF NOT EXISTS fingerprint_log SCHEMAFULL;
                DEFINE FIELD IF NOT EXISTS fingerprint_id   ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS ip                 ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS isp                ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS asn                ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS country            ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS user_agent         ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS accept_language    ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS device_class       ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS geo_region         ON fingerprint_log TYPE string;
                DEFINE FIELD IF NOT EXISTS status_code        ON fingerprint_log TYPE option<number>;
                DEFINE FIELD IF NOT EXISTS error              ON fingerprint_log TYPE option<string>;
                DEFINE FIELD IF NOT EXISTS used_at            ON fingerprint_log TYPE datetime;
                "#,
            )
            .await
            .context("define graph schema")?;
        Ok(())
    }

    fn url_id(url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        let digest = hasher.finalize();
        let hash = format!(
            "{:016x}",
            u128::from_be_bytes(digest[..16].try_into().unwrap())
        );
        format!("url_node:{}", hash)
    }

    fn source_str(source: DiscoverySource) -> &'static str {
        match source {
            DiscoverySource::Seed => "seed",
            DiscoverySource::Sitemap => "sitemap",
            DiscoverySource::LinkCrawl => "link_crawl",
            DiscoverySource::ExternalLink => "external_link",
        }
    }

    fn parse_source(s: &str) -> DiscoverySource {
        match s {
            "sitemap" => DiscoverySource::Sitemap,
            "link_crawl" => DiscoverySource::LinkCrawl,
            "external_link" => DiscoverySource::ExternalLink,
            _ => DiscoverySource::Seed,
        }
    }

    async fn bump_graph_version(&self) {
        let result = self
            .db
            .query("UPSERT graph_meta:version SET value += 1")
            .await;
        if let Err(e) = result {
            tracing::error!("failed to bump graph version: {}", e);
        }
    }

    fn deserialize_url_node(value: JsonValue, url_override: &str) -> Option<UrlNode> {
        let row: SurrealUrlNode = serde_json::from_value(value).ok()?;
        Some(UrlNode {
            url: url_override.to_string(),
            domain: row.domain,
            source: Self::parse_source(&row.source),
            depth: row.depth,
            priority: row.priority,
            lastmod: row.lastmod,
            changefreq: row.changefreq,
            discovered_at: row.discovered_at,
            crawled: row.crawled,
        })
    }
}

#[async_trait]
impl CrawlGraphStore for SurrealStore {
    async fn record_url(&self, node: UrlNode) {
        let id = Self::url_id(&node.url);
        let source = Self::source_str(node.source).to_string();

        let query = format!(
            "UPSERT {} SET url = $url, domain = $domain, source = $source, depth = $depth, priority = $priority, lastmod = $lastmod, changefreq = $changefreq, discovered_at = $discovered_at, crawled = $crawled",
            id
        );

        let discovered_at: Datetime = node.discovered_at.into();
        let lastmod: Option<Datetime> = node.lastmod.map(Into::into);

        let result = self
            .db
            .query(query)
            .bind(("url", node.url))
            .bind(("domain", node.domain))
            .bind(("source", source))
            .bind(("depth", node.depth))
            .bind(("priority", node.priority))
            .bind(("lastmod", lastmod))
            .bind(("changefreq", node.changefreq))
            .bind(("discovered_at", discovered_at))
            .bind(("crawled", node.crawled))
            .await;
        match result {
            Ok(response) => {
                if let Err(e) = response.check() {
                    tracing::error!("failed to record url node: {}", e);
                }
            }
            Err(e) => tracing::error!("failed to record url node: {}", e),
        }
        self.bump_graph_version().await;
    }

    async fn record_link(&self, edge: LinkEdge) {
        let from = Self::url_id(&edge.from);
        let to = Self::url_id(&edge.to);

        let result = self
            .db
            .query(format!(
                "RELATE {} -> link_edge -> {} SET anchor_text = $anchor_text",
                from, to
            ))
            .bind(("anchor_text", edge.anchor_text))
            .await;
        if let Err(e) = result {
            tracing::error!("failed to record link edge: {}", e);
        }
        self.bump_graph_version().await;
    }

    async fn get_url(&self, url: &str) -> Option<UrlNode> {
        let id = Self::url_id(url);
        let url = url.to_string();

        let response = self.db.query(format!("SELECT * FROM {}", id)).await.ok()?;
        let mut response = response;
        let row: Option<JsonValue> = response.take(0).ok()?;
        row.and_then(|r| Self::deserialize_url_node(r, &url))
    }

    async fn get_urls(&self) -> Vec<UrlNode> {
        let mut response = match self.db.query("SELECT * FROM url_node").await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list url nodes: {}", e);
                return Vec::new();
            }
        };
        let rows: Vec<JsonValue> = match response.take(0) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to deserialize url nodes: {}", e);
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter_map(|r| {
                let url = r
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                Self::deserialize_url_node(r, &url)
            })
            .collect()
    }

    async fn get_links_from(&self, url: &str) -> Vec<LinkEdge> {
        let from = Self::url_id(url);
        let from_url = url.to_string();

        let mut response = match self
            .db
            .query(format!(
                "SELECT id, out.url as to_url, anchor_text FROM link_edge WHERE in = {}",
                from
            ))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list outgoing links: {}", e);
                return Vec::new();
            }
        };
        let rows: Vec<JsonValue> = match response.take(0) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to deserialize link edges: {}", e);
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter_map(|r| {
                let to_url = r.get("to_url").and_then(|v| v.as_str())?;
                let anchor_text = r
                    .get("anchor_text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some(LinkEdge {
                    from: from_url.clone(),
                    to: to_url.to_string(),
                    anchor_text,
                })
            })
            .collect()
    }

    async fn get_links_to(&self, url: &str) -> Vec<LinkEdge> {
        let to = Self::url_id(url);
        let to_url = url.to_string();

        let mut response = match self
            .db
            .query(format!(
                "SELECT id, in.url as from_url, anchor_text FROM link_edge WHERE out = {}",
                to
            ))
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list incoming links: {}", e);
                return Vec::new();
            }
        };
        let rows: Vec<JsonValue> = match response.take(0) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to deserialize link edges: {}", e);
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter_map(|r| {
                let from_url = r.get("from_url").and_then(|v| v.as_str())?;
                let anchor_text = r
                    .get("anchor_text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some(LinkEdge {
                    from: from_url.to_string(),
                    to: to_url.clone(),
                    anchor_text,
                })
            })
            .collect()
    }

    async fn get_all_links(&self) -> Vec<LinkEdge> {
        let mut response = match self
            .db
            .query("SELECT id, in.url as from_url, out.url as to_url, anchor_text FROM link_edge")
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list all links: {}", e);
                return Vec::new();
            }
        };
        let rows: Vec<JsonValue> = match response.take(0) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to deserialize all links: {}", e);
                return Vec::new();
            }
        };
        rows.into_iter()
            .filter_map(|r| {
                let from_url = r.get("from_url").and_then(|v| v.as_str())?;
                let to_url = r.get("to_url").and_then(|v| v.as_str())?;
                let anchor_text = r
                    .get("anchor_text")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                Some(LinkEdge {
                    from: from_url.to_string(),
                    to: to_url.to_string(),
                    anchor_text,
                })
            })
            .collect()
    }

    async fn graph_version(&self) -> String {
        let mut response = match self.db.query("SELECT value FROM graph_meta:version").await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to read graph version: {}", e);
                return "0".to_string();
            }
        };
        let rows: Vec<JsonValue> = match response.take(0) {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to deserialize graph version: {}", e);
                return "0".to_string();
            }
        };
        rows.into_iter()
            .next()
            .and_then(|r| r.get("value").and_then(|v| v.as_i64()))
            .unwrap_or(0)
            .to_string()
    }
}

#[async_trait]
impl CrawlGraphStore for Arc<SurrealStore> {
    async fn record_url(&self, node: UrlNode) {
        (**self).record_url(node).await;
    }

    async fn record_link(&self, edge: LinkEdge) {
        (**self).record_link(edge).await;
    }

    async fn get_url(&self, url: &str) -> Option<UrlNode> {
        (**self).get_url(url).await
    }

    async fn get_urls(&self) -> Vec<UrlNode> {
        (**self).get_urls().await
    }

    async fn get_links_from(&self, url: &str) -> Vec<LinkEdge> {
        (**self).get_links_from(url).await
    }

    async fn get_links_to(&self, url: &str) -> Vec<LinkEdge> {
        (**self).get_links_to(url).await
    }

    async fn get_all_links(&self) -> Vec<LinkEdge> {
        (**self).get_all_links().await
    }

    async fn graph_version(&self) -> String {
        (**self).graph_version().await
    }
}

#[async_trait]
impl FingerprintAuditLog for SurrealStore {
    async fn log_fingerprint_use(
        &self,
        fp: &Fingerprint,
        status_code: Option<u16>,
        error: Option<&str>,
    ) {
        let used_at: Datetime = chrono::Utc::now().into();
        let log_result = self
            .db
            .query(
                "CREATE fingerprint_log SET fingerprint_id = $fingerprint_id, ip = $ip, isp = $isp, asn = $asn, country = $country, user_agent = $user_agent, accept_language = $accept_language, device_class = $device_class, geo_region = $geo_region, status_code = $status_code, error = $error, used_at = $used_at",
            )
            .bind(("fingerprint_id", fp.id.clone()))
            .bind(("ip", fp.ip.clone()))
            .bind(("isp", fp.isp.name.clone()))
            .bind(("asn", fp.isp.asn.clone()))
            .bind(("country", fp.isp.country.clone()))
            .bind(("user_agent", fp.user_agent.clone()))
            .bind(("accept_language", fp.accept_language.clone()))
            .bind(("device_class", format!("{:?}", fp.device_class)))
            .bind(("geo_region", format!("{:?}", fp.geo_region)))
            .bind(("status_code", status_code.map(|s| s as i64)))
            .bind(("error", error.map(|e| e.to_string())))
            .bind(("used_at", used_at))
            .await;
        if let Err(e) = log_result {
            tracing::error!("failed to log fingerprint use: {}", e);
            return;
        }

        // Upsert the running health row for this IP.
        let working = error.is_none() && status_code.map(|s| s < 400).unwrap_or(true);
        let last_used: Datetime = chrono::Utc::now().into();
        let health_result = self
            .db
            .query(
                "UPSERT ip_health SET ip = $ip, fingerprint_id = $fingerprint_id, isp = $isp, asn = $asn, country = $country, user_agent = $user_agent, accept_language = $accept_language, device_class = $device_class, geo_region = $geo_region, working = $working, success_count += $success_inc, failure_count += $failure_inc, last_used = $last_used, last_error = $last_error",
            )
            .bind(("ip", fp.ip.clone()))
            .bind(("fingerprint_id", fp.id.clone()))
            .bind(("isp", fp.isp.name.clone()))
            .bind(("asn", fp.isp.asn.clone()))
            .bind(("country", fp.isp.country.clone()))
            .bind(("user_agent", fp.user_agent.clone()))
            .bind(("accept_language", fp.accept_language.clone()))
            .bind(("device_class", format!("{:?}", fp.device_class)))
            .bind(("geo_region", format!("{:?}", fp.geo_region)))
            .bind(("working", working))
            .bind(("success_inc", if working { 1 } else { 0 }))
            .bind(("failure_inc", if working { 0 } else { 1 }))
            .bind(("last_used", last_used))
            .bind(("last_error", error.map(|e| e.to_string())))
            .await;
        if let Err(e) = health_result {
            tracing::error!("failed to upsert ip health: {}", e);
        }
    }
}

#[async_trait]
impl FingerprintAuditLog for Arc<SurrealStore> {
    async fn log_fingerprint_use(
        &self,
        fp: &Fingerprint,
        status_code: Option<u16>,
        error: Option<&str>,
    ) {
        (**self).log_fingerprint_use(fp, status_code, error).await;
    }
}

#[derive(Debug, serde::Deserialize)]
struct SurrealUrlNode {
    domain: String,
    source: String,
    depth: u32,
    priority: f32,
    lastmod: Option<chrono::DateTime<chrono::Utc>>,
    changefreq: Option<String>,
    discovered_at: chrono::DateTime<chrono::Utc>,
    crawled: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    #[tokio::test]
    async fn test_surreal_store_records_and_reads() {
        let store = SurrealStore::new("memory", "", "", "webfind", "webfind")
            .await
            .expect("connect to in-memory SurrealDB");
        let store = Arc::new(store);

        store
            .record_url(UrlNode {
                url: "https://example.com/".to_string(),
                domain: "example.com".to_string(),
                source: DiscoverySource::Seed,
                depth: 0,
                priority: 1.0,
                lastmod: None,
                changefreq: None,
                discovered_at: Utc::now(),
                crawled: false,
            })
            .await;
        store
            .record_url(UrlNode {
                url: "https://example.com/a".to_string(),
                domain: "example.com".to_string(),
                source: DiscoverySource::LinkCrawl,
                depth: 1,
                priority: 0.5,
                lastmod: None,
                changefreq: None,
                discovered_at: Utc::now(),
                crawled: false,
            })
            .await;
        store
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: Some("link".to_string()),
            })
            .await;

        let urls = store.get_urls().await;
        assert_eq!(urls.len(), 2, "expected 2 url nodes");

        let root = store
            .get_url("https://example.com/")
            .await
            .expect("root url exists");
        assert_eq!(root.source, DiscoverySource::Seed);

        let outgoing = store.get_links_from("https://example.com/").await;
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].to, "https://example.com/a");

        let incoming = store.get_links_to("https://example.com/a").await;
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].from, "https://example.com/");
    }
}
