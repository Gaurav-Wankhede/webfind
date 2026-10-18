use super::*;
use anyhow::{Context, Result};
use libsql::params;

impl TursoStore {
    /// to an approximate logical sum so tests exercise the pruning path.
    pub async fn db_size_bytes(&self) -> Result<u64> {
        if self.db_path != ":memory:"
            && let Ok(meta) = std::fs::metadata(&self.db_path) {
                return Ok(meta.len());
            }
        // Approximate logical size for in-memory stores: sum of page_content and
        // url_nodes BLOBs/TEXT so pruning still has a signal.
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT
                    (SELECT COALESCE(SUM(LENGTH(content_text) + LENGTH(COALESCE(content_markdown,''))
                                        + LENGTH(COALESCE(content_html,''))), 0) FROM page_content)
                  + (SELECT COALESCE(SUM(LENGTH(COALESCE(embedding,''))), 0) FROM url_nodes)
                "#,
                params![],
            )
            .await
            .context("compute logical storage size")?;
        let row = rows.next().await?.context("storage size row")?;
        Ok(row.get::<i64>(0).unwrap_or(0) as u64)
    }

    /// Count how many pages currently retain full content.
    pub async fn full_content_page_count(&self) -> Result<u64> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT COUNT(*)
                FROM page_content
                WHERE content_text <> '' OR content_markdown IS NOT NULL OR content_html IS NOT NULL
                "#,
                params![],
            )
            .await
            .context("count full-content pages")?;
        let row = rows.next().await?.context("full-content count row")?;
        Ok(row.get::<i64>(0).unwrap_or(0) as u64)
    }

    /// Enforce the storage budget by evicting the bulky full content of the
    /// oldest pages (by `fetched_at`), keeping the searchable core: embedding,
    /// excerpt, link edges, and FTS entry.
    ///
    /// Two knobs, either of which independently triggers eviction:
    /// - `max_bytes`: total DB file must stay under this size.
    /// - `max_full_content_pages`: at most this many pages keep full text.
    ///
    /// Returns the number of pages whose full content was stripped.
    pub async fn enforce_storage_budget(
        &self,
        max_bytes: Option<u64>,
        max_full_content_pages: Option<u64>,
    ) -> Result<u64> {
        let Some(bytes_budget) = max_bytes else {
            // No byte budget — still respect a page cap if configured.
            if let Some(cap) = max_full_content_pages {
                let count = self.full_content_page_count().await?;
                if count <= cap {
                    return Ok(0);
                }
                let excess = count - cap;
                return self.evict_full_content(excess).await;
            }
            return Ok(0);
        };

        let mut pruned = 0u64;
        loop {
            let size = self.db_size_bytes().await?;
            if size <= bytes_budget {
                // Under byte budget; still honor the page cap.
                if let Some(cap) = max_full_content_pages {
                    let count = self.full_content_page_count().await?;
                    if count > cap {
                        pruned += self.evict_full_content(count - cap).await?;
                    }
                }
                return Ok(pruned);
            }
            // Over budget: strip one page at a time until we fit.
            let n = self.evict_full_content(1).await?;
            if n == 0 {
                // Nothing left to evict; can't shrink further.
                return Ok(pruned);
            }
            pruned += n;
        }
    }

    /// Strip the full content (`content_text`, `content_markdown`, `content_html`)
    /// of the `limit` oldest pages, leaving a searchable stub (excerpt retained
    /// on the owning url_node). Returns how many rows were updated.
    pub(crate) async fn evict_full_content(&self, limit: u64) -> Result<u64> {
        let limit = limit.clamp(1, 10_000);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .context("begin eviction transaction")?;

        let rows = tx
            .execute(
                r#"
                UPDATE page_content
                SET content_text = '',
                    content_markdown = NULL,
                    content_html = NULL,
                    word_count = NULL,
                    reading_time_seconds = NULL
                WHERE id IN (
                    SELECT id FROM page_content
                    WHERE content_text <> '' OR content_markdown IS NOT NULL OR content_html IS NOT NULL
                    ORDER BY fetched_at ASC
                    LIMIT ?1
                )
                "#,
                params![limit as i64],
            )
            .await
            .context("evict oldest full content")?;

        tx.execute(
            "UPDATE graph_meta SET value = value + 1 WHERE key = ?1",
            params![GRAPH_VERSION_KEY],
        )
        .await
        .context("bump graph version after eviction")?;

        tx.commit().await.context("commit eviction transaction")?;

        Ok(rows as u64)
    }

    /// Count rows in `table`. Used by the migration tool to verify that every
    /// exported row landed in the destination.
        pub async fn count_rows(&self, table: &str) -> Result<usize> {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        let mut rows = self
            .conn
            .query(&sql, params![])
            .await
            .with_context(|| format!("count rows in `{table}`"))?;
        let row = rows
            .next()
            .await
            .context("read count row")?
            .context("count query returned no rows")?;
        let count: i64 = row.get(0).unwrap_or(0);
        Ok(count.max(0) as usize)
    }

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
            .with_context(|| format!("migrate url node `{}`", node.url))?;
        }

        // 2. Edges.
        for (from, to, anchor) in edges {
            tx.execute(
                r#"
                INSERT INTO link_edges
                    (source_id, target_id, anchor_text, recorded_at, edge_type, weight, created_at)
                VALUES (?1, ?2, ?3, ?4, 'hyperlink', 1.0, ?4)
                "#,
                params![
                    url_id(from),
                    url_id(to),
                    anchor.clone().unwrap_or_default(),
                    chrono::Utc::now().to_rfc3339(),
                ],
            )
            .await
            .with_context(|| format!("migrate link edge `{from}` -> `{to}`"))?;
        }

        tx.commit().await.context("commit migration transaction")?;
        self.bump_graph_version().await;
        Ok((nodes.len(), edges.len()))
    }
}
