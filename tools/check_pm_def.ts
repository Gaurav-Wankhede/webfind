import * as fs from "fs";
const lines = fs.readFileSync("src/engine/fetcher_core_temp.rs", "utf8").split("\n");
console.log(lines.slice(859, 875).join("\n"));
