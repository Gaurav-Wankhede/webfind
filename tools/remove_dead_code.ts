import * as fs from "fs";

// 1. In container.rs: remove unused extract_domain
let c = fs.readFileSync("src/engine/fetcher/container.rs", "utf8");
c = c.replace(/fn extract_domain[\s\S]*?\}\n/, "");
c = c.replace(/pub\(crate\) fn extract_domain[\s\S]*?\}\n/, "");
fs.writeFileSync("src/engine/fetcher/container.rs", c, "utf8");
console.log("Removed unused extract_domain from container.rs");

// 2. In raw_content.rs: remove duplicate unused struct PageMeta
let raw = fs.readFileSync("src/engine/fetcher/raw_content.rs", "utf8");
let pmIdx = raw.indexOf("struct PageMeta");
if (pmIdx !== -1) {
    let pmEnd = raw.indexOf("}", pmIdx);
    raw = raw.slice(0, pmIdx) + raw.slice(pmEnd + 1);
    fs.writeFileSync("src/engine/fetcher/raw_content.rs", raw, "utf8");
    console.log("Removed duplicate PageMeta from raw_content.rs");
}

// 3. In text_process.rs: remove unused static RE_* at the bottom (they are in entities.rs where they are actively used!)
let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
let reIdx = tp.indexOf("static RE_EMAIL");
if (reIdx !== -1) {
    let before = tp.slice(0, tp.lastIndexOf("///", reIdx)).trim();
    fs.writeFileSync("src/engine/fetcher/text_process.rs", before + "\n", "utf8");
    console.log("Removed redundant static RE_* from text_process.rs");
}
