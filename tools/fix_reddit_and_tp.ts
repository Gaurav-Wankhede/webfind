import * as fs from "fs";

// Fix reddit.rs: move all use statements outside impl Fetcher
let red = fs.readFileSync("src/engine/fetcher/reddit.rs", "utf8");
red = red.replace(/use [^;]+;\s*/g, "");
const redHeader = `use std::time::Instant;
use anyhow::{Context, Result};
use chrono::Utc;
use crate::schema::content::*;
use super::text_process::{count_sentences, count_words, estimate_reading_time, extract_page_keywords, normalize_text};
use super::url_util::{html_escape, title_from_url};
use super::entities::extract_entities;
use super::Fetcher;

impl Fetcher {
`;
const fnIdx = red.indexOf("pub(crate) async fn fetch_reddit");
const endBrace = red.lastIndexOf("}");
const fnBody = red.slice(fnIdx, endBrace + 1);
fs.writeFileSync("src/engine/fetcher/reddit.rs", redHeader + fnBody + "\n", "utf8");
console.log("Rewrote reddit.rs");

// Fix text_process.rs: ensure closing brace is intact
let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
let lastFn = tp.lastIndexOf("pub(crate) fn text_to_html");
let bodyStart = tp.indexOf("{", lastFn);
let bodyEnd = tp.indexOf("}", bodyStart);
// Look for outer end of function
let closing = tp.lastIndexOf("}");
tp = tp.slice(0, closing + 1) + "\n";
fs.writeFileSync("src/engine/fetcher/text_process.rs", tp, "utf8");
console.log("Rewrote text_process.rs");
