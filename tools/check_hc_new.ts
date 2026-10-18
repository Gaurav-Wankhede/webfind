import * as fs from "fs";
const file = "src/engine/human_client.rs";
const lines = fs.readFileSync(file, "utf8").split("\n");
for (let i = 0; i < lines.length; i++) {
    if (lines[i].includes("pub fn new(")) {
        console.log(`Line ${i+1}: ${lines.slice(i, i+15).join("\n")}`);
        break;
    }
}
