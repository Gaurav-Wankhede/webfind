import * as fs from "fs";

// 1. In fetcher.rs:
let f = fs.readFileSync("src/engine/fetcher.rs", "utf8");
const targetNewHuman = `    pub fn new_human(
        proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        rotate_ua: RotateUserAgent,
        rps: Option<std::num::NonZeroU32>,
    ) -> Result<Self> {
        let mut b = HumanClient::builder();
        if let Some(pool) = proxy_pool {
            b = b.proxy_pool((*pool).clone());
        }
        if let Some(mgr) = session_manager {
            if let Ok(guard) = mgr.try_lock() {
                b = b.session_manager((*guard).clone());
            }
        }
        if let Some(r) = rps {
            b = b.requests_per_second(r);
        }
        let hc = b.build()?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(hc),
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }`;

const fixedNewHuman = `    pub fn new_human(
        _proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        _session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        _rotate_ua: RotateUserAgent,
        _rps: Option<std::num::NonZeroU32>,
    ) -> Result<Self> {
        let hc = HumanClient::builder().build()?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(hc),
            profile: None,
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }`;

f = f.replace(targetNewHuman, fixedNewHuman);
fs.writeFileSync("src/engine/fetcher.rs", f, "utf8");

// 2. In entities.rs: Add Regex constants and imports
const entitiesHeader = `use std::collections::HashSet;
use std::sync::LazyLock;
use regex::Regex;
use crate::schema::content::{CodeBlockEntity, Entities, ExampleEntity, MathEntity};

static RE_EMAIL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[a-zA-Z0-9_.+-]+@[a-zA-Z0-9-]+\.[a-zA-Z0-9-.]+").unwrap()
});

static RE_PHONE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(\\+?\\d{1,3}[-.\\s]?)?\\(?\\d{3}\\)?[-.\\s]?\\d{3}[-.\\s]?\\d{4}").unwrap()
});

static RE_ADDRESS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\\b\\d{1,5}\\s+[a-z0-9\\s.,]{3,30}\\s+(?:street|st|avenue|ave|road|rd|boulevard|blvd|drive|dr|lane|ln|way)\\b").unwrap()
});

static RE_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"https?://[^\\s/$.?#].[^\\s]*").unwrap()
});

static RE_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\\bv?(?:[0-9]+\\.){1,3}[0-9]+(?:-[a-z0-9.]+)?\\b").unwrap()
});

static RE_LICENSE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\\b(?:MIT|Apache-2\\.0|GPL-[0-9.]+|BSD-[0-9]?-Clause|MPL-2\\.0|ISC|Unlicense|CC-BY-[0-9.]+|AGPL-[0-9.]+)\\b").unwrap()
});

static RE_PRICE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[\\$\\€\\£\\¥]\\s*[0-9]+(?:,[0-9]{3})*(?:\\.[0-9]{2})?|[0-9]+(?:\\.[0-9]{2})?\\s*(?:USD|EUR|GBP|JPY|CAD|AUD))\\b").unwrap()
});

static RE_DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\b(?:20\\d{2}|19\\d{2})[-/.](?:0[1-9]|1[0-2])[-/.](?:0[1-9]|[12]\\d|3[01])\\b|\\b(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*\\s+\\d{1,2},?\\s+(?:20\\d{2}|19\\d{2})\\b").unwrap()
});

static RE_IP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\b(?:[0-9]{1,3}\\.){3}[0-9]{1,3}\\b|(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}\\b").unwrap()
});

static RE_SOCIAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)https?://(?:www\\.)?(?:twitter\\.com|x\\.com|github\\.com|linkedin\\.com/in|youtube\\.com/(?:c/|channel/|user/|@)?|instagram\\.com|facebook\\.com|reddit\\.com/u(?:ser)?)/[A-Za-z0-9_.-]+").unwrap()
});

static RE_MATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\\$\\$[\\s\\S]+?\\$\\$|\\$[^\\$\\n]+?\\$|\\\\\\[[\\s\\S]+?\\\\\\]|\\\\\\([^\\n]+?\\\\\\))").unwrap()
});
`;
let ent = fs.readFileSync("src/engine/fetcher/entities.rs", "utf8");
let entFnIdx = ent.indexOf("pub(crate) fn extract_entities");
fs.writeFileSync("src/engine/fetcher/entities.rs", entitiesHeader + "\n" + ent.slice(entFnIdx), "utf8");
console.log("Updated entities.rs");

// 3. In meta.rs: Add extract_domain and HashSet
let meta = fs.readFileSync("src/engine/fetcher/meta.rs", "utf8");
meta = `use std::collections::HashSet;
use crate::engine::util::extract_domain;
` + meta;
fs.writeFileSync("src/engine/fetcher/meta.rs", meta, "utf8");
console.log("Updated meta.rs");

// 4. In quality_entities.rs: Add constants and types
const qeHeader = `use scraper::{Html, Selector};
use crate::schema::content::{CalloutEntity, CodeBlockEntity, DiagramEntity, ExampleEntity, FaqEntity, ImageInfo, StepEntity};

const DIAGRAM_FP_DENY: &[&str] = &[
    "logo", "icon", "avatar", "badge", "button", "nav", "social",
    "favicon", "banner", "spinner", "loader", "ad-", "pixel",
];

const DIAGRAM_HINTS: &[&str] = &[
    "diagram", "architecture", "flowchart", "workflow", "schema",
    "uml", "sequence", "pipeline", "topology", "wireframe", "graph",
];

const DIAGRAM_LANGS: &[&str] = &[
    "mermaid", "dot", "graphviz", "plantuml", "d2", "c4", "nomnoml",
];

fn block_language(elem: scraper::ElementRef<'_>, fallback: scraper::ElementRef<'_>) -> Option<String> {
    elem.value()
        .attr("class")
        .or_else(|| fallback.value().attr("class"))
        .and_then(|classes| {
            classes.split_whitespace().find_map(|c| {
                c.strip_prefix("language-")
                    .or_else(|| c.strip_prefix("lang-"))
                    .map(|s| s.to_lowercase())
            })
        })
}
`;
let qe = fs.readFileSync("src/engine/fetcher/quality_entities.rs", "utf8");
let qeFnIdx = qe.indexOf("fn is_diagram_image");
fs.writeFileSync("src/engine/fetcher/quality_entities.rs", qeHeader + "\n" + qe.slice(qeFnIdx), "utf8");
console.log("Updated quality_entities.rs");

// 5. In url_util.rs: Use crate::engine::util::extract_domain
let u = fs.readFileSync("src/engine/fetcher/url_util.rs", "utf8");
u = `use crate::engine::util::extract_domain;\n` + u;
fs.writeFileSync("src/engine/fetcher/url_util.rs", u, "utf8");
console.log("Updated url_util.rs");

// 6. In raw_content.rs: Add Value import
let r = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
r = `use serde_json::Value;\n` + r;
fs.writeFileSync("src/engine/fetcher/raw_content.rs", r, "utf8");
console.log("Updated raw_content.rs");

// 7. In extract.rs: Add chrono::Utc
let ex = fs.readFileSync("src/engine/fetcher/extract.rs", "utf8");
ex = `use chrono::Utc;\n` + ex;
fs.writeFileSync("src/engine/fetcher/extract.rs", ex, "utf8");
console.log("Updated extract.rs");
