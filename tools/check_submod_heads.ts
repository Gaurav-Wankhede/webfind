import * as fs from "fs";
const mods = ["cleaner.rs", "container.rs", "dom_entities.rs", "entities.rs", "extract.rs", "meta.rs", "paywall.rs", "quality_entities.rs", "raw_content.rs", "text_process.rs", "url_util.rs"];
for (const m of mods) {
    const lines = fs.readFileSync("src/engine/fetcher/" + m, "utf8").split("\n").slice(0, 10);
    console.log(`=== ${m} ===`);
    console.log(lines.join("\n"));
}
