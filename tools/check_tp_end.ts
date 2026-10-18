import * as fs from "fs";
const file = "src/engine/fetcher/text_process.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log(lines.slice(-30).join("\n"));
