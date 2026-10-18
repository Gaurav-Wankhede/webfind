import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
for (let i = 56; i < 455; i++) {
    if (lines[i].startsWith("    pub fn ") || lines[i].startsWith("    pub async fn ") || lines[i].startsWith("    fn ") || lines[i].startsWith("    async fn ") || lines[i].startsWith("pub fn ") || lines[i].startsWith("fn ")) {
        console.log(`${i + 1}: ${lines[i]}`);
    }
}
