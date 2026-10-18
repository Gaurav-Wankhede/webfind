import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log("Total lines:", lines.length);
for (let i = 0; i < 60; i++) {
    console.log(`${i + 1}: ${lines[i]}`);
}
