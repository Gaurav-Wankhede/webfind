import * as fs from "fs";
const file = "src/storage/turso_store.rs";
let content = fs.readFileSync(file, "utf8");

// Submodule declarations to add
const submods = `pub mod schema;
pub mod crawl_graph;
pub mod audit;
pub mod crud;
pub mod dedup;
pub mod search;
pub mod graph;
`;
content = content.replace(/pub mod schema;[\s\S]*?pub mod audit;\n/, submods);

// 1. Cut crud methods: from "    pub async fn db_size_bytes" up to right before "    /// Build or open the DiskANN"
const crudStart = "    /// On-disk size of the database file in bytes, or the in-memory logical size.\n    pub async fn db_size_bytes";
const crudEnd = "    /// Build or open the DiskANN vector index";

const cIdx1 = content.indexOf(crudStart);
const cIdx2 = content.indexOf(crudEnd);
if (cIdx1 !== -1 && cIdx2 !== -1) {
    content = content.slice(0, cIdx1) + content.slice(cIdx2);
    console.log("Cut crud methods!");
} else {
    console.log("Could not find crud markers:", cIdx1, cIdx2);
}

// 2. Cut search/graph/dedup methods: from "    /// (Re)build the FTS5 index" up to right before "#[cfg(test)]"
const sgdStart = "    /// (Re)build the FTS5 index from `url_nodes`";
const sgdEnd = "#[cfg(test)]\nmod tests;";

const sIdx1 = content.indexOf(sgdStart);
const sIdx2 = content.indexOf(sgdEnd);
if (sIdx1 !== -1 && sIdx2 !== -1) {
    // Keep closing brace of impl TursoStore before tests
    content = content.slice(0, sIdx1) + "}\n\n" + content.slice(sIdx2);
    console.log("Cut search/graph/dedup methods and closed impl TursoStore!");
} else {
    console.log("Could not find sgd markers:", sIdx1, sIdx2);
}

fs.writeFileSync(file, content, "utf8");
