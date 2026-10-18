import * as fs from "fs";

// Fix search.rs: clean imports at the top
let search = fs.readFileSync("src/storage/turso_store/search.rs", "utf8");
// Remove any duplicate or inner use statements
search = search.replace(/use anyhow::Result;\s*use libsql::Connection;\s*/g, "");
// Remove the top prefix if duplicated
const searchHeader = `use super::*;
use anyhow::{Context, Result};
use libsql::params;

fn norm(v: &[f32]) -> f64 {
    let sum: f64 = v.iter().map(|x| (*x as f64) * (*x as f64)).sum();
    sum.sqrt()
}

fn cosine(q: &[f32], q_norm: f64, v: &[f32]) -> f64 {
    let dot: f64 = q
        .iter()
        .zip(v.iter())
        .map(|(a, b)| (*a as f64) * (*b as f64))
        .sum();
    let v_norm = norm(v);
    if q_norm <= f64::EPSILON || v_norm <= f64::EPSILON {
        0.0
    } else {
        dot / (q_norm * v_norm)
    }
}

fn derive_title(excerpt: &str, url: &str) -> String {
    let first_line = excerpt
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let candidate = if first_line.is_empty() {
        url.to_string()
    } else {
        first_line.to_string()
    };
    let mut t = candidate.trim();
    if t.len() > 80 {
        t = &t[..80];
    }
    t.to_string()
}

impl TursoStore {
`;

// Also append get_content_info, index_freshness, and search to search.rs
const searchFooter = `
    async fn get_content_info(&self, url: &str) -> Result<(Option<String>, Option<chrono::DateTime<chrono::Utc>>)> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT pc.excerpt, COALESCE(pc.created_at, n.discovered_at)
                FROM url_nodes n
                LEFT JOIN page_content pc ON pc.url_node_id = n.id
                WHERE n.url = ?1
                ORDER BY pc.created_at DESC
                LIMIT 1
                "#,
                params![url],
            )
            .await
            .with_context(|| format!("read content info for \`{url}\`"))?;
        match rows.next().await {
            Ok(Some(row)) => {
                let excerpt: Option<String> = row.get(0).ok().flatten();
                let dt_str: Option<String> = row.get(1).ok().flatten();
                let crawled_at = dt_str.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&chrono::Utc)));
                Ok((excerpt, crawled_at))
            }
            Ok(None) => Ok((None, None)),
            Err(e) => Err(e).context("iterate content info lookup"),
        }
    }

    pub async fn index_freshness(&self) -> Result<crate::schema::response::IndexFreshness> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT min(discovered_at), max(discovered_at),
                       avg((julianday(\x27now\x27) - julianday(discovered_at)))
                FROM url_nodes
                "#,
                (),
            )
            .await
            .context("query index freshness from url_nodes")?;

        match rows.next().await {
            Ok(Some(row)) => {
                let min_s: Option<String> = row.get(0).ok().flatten();
                let max_s: Option<String> = row.get(1).ok().flatten();
                let avg_days: f64 = row.get(2).unwrap_or(0.0);

                let oldest_page = min_s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&chrono::Utc)));
                let newest_page = max_s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok().map(|d| d.with_timezone(&chrono::Utc)));

                Ok(crate::schema::response::IndexFreshness {
                    oldest_page,
                    newest_page,
                    avg_age_days: (avg_days * 10.0).round() / 10.0,
                })
            }
            Ok(None) => Ok(crate::schema::response::IndexFreshness {
                oldest_page: None,
                newest_page: None,
                avg_age_days: 0.0,
            }),
            Err(e) => Err(e).context("iterate index freshness"),
        }
    }

    pub async fn search(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Result<Vec<TursoSearchHit>> {
        let hits = self.hybrid_search(query, query_embedding, limit).await?;
        let mut out = Vec::with_capacity(hits.len());
        for mut hit in hits {
            if hit.graph_score.is_none()
                && let Ok(mut rows) = self
                    .conn
                    .query(
                        r#"
                        SELECT count(*) FROM link_edges e
                        JOIN url_nodes target ON e.target_node_id = target.id
                        WHERE target.url = ?1
                        "#,
                        params![hit.url.clone()],
                    )
                    .await
                && let Ok(Some(row)) = rows.next().await
            {
                let in_degree: i64 = row.get(0).unwrap_or(0);
                if in_degree > 0 {
                    let instant_score = 1.0 / (60.0 + (100.0 / in_degree as f64));
                    hit.graph_score = Some(instant_score);
                    if !hit.signals.contains(&"graph".to_string()) {
                        hit.signals.push("graph".to_string());
                    }
                }
            }

            let (excerpt, crawled_at) = self
                .get_content_info(&hit.url)
                .await?;
            let excerpt = excerpt.unwrap_or_default();
            let title = derive_title(&excerpt, &hit.url);
            out.push(TursoSearchHit {
                url: hit.url,
                title,
                excerpt,
                score: hit.score,
                signals: hit.signals,
                bm25_score: hit.bm25_score,
                vector_score: hit.vector_score,
                graph_score: hit.graph_score,
                crawled_at,
            });
        }
        Ok(out)
    }
}
`;

// Extract inner functions from search.rs
let ftsIdx = search.indexOf("pub async fn rebuild_fts");
let hybridEnd = search.lastIndexOf("Ok(hits)");
let hybridEndBrace = search.indexOf("}", hybridEnd);
let searchBody = search.slice(ftsIdx, hybridEndBrace + 1);

fs.writeFileSync("src/storage/turso_store/search.rs", searchHeader + searchBody + searchFooter, "utf8");
console.log("Rewrote search.rs cleanly!");

// Fix graph.rs: clean imports at the top
let graph = fs.readFileSync("src/storage/turso_store/graph.rs", "utf8");
let graphHeader = `use super::*;
use anyhow::{Context, Result};
use libsql::params;
use libsql::TransactionBehavior;

impl TursoStore {
`;
let prIdx = graph.indexOf("pub async fn compute_pagerank");
let fnEnd = graph.lastIndexOf("Ok(normalized)");
let fnEndBrace = graph.indexOf("}", fnEnd);
let graphBody = graph.slice(prIdx, fnEndBrace + 1);
fs.writeFileSync("src/storage/turso_store/graph.rs", graphHeader + graphBody + "\n}\n", "utf8");
console.log("Rewrote graph.rs cleanly!");
