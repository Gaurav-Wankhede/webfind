import * as fs from "fs";

// 1. In meta.rs: add struct PageMeta definition and fix extract_domain import
let meta = fs.readFileSync("src/engine/fetcher/meta.rs", "utf8");
meta = meta.replace("use std::collections::HashSet;\nuse crate::engine::util::extract_domain;\n", "");
const metaHeader = `use std::collections::HashSet;
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde_json::Value;
use crate::schema::content::{ImageInfo, OpenGraph, TwitterCard};

fn extract_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}

#[derive(Debug, Default)]
pub(crate) struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical_url: Option<String>,
    pub published_time: Option<DateTime<Utc>>,
    pub modified_time: Option<DateTime<Utc>>,
    pub author: Option<String>,
    pub site_name: Option<String>,
}
`;
let metaStart = meta.indexOf("pub(crate) fn extract_meta");
fs.writeFileSync("src/engine/fetcher/meta.rs", metaHeader + "\n" + meta.slice(metaStart), "utf8");
console.log("Fixed meta.rs");

// 2. In url_util.rs: fix extract_domain usage
let u = fs.readFileSync("src/engine/fetcher/url_util.rs", "utf8");
u = u.replace("use crate::engine::util::extract_domain;\n", "");
const uHeader = `fn extract_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}
`;
fs.writeFileSync("src/engine/fetcher/url_util.rs", uHeader + "\n" + u, "utf8");
console.log("Fixed url_util.rs");

// 3. In container.rs: define extract_domain and is_noise_element
let c = fs.readFileSync("src/engine/fetcher/container.rs", "utf8");
c = c.replace(/use super::\*;\s*use scraper::Selector;\s*/g, "");
const cHeader = `use scraper::{Html, Selector};
use super::cleaner::is_noise_element;
use super::text_process::normalize_text;

fn extract_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}
`;
let cFnIdx = c.indexOf("pub(crate) fn extract_h1");
fs.writeFileSync("src/engine/fetcher/container.rs", cHeader + "\n" + c.slice(cFnIdx), "utf8");
console.log("Fixed container.rs");

// 4. In cleaner.rs: make is_noise_element pub(crate)
let cl = fs.readFileSync("src/engine/fetcher/cleaner.rs", "utf8");
cl = cl.replace("fn is_noise_element", "pub(crate) fn is_noise_element");
fs.writeFileSync("src/engine/fetcher/cleaner.rs", cl, "utf8");
console.log("Fixed cleaner.rs");

// 5. In reddit.rs: clean up structure & trailing comments
let red = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
let redEnd = red.indexOf("/// Extract StructuredContent from raw HTML.");
if (redEnd !== -1) {
    red = red.slice(0, redEnd) + "\n}\n";
    fs.writeFileSync("src/engine/fetcher/reddit.rs", red, "utf8");
    console.log("Fixed reddit.rs");
}

// 6. In text_process.rs: remove trailing doc comment
let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
let tpEnd = tp.lastIndexOf("}");
tp = tp.slice(0, tpEnd + 1) + "\n";
fs.writeFileSync("src/engine/fetcher/text_process.rs", tp, "utf8");
console.log("Fixed text_process.rs");

// 7. In fetcher.rs: make re-exports pub(crate)
let f = fs.readFileSync("src/engine/fetcher.rs", "utf8");
f = f.replace(/pub use cleaner::/g, "pub(crate) use cleaner::");
f = f.replace(/pub use container::/g, "pub(crate) use container::");
f = f.replace(/pub use dom_entities::/g, "pub(crate) use dom_entities::");
f = f.replace(/pub use entities::/g, "pub(crate) use entities::");
f = f.replace(/pub use meta::/g, "pub(crate) use meta::");
f = f.replace(/pub use quality_entities::/g, "pub(crate) use quality_entities::");
f = f.replace(/pub use raw_content::/g, "pub(crate) use raw_content::");
f = f.replace(/pub use text_process::/g, "pub(crate) use text_process::");
f = f.replace(/pub use url_util::/g, "pub(crate) use url_util::");
fs.writeFileSync("src/engine/fetcher.rs", f, "utf8");
console.log("Fixed fetcher.rs re-exports");
