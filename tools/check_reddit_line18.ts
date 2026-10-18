import * as fs from "fs";
const file = "src/engine/fetcher/reddit.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log(lines.slice(12, 35).join("\n"));
