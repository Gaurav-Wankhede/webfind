import * as fs from "fs";
const file = "src/engine/fetcher/raw_content.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
for (let i = 0; i < lines.length; i++) {
    if (lines[i].startsWith("pub(crate) fn ") || lines[i].startsWith("pub fn ") || lines[i].startsWith("fn ") || lines[i].startsWith("    async fn ") || lines[i].startsWith("    pub fn ") || lines[i].startsWith("    pub async fn ")) {
        console.log(`Line ${i+1}: ${lines[i]}`);
    }
}
