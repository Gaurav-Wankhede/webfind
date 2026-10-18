import * as fs from "fs";
const file = "src/storage/turso_store.rs";
const content = fs.readFileSync(file, "utf8");
const target = "#[cfg(test)]\nmod tests {";
const idx = content.indexOf(target);
if (idx !== -1) {
    const updated = content.slice(0, idx) + "#[cfg(test)]\nmod tests;\n";
    fs.writeFileSync(file, updated, "utf8");
    console.log("Replaced tests in turso_store.rs");
} else {
    console.log("Target not found");
}
