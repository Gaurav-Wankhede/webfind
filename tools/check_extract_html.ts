import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log("Lines 448-460:");
console.log(lines.slice(448, 460).join("\n"));
console.log("Lines 710-725:");
console.log(lines.slice(710, 725).join("\n"));
