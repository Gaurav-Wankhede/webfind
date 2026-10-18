import * as fs from "fs";
let crud = fs.readFileSync("src/storage/turso_store/crud.rs", "utf8");
const fnStr = `    pub async fn count_rows(&self, table: &str) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let mut rows = self
            .conn
            .query(&sql, params![])
            .await
            .with_context(|| format!("count rows in \`{table}\`"))?;
        let row = rows
            .next()
            .await
            .context("read count row")?
            .context("count query returned no rows")?;
        let count: i64 = row.get(0).unwrap_or(0);
        Ok(count.max(0) as usize)
    }
}
`;
const idx = crud.indexOf("pub async fn count_rows");
if (idx !== -1) {
    crud = crud.slice(0, idx) + fnStr;
    fs.writeFileSync("src/storage/turso_store/crud.rs", crud, "utf8");
    console.log("Fixed count_rows in crud.rs");
}
