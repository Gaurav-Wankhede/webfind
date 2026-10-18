import * as fs from "fs";
const file = "src/engine/fetcher/extract.rs";
console.log(fs.readFileSync(file, "utf8").split("\n").slice(0, 30).join("\n"));
