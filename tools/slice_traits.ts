import * as fs from "fs";
const file = "src/storage/turso_store.rs";
let content = fs.readFileSync(file, "utf8");

// Add submodules at top
const modDecl = `pub mod schema;
pub mod crawl_graph;
pub mod audit;
`;
content = content.replace("pub mod schema;\n", modDecl);

// Cut impl CrawlGraphStore through impl FingerprintAuditLog
const targetStart = "#[async_trait]\nimpl CrawlGraphStore for TursoStore {";
const targetEnd = "#[cfg(test)]\nmod tests;";

const startIdx = content.indexOf(targetStart);
const endIdx = content.indexOf(targetEnd);

if (startIdx !== -1 && endIdx !== -1) {
    const updated = content.slice(0, startIdx) + targetEnd + "\n";
    fs.writeFileSync(file, updated, "utf8");
    console.log("Extracted crawl_graph and audit from turso_store.rs!");
} else {
    console.log("Could not find slice markers:", startIdx, endIdx);
}
