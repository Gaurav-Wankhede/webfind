import * as fs from "fs";
const fnStr = `
    pub async fn find_duplicate(&self, url: &str, content_text: &str) -> Result<Option<String>> {
        let words = content_text.split_whitespace().count();
        if words < Self::MIN_DEDUP_WORDS {
            return Ok(None);
        }
        let candidate = crate::storage::cache_store::SimHash::compute(content_text);
        let Some((existing_url, _dist)) = self.find_near_duplicate(candidate, url).await? else {
            return Ok(None);
        };
        // First-seen: compare discovery timestamps.
        let existing_seen = self
            .get_url(&existing_url)
            .await
            .map(|n| n.discovered_at);
        let incoming_seen = self.get_url(url).await.map(|n| n.discovered_at);
        let existing_is_canonical = match (existing_seen, incoming_seen) {
            (Some(e), Some(i)) => e < i || (e == i && existing_url.as_str() < url),
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => true,
        };
        if existing_is_canonical {
            Ok(Some(existing_url))
        } else {
            Ok(None)
        }
    }
`;

let dedup = fs.readFileSync("src/storage/turso_store/dedup.rs", "utf8");
let qv = dedup.indexOf("pub const MIN_QUALITY_WORDS");
if (qv !== -1) {
    dedup = dedup.slice(0, qv) + fnStr + "\n" + dedup.slice(qv);
    fs.writeFileSync("src/storage/turso_store/dedup.rs", dedup, "utf8");
    console.log("Restored find_duplicate in dedup.rs");
}
