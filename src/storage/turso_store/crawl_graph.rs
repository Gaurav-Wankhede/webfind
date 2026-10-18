use super::*;
use async_trait::async_trait;
use anyhow::{Context, Result};
use chrono::Utc;
use libsql::params;

#[async_trait]
impl CrawlGraphStore for TursoStore {
    async fn record_url(&self, node: UrlNode) {
        let id = url_id(&node.url);
        let discovered_at = node.discovered_at.to_rfc3339();

        // UPSERT keyed on the unique URL: update on re-crawl, insert otherwise.
        let result = self
            .conn
            .execute(
                r#"
                INSERT INTO url_nodes
                    (id, url, domain, source, depth, priority, lastmod, changefreq,
                     discovered_at, crawled)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                ON CONFLICT(url) DO UPDATE SET
                    domain = excluded.domain,
                    source = excluded.source,
                    depth = excluded.depth,
                    priority = excluded.priority,
                    lastmod = excluded.lastmod,
                    changefreq = excluded.changefreq,
                    crawled = excluded.crawled
                "#,
                params![
                    id,
                    node.url,
                    node.domain,
                    Self::source_str(node.source),
                    node.depth as i64,
                    node.priority as f64,
                    Self::ts(node.lastmod),
                    node.changefreq,
                    discovered_at,
                    node.crawled as i64,
                ],
            )
            .await;
        if let Err(e) = result {
            tracing::error!("failed to record url node: {}", e);
            return;
        }
        self.bump_graph_version().await;
    }

    async fn record_link(&self, edge: LinkEdge) {
        // link_edges has FOREIGN KEY references to url_nodes(id). The crawler
        // records edges for URLs that may never become full nodes (e.g.
        // external links that aren't followed, or a link seen before its own
        // page is crawled). Insert a placeholder endpoint first when missing so
        // the edge insert does not trip the FK constraint.
        for endpoint in [&edge.from, &edge.to] {
            let placeholder = self
                .conn
                .execute(
                    "INSERT OR IGNORE INTO url_nodes (id, url, domain, source, depth, priority, lastmod, changefreq, discovered_at, crawled) VALUES (?1, ?2, ?3, 'link_crawl', 0, 0.5, NULL, NULL, ?4, 0)",
                    params![url_id(endpoint), endpoint.as_str(), extract_domain(endpoint), Utc::now().to_rfc3339()],
                )
                .await;
            if let Err(e) = placeholder {
                tracing::debug!("ensure link endpoint {} node: {}", endpoint, e);
            }
        }

        let result = self
            .conn
            .execute(
                r#"
                INSERT INTO link_edges
                    (source_id, target_id, anchor_text, recorded_at, edge_type, weight, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?4)
                "#,
                params![
                    url_id(&edge.from),
                    url_id(&edge.to),
                    edge.anchor_text.clone(),
                    Utc::now().to_rfc3339(),
                    edge.edge_type.as_str(),
                    edge.weight,
                ],
            )
            .await;
        if let Err(e) = result {
            tracing::error!("failed to record link edge: {}", e);
            return;
        }
        self.bump_graph_version().await;
    }

    async fn get_url(&self, url: &str) -> Option<UrlNode> {
        let mut rows = match self
            .conn
            .query(
                r#"
                SELECT url, domain, source, depth, priority, lastmod, changefreq,
                       discovered_at, crawled
                FROM url_nodes
                WHERE url = ?1
                "#,
                params![url],
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to read url node: {}", e);
                return None;
            }
        };

        let row = rows.next().await.ok().flatten()?;
        let url: String = row.get(0).ok()?;
        let domain: String = row.get(1).ok()?;
        let source: String = row.get(2).ok()?;
        let depth: i64 = row.get(3).ok()?;
        let priority: f64 = row.get(4).ok()?;
        let lastmod: Option<String> = row.get(5).ok()?;
        let changefreq: Option<String> = row.get(6).ok()?;
        let discovered_at: String = row.get(7).ok()?;
        let crawled: i64 = row.get(8).ok()?;

