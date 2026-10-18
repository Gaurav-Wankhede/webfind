import * as fs from "fs";
const file = "src/engine/fetcher_core_temp.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
console.log(lines.slice(90, 137).join("\n"));
