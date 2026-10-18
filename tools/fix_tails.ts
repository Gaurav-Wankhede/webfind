import * as fs from "fs";

let cleaner = fs.readFileSync("src/engine/fetcher/cleaner.rs", "utf8");
let cEnd = cleaner.lastIndexOf("}");
cleaner = cleaner.slice(0, cEnd + 1) + "\n";
fs.writeFileSync("src/engine/fetcher/cleaner.rs", cleaner, "utf8");
console.log("Trimmed cleaner.rs to last closing brace");

let raw = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
// Look at line 70
const lines = raw.split("\n");
for (let i = 65; i < 75 && i < lines.length; i++) {
    console.log(`raw line ${i+1}: ${lines[i]}`);
}
