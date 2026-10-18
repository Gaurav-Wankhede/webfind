import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
for (let i = 448; i < 2860; i++) {
    if (lines[i].startsWith("pub fn ") || lines[i].startsWith("fn ") || lines[i].startsWith("pub(crate) fn ")) {
        console.log(`${i + 1}: ${lines[i]}`);
    }
}
