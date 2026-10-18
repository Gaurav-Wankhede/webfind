import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
for (let i = 0; i < lines.length; i++) {
    if (lines[i].includes("try_manifest_fast_path")) {
        console.log(`Line ${i+1}: ${lines[i]}`);
    }
}
