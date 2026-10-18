import * as fs from "fs";

// 1. In meta.rs: Restore exact struct PageMeta
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
    pub published_time: Option<String>,
    pub modified_time: Option<String>,
    pub updated_time: Option<String>,
    pub last_modified: Option<String>,
    pub author: Option<String>,
}
`;
let meta = fs.readFileSync("src/engine/fetcher/meta.rs", "utf8");
let metaStart = meta.indexOf("pub(crate) fn extract_meta");
fs.writeFileSync("src/engine/fetcher/meta.rs", metaHeader + "\n" + meta.slice(metaStart), "utf8");
console.log("Fixed meta.rs");

// 2. In extract.rs: update published_at and modified_at parsing from PageMeta strings
let ex = fs.readFileSync("src/engine/fetcher/extract.rs", "utf8");
ex = ex.replace("published_at: meta.published_time.clone(),", "published_at: meta.published_time.as_deref().and_then(parse_date),");
ex = ex.replace("modified_at: meta.modified_time.clone(),", "modified_at: meta.modified_time.as_deref().or(meta.updated_time.as_deref()).or(meta.last_modified.as_deref()).and_then(parse_date),");
fs.writeFileSync("src/engine/fetcher/extract.rs", ex, "utf8");
console.log("Fixed extract.rs");

// 3. In container.rs: make extract_domain pub(crate)
let c = fs.readFileSync("src/engine/fetcher/container.rs", "utf8");
c = c.replace("fn extract_domain", "pub(crate) fn extract_domain");
fs.writeFileSync("src/engine/fetcher/container.rs", c, "utf8");
console.log("Fixed container.rs");

// 4. In reddit.rs: import MIN_VALID_WORDS
let red = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
red = red.replace("use super::Fetcher;", "use super::{Fetcher, MIN_VALID_WORDS};");
fs.writeFileSync("src/engine/fetcher/reddit.rs", red, "utf8");
console.log("Fixed reddit.rs");

// 5. In text_process.rs:
// - import UnicodeSegmentation
// - fix lol_html RewriteStrSettings to use element_content_handlers: handlers
// - import purge_doc_boilerplate and html_escape
let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
tp = `use unicode_segmentation::UnicodeSegmentation;
use super::container::purge_doc_boilerplate;
use super::url_util::html_escape;
use crate::schema::response::Keyword;
` + tp.replace("use crate::schema::content::Keyword;\n", "");

const oldCleanStreaming = `pub(crate) fn clean_html_streaming(html: &str) -> String {
    let handlers: Vec<lol_html::ElementContentHandlers> = vec![
        (
            std::borrow::Cow::Borrowed("script, style, noscript, svg, nav, footer, header"),
            lol_html::ElementContentHandler::default()
                .element(|el| {
                    el.remove();
                    Ok(())
                }),
        ),
    ];

    let settings = handlers
        .into_iter()
        .fold(lol_html::RewriteStrSettings::new(), |mut s, h| {
            s.append_element_content_handler(h);
            s
        });

    lol_html::rewrite_str(html, settings).unwrap_or_else(|_| html.to_owned())
}`;

const newCleanStreaming = `pub(crate) fn clean_html_streaming(html: &str) -> String {
    let handlers: Vec<lol_html::ElementContentHandlers> = vec![
        (
            std::borrow::Cow::Borrowed("script, style, noscript, svg, nav, footer, header"),
            lol_html::ElementContentHandler::default()
                .element(|el| {
                    el.remove();
                    Ok(())
                }),
        ),
    ];

    let settings = lol_html::RewriteStrSettings {
        element_content_handlers: handlers,
        ..Default::default()
    };

    lol_html::rewrite_str(html, settings).unwrap_or_else(|_| html.to_owned())
}`;

tp = tp.replace(oldCleanStreaming, newCleanStreaming);
fs.writeFileSync("src/engine/fetcher/text_process.rs", tp, "utf8");
console.log("Fixed text_process.rs");
