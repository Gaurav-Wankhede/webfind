import * as fs from "fs";
const fnStr = `
    /// Batch-import many URL nodes and link edges inside a single transaction.
    #[allow(clippy::type_complexity)]
    pub async fn migrate_batch(
        &self,
        nodes: &[(UrlNode, Option<String>, Option<Vec<f32>>)],
        edges: &[(String, String, Option<String>)],
    ) -> Result<(usize, usize)> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .context("begin migration transaction")?;

        // 1. Nodes (with optional excerpt + embedding).
        for (node, excerpt, embedding) in nodes {
            let id = url_id(&node.url);
            let discovered_at = node.discovered_at.to_rfc3339();
            tx.execute(
                r#"
                INSERT INTO url_nodes
                    (id, url, domain, source, depth, priority, lastmod, changefreq,
                     discovered_at, crawled, excerpt, embedding)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                ON CONFLICT(url) DO UPDATE SET
                    domain = excluded.domain,
                    source = excluded.source,
                    depth = excluded.depth,
                    priority = excluded.priority,
                    lastmod = excluded.lastmod,
                    changefreq = excluded.changefreq,
                    crawled = excluded.crawled,
                    excerpt = excluded.excerpt,
                    embedding = excluded.embedding
                "#,
                params![
                    id,
                    node.url.clone(),
                    node.domain.clone(),
                    Self::source_str(node.source),
                    node.depth as i64,
                    node.priority as f64,
                    Self::ts(node.lastmod),
                    node.changefreq.clone(),
                    discovered_at,
                    node.crawled as i64,
                    excerpt.clone().unwrap_or_default(),
                    embedding
                        .clone()
                        .map(embedding_to_bytes)
                        .unwrap_or_default(),
                ],
            )
            .await
            .with_context(|| format!("migrate url node \`{}\`", node.url))?;
        }

        // 2. Edges.
        for (from, to, anchor) in edges {
            tx.execute(
                r#"
                INSERT INTO link_edges
                    (source_id, target_id, anchor_text, recorded_at, edge_type, weight, created_at)
                VALUES (?1, ?2, ?3, ?4, \x27hyperlink\x27, 1.0, ?4)
                "#,
                params![
                    url_id(from),
                    url_id(to),
                    anchor.clone().unwrap_or_default(),
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .await
            .with_context(|| format!("migrate link edge \`{from}\` -> \`{to}\`"))?;
        }

        tx.commit().await.context("commit migration transaction")?;
        self.bump_graph_version().await;
        Ok((nodes.len(), edges.len()))
    }
}
`;

let crud = fs.readFileSync("src/storage/turso_store/crud.rs", "utf8");
let lastBrace = crud.lastIndexOf("}");
if (lastBrace !== -1) {
    crud = crud.slice(0, lastBrace) + fnStr;
    fs.writeFileSync("src/storage/turso_store/crud.rs", crud, "utf8");
    console.log("Added migrate_batch to crud.rs");
}
