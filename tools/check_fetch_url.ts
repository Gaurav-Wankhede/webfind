import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log("Lines 160-310:");
for (let i = 160; i < 310; i++) {
    if (lines[i].startsWith("    pub async fn fetch_url") || lines[i].startsWith("    async fn fetch_reddit")) {
        console.log(`Line ${i+1}: ${lines[i]}`);
    }
}
