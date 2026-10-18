import * as fs from "fs";

// 1. In fetcher.rs:
// Fix new_human signature:
// pub fn new_human(
//     proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
//     session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
//     rotate_ua: RotateUserAgent,
//     rps: Option<std::num::NonZeroU32>,
// ) -> Result<Self>
let f = fs.readFileSync("src/engine/fetcher.rs", "utf8");
const oldNewHuman = `    pub fn new_human(
        profile: Option<DeviceProfile>,
        proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        audit_log: Option<Arc<dyn FingerprintAuditLog>>,
        rotate_ua: RotateUserAgent,
    ) -> Result<Self> {
        let (human_client, profile) = match (profile, proxy_pool) {
            (Some(prof), Some(pool)) => {
                let hc = HumanClient::new(
                    prof.clone(),
                    pool,
                    session_manager,
                    audit_log.clone(),
                    matches!(rotate_ua, RotateUserAgent::Rotate),
                )?;
                (Some(hc), Some(prof))
            }
            (Some(prof), None) => {
                let pool = Arc::new(crate::engine::proxy_pool::ProxyPool::new(vec![]));
                let hc = HumanClient::new(
                    prof.clone(),
                    pool,
                    session_manager,
                    audit_log.clone(),
                    matches!(rotate_ua, RotateUserAgent::Rotate),
                )?;
                (Some(hc), Some(prof))
            }
            _ => (None, None),
        };

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client,
            profile,
            audit_log,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }`;

const fixedNewHuman = `    pub fn new_human(
        proxy_pool: Option<Arc<crate::engine::proxy_pool::ProxyPool>>,
        session_manager: Option<Arc<tokio::sync::Mutex<crate::engine::device_profile::SessionManager>>>,
        rotate_ua: RotateUserAgent,
        _rps: Option<std::num::NonZeroU32>,
    ) -> Result<Self> {
        let pool = proxy_pool.unwrap_or_else(|| Arc::new(crate::engine::proxy_pool::ProxyPool::new(vec![])));
        let profile = DeviceProfile::desktop_chrome();
        let hc = HumanClient::new(
            profile.clone(),
            pool,
            session_manager,
            None,
            matches!(rotate_ua, RotateUserAgent::Rotate),
        )?;

        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(FETCH_TIMEOUT_SECS))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("failed to build default HTTP client")?;

        Ok(Self {
            client,
            human_client: Some(hc),
            profile: Some(profile),
            audit_log: None,
            dynamic_fallback: false,
            dynamic_wait_ms: 2000,
            dynamic_deep: false,
        })
    }`;

f = f.replace(oldNewHuman, fixedNewHuman);
// Make try_manifest_fast_path and fetch_reddit pub(crate)
f = f.replace("pub use cleaner::*;", "pub use cleaner::*;\npub use reddit::*;\npub use extract::*;\npub use raw_content::*;");
fs.writeFileSync("src/engine/fetcher.rs", f, "utf8");
console.log("Updated fetcher.rs");

// 2. In fetcher/reddit.rs:
// Make methods pub(crate) and fix &str vs String in listing_to_text
let r = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
r = r.replace("async fn fetch_reddit(&self", "pub(crate) async fn fetch_reddit(&self");
r = r.replace("fn build_reddit_request(&self", "pub(crate) fn build_reddit_request(&self");
r = r.replace("fn build_reddit_content(&self", "pub(crate) fn build_reddit_content(&self");
// Fix text borrowing if needed:
r = r.replace("match super::reddit::parse_reddit_listing(&body) {", "match super::reddit::parse_reddit_listing(&body) {");
r = r.replace("use super::*;", "use super::*;\nuse chrono::Utc;\nuse crate::schema::content::*;");
fs.writeFileSync("src/engine/fetcher/reddit.rs", r, "utf8");
console.log("Updated reddit.rs");

// 3. In fetcher/raw_content.rs:
// Make try_manifest_fast_path pub(crate)
let raw = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
raw = raw.replace("async fn try_manifest_fast_path(&self", "pub(crate) async fn try_manifest_fast_path(&self");
raw = raw.replace("fn parse_content_type_header", "pub(crate) fn parse_content_type_header");
raw = raw.replace("fn extract_from_text", "pub(crate) fn extract_from_text");
raw = raw.replace("fn make_invalid_content", "pub(crate) fn make_invalid_content");
raw = raw.replace("enum ContentKind", "pub(crate) enum ContentKind");
raw = "use super::*;\nuse chrono::Utc;\nuse scraper::Selector;\n" + raw;
fs.writeFileSync("src/engine/fetcher/raw_content.rs", raw, "utf8");
console.log("Updated raw_content.rs");

// 4. In cleaner.rs, container.rs, dom_entities.rs, quality_entities.rs:
// Ensure Selector and super::* are imported
const addHeader = (file: string) => {
    let content = fs.readFileSync(file, "utf8");
    if (!content.includes("use scraper::Selector;")) {
        content = "use super::*;\nuse scraper::Selector;\n" + content;
        fs.writeFileSync(file, content, "utf8");
    }
};
addHeader("src/engine/fetcher/cleaner.rs");
addHeader("src/engine/fetcher/container.rs");
addHeader("src/engine/fetcher/dom_entities.rs");
addHeader("src/engine/fetcher/quality_entities.rs");
addHeader("src/engine/fetcher/extract.rs");
console.log("Added headers to submodules");
