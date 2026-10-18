import * as fs from "fs";

// 1. In cleaner.rs, remove trailing `mod paywall;` if any
let cleaner = fs.readFileSync("src/engine/fetcher/cleaner.rs", "utf8");
let pIdx = cleaner.indexOf("mod paywall;");
if (pIdx !== -1) {
    cleaner = cleaner.slice(0, pIdx);
    fs.writeFileSync("src/engine/fetcher/cleaner.rs", cleaner, "utf8");
    console.log("Fixed cleaner.rs");
}

// 2. In container.rs, remove trailing doc comment
let container = fs.readFileSync("src/engine/fetcher/container.rs", "utf8");
let cEnd = container.lastIndexOf("}");
if (cEnd !== -1) {
    container = container.slice(0, cEnd + 1) + "\n";
    fs.writeFileSync("src/engine/fetcher/container.rs", container, "utf8");
    console.log("Fixed container.rs");
}

// 3. In dom_entities.rs, remove trailing doc comment
let dom = fs.readFileSync("src/engine/fetcher/dom_entities.rs", "utf8");
let dEnd = dom.lastIndexOf("}");
if (dEnd !== -1) {
    dom = dom.slice(0, dEnd + 1) + "\n";
    fs.writeFileSync("src/engine/fetcher/dom_entities.rs", dom, "utf8");
    console.log("Fixed dom_entities.rs");
}

// 4. In quality_entities.rs, remove trailing doc comment
let qe = fs.readFileSync("src/engine/fetcher/quality_entities.rs", "utf8");
let qEnd = qe.lastIndexOf("}");
if (qEnd !== -1) {
    qe = qe.slice(0, qEnd + 1) + "\n";
    fs.writeFileSync("src/engine/fetcher/quality_entities.rs", qe, "utf8");
    console.log("Fixed quality_entities.rs");
}

// 5. In extract.rs, move imports outside impl Fetcher and remove trailing doc comment
let extract = fs.readFileSync("src/engine/fetcher/extract.rs", "utf8");
extract = extract.replace(/use super::\*;\s*use std::time::Instant;\s*use anyhow::Result;\s*use scraper::Html;\s*use readability_rust::Readability;\s*use crate::schema::content::\*;\s*/g, "");
extract = extract.replace(/use scraper::Html;\s*use crate::schema::content::\*;\s*/g, "");
let exHeader = `use super::*;
use std::time::Instant;
use anyhow::Result;
use scraper::Html;
use readability_rust::Readability;
use crate::schema::content::*;

impl Fetcher {
`;
let exStart = extract.indexOf("pub fn extract_from_html");
let exEnd = extract.lastIndexOf("entities,");
let exEndBrace = extract.indexOf("}", extract.indexOf("}", exEnd) + 1);
let exBody = extract.slice(exStart, exEndBrace + 1);
fs.writeFileSync("src/engine/fetcher/extract.rs", exHeader + exBody + "\n}\n", "utf8");
console.log("Fixed extract.rs");

// 6. In raw_content.rs, fix structure and closing braces
let raw = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
raw = raw.replace(/use super::\*;\s*use std::time::Instant;\s*use anyhow::\{Context, Result\};\s*use crate::schema::content::\*;\s*/g, "");
raw = raw.replace(/use crate::schema::content::\*;\s*/g, "");
let rawHeader = `use super::*;
use std::time::Instant;
use anyhow::{Context, Result};
use crate::schema::content::*;

impl Fetcher {
`;
let rawStart = raw.indexOf("async fn try_manifest_fast_path");
let rawEnd = raw.lastIndexOf("None\n    }");
let rawEndBrace = raw.indexOf("}", rawEnd);
let rawBody = raw.slice(rawStart, rawEndBrace + 1);
let rawRest = raw.slice(rawEndBrace + 1).trim();
// Remove extra closing brace if present
if (rawRest.endsWith("}")) {
    rawRest = rawRest.slice(0, -1).trim();
}
fs.writeFileSync("src/engine/fetcher/raw_content.rs", rawHeader + rawBody + "\n}\n\n" + rawRest + "\n", "utf8");
console.log("Fixed raw_content.rs");
