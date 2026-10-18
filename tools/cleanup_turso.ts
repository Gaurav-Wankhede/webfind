import * as fs from "fs";

// 1. In crud.rs, remove trailing doc comment
let crud = fs.readFileSync("src/storage/turso_store/crud.rs", "utf8");
let lastFn = crud.lastIndexOf("pub async fn count_rows");
let endOfCountRows = crud.indexOf("}", lastFn);
if (endOfCountRows !== -1) {
    let nextBrace = crud.indexOf("}", endOfCountRows + 1);
    // Find where the function ends and close impl TursoStore
    crud = crud.slice(0, endOfCountRows + 1) + "\n}\n";
    fs.writeFileSync("src/storage/turso_store/crud.rs", crud, "utf8");
    console.log("Cleaned crud.rs");
}

// 2. In dedup.rs, remove trailing doc comment
let dedup = fs.readFileSync("src/storage/turso_store/dedup.rs", "utf8");
let qv = dedup.indexOf("pub enum QualityVerdict");
let endOfQv = dedup.indexOf("}", dedup.indexOf("}", qv) + 1);
let targetDedup = "pub struct TursoSearchHit {";
let endOfHit = dedup.indexOf("}", dedup.indexOf(targetDedup));
if (endOfHit !== -1) {
    dedup = dedup.slice(0, endOfHit + 1) + "\n";
    fs.writeFileSync("src/storage/turso_store/dedup.rs", dedup, "utf8");
    console.log("Cleaned dedup.rs");
}

// 3. In search.rs, add impl TursoStore and remove trailing doc comment
let search = fs.readFileSync("src/storage/turso_store/search.rs", "utf8");
if (!search.includes("impl TursoStore {")) {
    search = `use super::*;
use anyhow::{Context, Result};
use libsql::params;
use chrono::{DateTime, Utc};

impl TursoStore {
` + search;
}
let lastSemi = search.lastIndexOf("Ok(hits)");
if (lastSemi !== -1) {
    let endOfFn = search.indexOf("}", lastSemi);
    search = search.slice(0, endOfFn + 1) + "\n}\n";
    fs.writeFileSync("src/storage/turso_store/search.rs", search, "utf8");
    console.log("Cleaned search.rs");
}

// 4. In graph.rs, add impl TursoStore and remove trailing doc comment
let graph = fs.readFileSync("src/storage/turso_store/graph.rs", "utf8");
if (!graph.includes("impl TursoStore {")) {
    graph = `use super::*;
use anyhow::{Context, Result};
use libsql::params;
use libsql::TransactionBehavior;

impl TursoStore {
` + graph;
}
let lastFreshness = graph.lastIndexOf("Ok(normalized)");
if (lastFreshness !== -1) {
    let endOfFn = graph.indexOf("}", lastFreshness);
    graph = graph.slice(0, endOfFn + 1) + "\n}\n";
    fs.writeFileSync("src/storage/turso_store/graph.rs", graph, "utf8");
    console.log("Cleaned graph.rs");
}

// 5. In turso_store.rs:
// - Cut lines from db_size_bytes up to right before build_vector_index
// - Re-export QualityVerdict, TursoSearchHit, HybridHit
let ts = fs.readFileSync("src/storage/turso_store.rs", "utf8");
const c1 = ts.indexOf("pub async fn db_size_bytes");
const c2 = ts.indexOf("pub async fn build_vector_index");
if (c1 !== -1 && c2 !== -1) {
    ts = ts.slice(0, ts.lastIndexOf("///", c1)) + ts.slice(c2);
    console.log("Cut crud methods from turso_store.rs");
}

// Add re-exports
if (!ts.includes("pub use dedup::{QualityVerdict, TursoSearchHit, HybridHit};")) {
    ts = ts.replace("pub mod graph;\n", "pub mod graph;\n\npub use dedup::{QualityVerdict, TursoSearchHit, HybridHit};\n");
}

// Add embedding_to_bytes helper if missing
if (!ts.includes("fn embedding_to_bytes(")) {
    const helper = `
fn embedding_to_bytes(embedding: Vec<f32>) -> Vec<u8> {
    embedding
        .into_iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}
`;
    ts = ts.replace("#[cfg(test)]", helper + "\n#[cfg(test)]");
}

fs.writeFileSync("src/storage/turso_store.rs", ts, "utf8");
console.log("Updated turso_store.rs");
