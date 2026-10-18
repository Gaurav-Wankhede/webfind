import * as fs from "fs";
const file = "src/engine/fetcher.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log(lines.slice(160, 305).join("\n"));