        Some(UrlNode {
            url,
            domain,
            source: Self::parse_source(&source),
            depth: depth.max(0) as u32,
            priority: priority as f32,
            lastmod: lastmod.as_deref().and_then(Self::parse_ts),
            changefreq,
            discovered_at: Self::parse_ts(&discovered_at).unwrap_or_else(Utc::now),
            crawled: crawled != 0,
        })
    }

    async fn get_urls(&self) -> Vec<UrlNode> {
        let mut rows = match self
            .conn
            .query(
                r#"
                SELECT url, domain, source, depth, priority, lastmod, changefreq,
                       discovered_at, crawled
                FROM url_nodes
                "#,
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list url nodes: {}", e);
                return Vec::new();
            }
        };

        let mut nodes = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("failed to iterate url nodes: {}", e);
                    break;
                }
            };
            let url: String = match row.get(0) {
                Ok(u) => u,
                Err(e) => {
                    tracing::error!("failed to read url node url: {}", e);
                    continue;
                }
            };
            let domain: String = row.get(1).unwrap_or_default();
            let source: String = row.get(2).unwrap_or_else(|_| "seed".to_string());
            let depth: i64 = row.get(3).unwrap_or(0);
            let priority: f64 = row.get(4).unwrap_or(1.0);
            let lastmod: Option<String> = row.get(5).unwrap_or(None);
            let changefreq: Option<String> = row.get(6).unwrap_or(None);
            let discovered_at: String = row.get(7).unwrap_or_default();
            let crawled: i64 = row.get(8).unwrap_or(0);

            nodes.push(UrlNode {
                url,
                domain,
                source: Self::parse_source(&source),
                depth: depth.max(0) as u32,
                priority: priority as f32,
                lastmod: lastmod.as_deref().and_then(Self::parse_ts),
                changefreq,
                discovered_at: Self::parse_ts(&discovered_at).unwrap_or_else(Utc::now),
                crawled: crawled != 0,
            });
        }
        nodes
    }

    async fn get_links_from(&self, url: &str) -> Vec<LinkEdge> {
        let mut rows = match self
            .conn
            .query(
                r#"
                SELECT l.source_id, l.target_id, l.anchor_text,
                       s.url AS from_url, t.url AS to_url,
                       l.edge_type, l.weight, l.created_at
                FROM link_edges l
                JOIN url_nodes s ON s.id = l.source_id
                JOIN url_nodes t ON t.id = l.target_id
                WHERE s.url = ?1
                "#,
                params![url],
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list outgoing links: {}", e);
                return Vec::new();
            }
        };

        let mut edges = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("failed to iterate outgoing links: {}", e);
                    break;
                }
            };
            let from_url: String = match row.get(3) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let to_url: String = match row.get(4) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let anchor_text: Option<String> = row.get(2).unwrap_or(None);
            let edge_type: EdgeType = row
                .get::<String>(5)
                .map(|s| EdgeType::parse(&s))
                .unwrap_or_default();
            let weight: f64 = row.get(6).unwrap_or(1.0);
            let created_at: Option<DateTime<Utc>> = row
                .get::<String>(7)
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
                edge_type,
                weight,
                created_at,
            });
        }
        edges
    }

    async fn get_links_to(&self, url: &str) -> Vec<LinkEdge> {
        let mut rows = match self
            .conn
            .query(
                r#"
                SELECT l.source_id, l.target_id, l.anchor_text,
                       s.url AS from_url, t.url AS to_url,
                       l.edge_type, l.weight, l.created_at
                FROM link_edges l
                JOIN url_nodes s ON s.id = l.source_id
                JOIN url_nodes t ON t.id = l.target_id
                WHERE t.url = ?1
                "#,
                params![url],
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list incoming links: {}", e);
                return Vec::new();
            }
        };

        let mut edges = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("failed to iterate incoming links: {}", e);
                    break;
                }
            };
            let from_url: String = match row.get(3) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let to_url: String = match row.get(4) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let anchor_text: Option<String> = row.get(2).unwrap_or(None);
            let edge_type: EdgeType = row
                .get::<String>(5)
                .map(|s| EdgeType::parse(&s))
                .unwrap_or_default();
            let weight: f64 = row.get(6).unwrap_or(1.0);
            let created_at: Option<DateTime<Utc>> = row
                .get::<String>(7)
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
                edge_type,
                weight,
                created_at,
            });
        }
        edges
    }

    async fn get_all_links(&self) -> Vec<LinkEdge> {
        let mut rows = match self
            .conn
            .query(
                r#"
                SELECT l.source_id, l.target_id, l.anchor_text,
                       s.url AS from_url, t.url AS to_url,
                       l.edge_type, l.weight, l.created_at
                FROM link_edges l
                JOIN url_nodes s ON s.id = l.source_id
                JOIN url_nodes t ON t.id = l.target_id
                "#,
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to list all links: {}", e);
                return Vec::new();
            }
        };

        let mut edges = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("failed to iterate all links: {}", e);
                    break;
                }
            };
            let from_url: String = match row.get(3) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let to_url: String = match row.get(4) {
                Ok(u) => u,
                Err(_) => continue,
            };
            let anchor_text: Option<String> = row.get(2).unwrap_or(None);
            let edge_type: EdgeType = row
                .get::<String>(5)
                .map(|s| EdgeType::parse(&s))
                .unwrap_or_default();
            let weight: f64 = row.get(6).unwrap_or(1.0);
            let created_at: Option<DateTime<Utc>> = row
                .get::<String>(7)
                .ok()
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
                edge_type,
                weight,
                created_at,
            });
        }
        edges
    }

    async fn graph_version(&self) -> String {
        let mut rows = match self
            .conn
            .query(
                "SELECT value FROM graph_meta WHERE key = ?1",
                params![GRAPH_VERSION_KEY],
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to read graph version: {}", e);
                return "1".to_string();
            }
        };
        match rows.next().await {
            Ok(Some(row)) => row.get::<i64>(0).unwrap_or(1).to_string(),
            _ => "1".to_string(),
        }
    }

    async fn record_page_content(&self, content: PageContentRecord) -> Result<()> {
        let PageContentRecord {
            url_node,
            content_text,
            content_markdown,
            content_html,
            excerpt,
            content_hash,
            word_count,
            reading_time_seconds,
            fetched_at,
            created_at,
        } = content;
        let node_id = Self::bare_node_id(&url_node).to_string();
        let fetched_at = fetched_at.to_rfc3339();
        let created_at = created_at.to_rfc3339();
        // 3-word-shingle SimHash fingerprint for near-duplicate detection at
        // ingest (Charikar 2002; Manku et al. 2007, ≤3-bit threshold).
        let simhash = crate::storage::cache_store::SimHash::compute(&content_text) as i64;

        // Serialize the write so concurrent workers cannot interleave and hit
        // `SQLITE_BUSY` on the embedded file.
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .await
            .context("begin page-content transaction")?;

        let insert = tx
            .execute(
                r#"
                INSERT INTO page_content
                    (id, url_node_id, content_text, content_markdown, content_html, excerpt,
                     content_hash, word_count, reading_time_seconds, fetched_at, created_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
                ON CONFLICT(id) DO UPDATE SET
                    url_node_id           = excluded.url_node_id,
                    content_text          = excluded.content_text,
                    content_markdown      = excluded.content_markdown,
                    content_html          = excluded.content_html,
                    excerpt               = excluded.excerpt,
                    content_hash          = excluded.content_hash,
                    word_count            = excluded.word_count,
                    reading_time_seconds  = excluded.reading_time_seconds,
                    fetched_at            = excluded.fetched_at,
                    created_at            = excluded.created_at
                "#,
                params![
                    content_hash.clone(),
                    node_id.clone(),
                    content_text,
                    content_markdown,
                    content_html,
                    excerpt.clone(),
                    content_hash.clone(),
                    word_count.map(|v| v as i64),
                    reading_time_seconds.map(|v| v as i64),
                    fetched_at,
                    created_at,
                ],
            )
            .await
            .context("insert page content")?;
        if insert == 0 {
            tracing::warn!("page content insert affected 0 rows: {content_hash}");
        }

        // Atomically promote the enriched content fields onto the owning node so
        // searches over `url_nodes` see the latest page without a second lookup.
        let _ = tx
            .execute(
                r#"
                UPDATE url_nodes
                SET crawled = 1,
                    excerpt = COALESCE(?2, excerpt),
                    simhash = ?3
                WHERE id = ?1
                "#,
                params![node_id, excerpt, simhash],
            )
            .await
            .context("attach content excerpt to url node")?;

        // Bump the version inside the same transaction so a reader never observes
        // new content with a stale version token.
        tx.execute(
            "UPDATE graph_meta SET value = value + 1 WHERE key = ?1",
            params![GRAPH_VERSION_KEY],
        )
        .await
        .context("bump graph version in transaction")?;

        tx.commit()
            .await
            .context("commit page-content transaction")?;
        Ok(())
    }

    async fn record_embedding(&self, url: &str, embedding: Vec<f32>) -> Result<()> {
        // Persist as raw little-endian f32 bytes; consuming layers reinterpret it.
        let bytes: Vec<u8> = embedding
            .clone()
            .into_iter()
            .flat_map(|f| f.to_le_bytes())
            .collect();
        let node_id = url_id(url);

        self.conn
            .execute(
                "UPDATE url_nodes SET embedding = ?1 WHERE id = ?2",
                params![bytes, node_id],
            )
            .await
            .context("record embedding")?;

        // Update the DiskANN index if it has been built. The add is done
        // synchronously while holding the lock; the mapping is persisted after
        // the lock is released so we never hold the mutex guard across an await.
        let new_mapping = {
            let mut guard = self
                .vector_index
                .lock()
                .map_err(|_| anyhow::anyhow!("vector index lock poisoned"))?;
            if let Some(index) = guard.as_mut() {
                let id = index.add_vector(url, embedding)?;
                Some((id, url.to_string()))
            } else {
                None
            }
        };
        if let Some((id, url)) = new_mapping {
            crate::storage::diskann_index::save_mapping(self, id, &url).await?;
        }
        self.bump_graph_version().await;
        Ok(())
    }

    async fn enqueue_crawl_job(&self, url: &str) -> Result<String> {
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                r#"
                INSERT INTO crawl_jobs (url, status, attempts, created_at, updated_at)
                VALUES (?1, 'pending', 0, ?2, ?2)
                ON CONFLICT(url) DO UPDATE SET updated_at = excluded.updated_at
                "#,
                params![url, now],
            )
            .await
            .context("enqueue crawl job")?;

        let mut rows = self
            .conn
            .query("SELECT id FROM crawl_jobs WHERE url = ?1", params![url])
            .await
            .context("read crawl job id")?;
        let row = rows.next().await.context("read crawl job row")?;
        match row {
            Some(row) => Ok(row.get::<i64>(0).unwrap_or(0).to_string()),
            None => Ok(url_id(url)),
        }
    }

    async fn dequeue_crawl_jobs(&self, limit: usize) -> Result<Vec<CrawlJob>> {
        // Atomic claim: one UPDATE claims the oldest pending jobs and returns them.
        // Because the status flip and the selection happen in a single statement,
        // concurrent workers can never both receive the same job (BUG-003).
        let now = Utc::now().to_rfc3339();
        let limit = limit.clamp(1, 500) as i64;
        let mut rows = self
            .conn
            .query(
                r#"
                UPDATE crawl_jobs
                SET status = 'processing', updated_at = ?1
                WHERE id IN (
                    SELECT id FROM crawl_jobs
                    WHERE status = 'pending'
                    ORDER BY created_at ASC, id ASC
                    LIMIT ?2
                )
                RETURNING id, url, status, attempts, error
                "#,
                params![now, limit],
            )
            .await
            .context("dequeue crawl jobs")?;

        let mut jobs = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate dequeued crawl jobs"),
            };
            let id: i64 = row.get(0).unwrap_or(0);
            let url: String = match row.get(1) {
                Ok(u) => u,
                Err(e) => return Err(e).context("read dequeued job url"),
            };
            let status: String = row.get(2).unwrap_or_else(|_| "processing".to_string());
            let attempts: i64 = row.get(3).unwrap_or(0);
            let error: Option<String> = row.get(4).unwrap_or(None);
            jobs.push(CrawlJob {
                id: id.to_string(),
                url,
                status,
                attempts: attempts.max(0) as i32,
                error,
            });
        }
        Ok(jobs)
    }

    async fn mark_crawl_job_status(
        &self,
        id: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<()> {
        let id: i64 = id
            .parse()
            .with_context(|| format!("invalid crawl job id `{id}`"))?;
        let now = Utc::now().to_rfc3339();
        self.conn
            .execute(
                r#"
                UPDATE crawl_jobs
                SET status = ?1, error = ?2, attempts = attempts + 1, updated_at = ?3
                WHERE id = ?4
                "#,
                params![status, error, now, id],
            )
            .await
            .context("mark crawl job status")?;
        Ok(())
    }

    /// Traverse the graph with a single recursive CTE (FR-3).
    ///
    /// The recursive step walks `link_edges` in either direction, deduplicating
    /// with `UNION` (cycle-safe) and bounding depth with the `t.depth < ?2`
    /// guard. One SQL query replaces the multi-round-trip BFS.
    async fn traverse_with_depth(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<TraversalNode> {
        let start_id = url_id(start);

        let branches = match direction {
            TraversalDirection::Outbound => {
                "UNION SELECT e.target_id, t.depth + 1 \
                 FROM traversal t JOIN link_edges e ON e.source_id = t.id \
                 WHERE t.depth < ?2"
            }
            TraversalDirection::Inbound => {
                "UNION SELECT e.source_id, t.depth + 1 \
                 FROM traversal t JOIN link_edges e ON e.target_id = t.id \
                 WHERE t.depth < ?2"
            }
            TraversalDirection::Both => {
                "UNION SELECT e.target_id, t.depth + 1 \
                 FROM traversal t JOIN link_edges e ON e.source_id = t.id \
                 WHERE t.depth < ?2 \
                 UNION SELECT e.source_id, t.depth + 1 \
                 FROM traversal t JOIN link_edges e ON e.target_id = t.id \
                 WHERE t.depth < ?2"
            }
        };

        let sql = format!(
            r#"
            WITH RECURSIVE traversal(id, depth) AS (
                SELECT ?1, 0
                {branches}
            )
            SELECT n.url, MIN(t.depth) as min_depth
            FROM traversal t
            JOIN url_nodes n ON n.id = t.id
            GROUP BY n.url
            UNION
            SELECT ?3, 0
            ORDER BY min_depth ASC, url ASC
            "#
        );

        let mut rows = match self
            .conn
            .query(&sql, params![start_id, max_depth as i64, start])
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("recursive CTE traversal failed: {}", e);
                return Vec::new();
            }
        };

        let mut nodes = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url = match row.get::<String>(0) {
                        Ok(u) => u,
                        Err(e) => {
                            tracing::error!("traversal row missing url: {}", e);
                            continue;
                        }
                    };
                    let depth = match row.get::<i64>(1) {
                        Ok(d) => d.max(0) as u32,
                        Err(_) => 0,
                    };
                    nodes.push(TraversalNode { url, depth });
                }
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("traversal iteration failed: {}", e);
                    break;
                }
            }
        }
        nodes
    }

    async fn traverse(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<String> {
        self.traverse_with_depth(start, max_depth, direction)
            .await
            .into_iter()
            .map(|n| n.url)
            .collect()
    }
}
