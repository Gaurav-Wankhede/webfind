import * as fs from "fs";

let f = fs.readFileSync("src/engine/fetcher.rs", "utf8");

// Replace imports at the top of fetcher.rs
const topImports = `use std::num::NonZeroU32;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use reqwest::Client;

use crate::engine::device_profile::{DeviceProfile, SessionManager};
use crate::engine::fingerprint::{FingerprintAuditLog, FingerprintGenerator};
use crate::engine::human_client::HumanClient;
use crate::engine::proxy_pool::ProxyPool;
use crate::schema::content::StructuredContent;
`;

const newHumanCode = `    pub fn new_human(
        proxy_pool: Option<ProxyPool>,
        session_manager: Option<SessionManager>,
        rotate_ua: RotateUserAgent,
        rps: u32,
    ) -> Result<Self> {
        let rps = NonZeroU32::new(rps.max(1)).expect("rps.max(1) is always >= 1");
        let ua_rotate = match rotate_ua {
            RotateUserAgent::Rotate => true,
            RotateUserAgent::Fixed => false,
        };
        let mut builder = HumanClient::builder()
            .requests_per_second(rps)
            .burst_size(rps.get() * 3)
            .rotate_ua(ua_rotate);
        if let Some(pool) = proxy_pool {
            builder = builder.proxy_pool(pool);
        }
        if let Some(mgr) = session_manager {
            builder = builder.session_manager(mgr);
        }
        let fingerprint_generator = Some(FingerprintGenerator::new());
        if let Some(generator) = fingerprint_generator {
            builder = builder.fingerprint_generator(generator);
        }
        let human_client = builder.build()?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .gzip(true)
            .build()
            .context("failed to build fallback HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(human_client),
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }`;

// Replace new_human in fetcher.rs
const nhStart = f.indexOf("pub fn new_human");
const nhEnd = f.indexOf("pub fn with_audit_log");
f = f.slice(0, nhStart) + newHumanCode + "\n\n    " + f.slice(nhEnd);

// Replace top imports
const modStart = f.indexOf("pub mod cleaner;");
f = topImports + "\n" + f.slice(modStart);

fs.writeFileSync("src/engine/fetcher.rs", f, "utf8");
console.log("Restored original new_human signature and imports in fetcher.rs");

// Also clean up unused imports across fetcher submodules
const fixUnused = (file: string, toRemove: string[]) => {
    let content = fs.readFileSync(file, "utf8");
    for (const r of toRemove) {
        content = content.replace(r, "");
    }
    fs.writeFileSync(file, content, "utf8");
};

fixUnused("src/engine/fetcher/dom_entities.rs", ["use super::*;\n"]);
fixUnused("src/engine/fetcher/extract.rs", [
    "use scraper::Selector;\n",
    "use super::*;\n",
    "use std::time::Instant;\n",
]);
fixUnused("src/engine/fetcher/raw_content.rs", [
    "use scraper::Selector;\n",
    "use super::*;\n",
    "use anyhow::{Context, Result};\n",
]);
fixUnused("src/engine/fetcher/text_process.rs", ["use regex::Regex;\n"]);
fixUnused("src/engine/fetcher.rs", [
    "extract_domain, ",
    "clean_html_streaming, ",
    "looks_like_css, ",
    "looks_like_javascript, ",
    "looks_like_prose, ",
    "strip_junk_segments, ",
]);
console.log("Cleaned unused imports");
