import * as fs from "fs";
const file = "src/engine/crawler.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log(lines.slice(345, 362).join("\n"));
