use super::*;
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
pub async fn rebuild_fts(&self) -> Result<()> {
        // A bare DELETE clears a regular-content FTS5 table (the `'delete-all'`
        // special command only applies to contentless/external-content tables).
        let _ = self.conn
            .execute("DELETE FROM urls_fts", ())
            .await
            .context("clear FTS index")?;

        let mut rows = self
            .conn
            .query(
                r#"
                SELECT n.url, pc.content_text, pc.excerpt
                FROM page_content pc
                JOIN url_nodes n ON n.id = pc.url_node_id
                WHERE pc.content_text <> ''
                "#,
                (),
            )
            .await
            .context("read content for FTS rebuild")?;

        let mut batch: Vec<(String, String, Option<String>)> = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let text: String = match row.get(1) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let excerpt: Option<String> = row.get(2).unwrap_or(None);
                    batch.push((url, text, excerpt));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate content for FTS rebuild"),
            }
        }

        for (url, text, excerpt) in batch {
            let _ = self.conn
                .execute(
                    "INSERT INTO urls_fts (url, content_text, excerpt) VALUES (?1, ?2, ?3)",
                    params![url, text, excerpt],
                )
                .await
                .context("populate FTS index")?;
        }

        // Record the graph version this FTS index was built from so read-only
        // searches can skip rebuilding it when the graph hasn't changed.
        let version = self.graph_version().await;
        let _ = self.conn
            .execute(
                "INSERT INTO graph_meta (key, value) VALUES ('fts_version', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![version],
            )
            .await
            .context("record fts version")?;

        Ok(())
    }

    /// Graph version the `urls_fts` index was built from, or `"0"` when
    /// never built. Search compares this against `graph_version()` to skip
    /// re-indexing on unchanged graphs.
    pub async fn fts_version(&self) -> String {
        let mut rows = match self
            .conn
            .query(
                "SELECT value FROM graph_meta WHERE key = 'fts_version'",
                (),
            )
            .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("failed to read fts version: {}", e);
                return "0".to_string();
            }
        };
        match rows.next().await {
            Ok(Some(row)) => row.get::<i64>(0).unwrap_or(0).to_string(),
            _ => "0".to_string(),
        }
    }

    /// BM25 full-text search over the FTS5 index (FR-2).
    ///
    /// `query` is an FTS5 query string (e.g. `"rust systems"` for an implicit
    /// AND, or `"\"exact phrase\""`). Returns `(url, score)` where lower
    /// `bm25()` scores rank higher. Call [`Self::rebuild_fts`] after content
    /// writes.
    pub async fn bm25_search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let limit = limit.clamp(1, 500) as i64;

        // Sanitize tokens so raw punctuation or hyphens (e.g. anti-patterns)
        // are treated as quoted terms rather than FTS5 column filters or syntax operators.
        let sanitized_query: String = query
            .split_whitespace()
            .map(|word| {
                let clean: String = word
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                    .collect();
                format!("\"{}\"", clean.replace('"', ""))
            })
            .filter(|token| token != "\"\"")
            .collect::<Vec<_>>()
            .join(" ");

        if sanitized_query.is_empty() {
            return Ok(Vec::new());
        }

        let mut rows = self
            .conn
            .query(
                r#"
                SELECT url, bm25(urls_fts) AS score
                FROM urls_fts
                WHERE urls_fts MATCH ?1
                ORDER BY score
                LIMIT ?2
                "#,
                params![sanitized_query, limit],
            )
            .await
            .context("FTS5 bm25 query")?;

        let mut raw: Vec<(String, f64)> = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = row.get(0).context("read url from FTS5 result")?;
                    let score: f64 = row.get(1).context("read bm25 score from FTS5 result")?;
                    if !url.is_empty() {
                        raw.push((url, score));
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate FTS5 bm25 results"),
            }
        }

        // SQLite FTS5's `bm25()` returns NEGATIVE values where *more negative*
        // means *more relevant*. Every other signal in the pipeline is
        // higher-is-better, so convert to a positive, monotonically increasing
        // relevance score here rather than letting the sign leak downstream
        // (where `ORDER BY score` and max-aggregation both assume positive).
        //
        // Grounded in the Okapi BM25 formulation (Robertson & Zaragoza 2009):
        // BM25 scores are non-negative in the standard derivation, so a
        // non-negative normalised score is the faithful representation.
        if raw.is_empty() {
            return Ok(Vec::new());
        }
        let max_neg = raw
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::INFINITY, f64::min);
        // Shift so the best match becomes 0, then negate so larger = better.
        let shifted: Vec<(String, f64)> = raw
            .into_iter()
            .map(|(url, s)| (url, -(s - max_neg)))
            .collect();
        let max_pos = shifted
            .iter()
            .map(|(_, s)| *s)
            .fold(f64::NEG_INFINITY, f64::max);
        if max_pos <= 1e-12 {
            // All scores identical: uniform relevance rather than dividing by 0.
            return Ok(shifted.into_iter().map(|(u, _)| (u, 1.0)).collect());
        }
        Ok(shifted
            .into_iter()
            .map(|(u, s)| (u, (s / max_pos).clamp(0.0, 1.0)))
            .collect())
    }

    /// Brute-force cosine similarity search over stored embeddings (FR-2).
    ///
    /// libSQL `core` has no `vector_top_k()`, so this scans the stored
    /// float32 embedding blobs and ranks by cosine similarity. Returns
    /// `(url, similarity)` where higher is more similar.
    pub async fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<(String, f64)>> {
        // Use the DiskANN index when it has been built — sublinear ANN search.
        if let Ok(guard) = self.vector_index.lock()
            && let Some(index) = guard.as_ref()
                && !index.is_empty() {
                    return Ok(index.search(query_embedding, limit));
                }

        // Fallback: brute-force cosine over stored embeddings.
        let mut rows = self
            .conn
            .query(
                "SELECT url, embedding FROM url_nodes WHERE embedding IS NOT NULL",
                (),
            )
            .await
            .context("read embeddings for vector search")?;

        let q = query_embedding;
        let q_norm = norm(q);
        let mut scored: Vec<(String, f64)> = Vec::new();

        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let bytes: Vec<u8> = match row.get(1) {
                        Ok(b) => b,
                        Err(_) => continue,
                    };
                    let floats: &[f32] = match bytemuck::try_cast_slice(&bytes) {
                        Ok(f) => f,
                        Err(_) => continue,
                    };
                    if floats.len() != q.len() {
                        continue; // dimension mismatch — skip
                    }
                    scored.push((url, cosine(q, q_norm, floats)));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate embeddings"),
            }
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(limit.clamp(1, 500));
        Ok(scored)
    }

    /// Compute PageRank centrality and persist it to the `pagerank` table.
    ///
    /// Delegates to the in-memory [`compute_pagerank`](crate::engine::crawl_graph::compute_pagerank)
    /// over this store's nodes and edges, then caches the result so hybrid
    /// search can fuse it as a graph signal. Re-run after graph mutations; the
    /// graph version bump signals staleness to consumers.
    /// Cache a query embedding so repeated queries skip the model.
    ///
    /// Stores the vector keyed by the query text in the `embedding_cache` table
    /// created by the metadata schema. Returns `Ok(())` on success; a missing
    /// table degrades to a no-op rather than failing the search.
    pub async fn hybrid_search(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Result<Vec<HybridHit>> {
        const K: f64 = 60.0;
        let limit = limit.clamp(1, 500);

        let bm25 = self.bm25_search(query, limit).await?;
        let vector = match query_embedding {
            Some(e) => self.vector_search(e, limit).await?,
            None => Vec::new(),
        };
        let pagerank = self.pagerank_scores_top(limit).await?;

        // Accumulate RRF scores keyed by URL, tracking each signal separately.
        let mut rrf: std::collections::HashMap<String, HybridHit> =
            std::collections::HashMap::new();
        for (rank, (url, _)) in bm25.iter().enumerate() {
            let rrf_contrib = 1.0 / (K + rank as f64 + 1.0);
            let entry = rrf.entry(url.clone()).or_insert_with(|| HybridHit {
                url: url.clone(),
                score: 0.0,
                signals: Vec::new(),
                bm25_score: None,
                vector_score: None,
                graph_score: None,
                freshness_score: None,
            });
            entry.score += rrf_contrib;
            entry.bm25_score = Some(entry.bm25_score.unwrap_or(0.0) + rrf_contrib);
            if !entry.signals.contains(&"bm25".to_string()) {
                entry.signals.push("bm25".to_string());
            }
        }
        for (rank, (url, _)) in vector.iter().enumerate() {
            let rrf_contrib = 1.0 / (K + rank as f64 + 1.0);
            let entry = rrf.entry(url.clone()).or_insert_with(|| HybridHit {
                url: url.clone(),
                score: 0.0,
                signals: Vec::new(),
                bm25_score: None,
                vector_score: None,
                graph_score: None,
                freshness_score: None,
            });
            entry.score += rrf_contrib;
            entry.vector_score = Some(entry.vector_score.unwrap_or(0.0) + rrf_contrib);
            if !entry.signals.contains(&"vector".to_string()) {
                entry.signals.push("vector".to_string());
            }
        }
        // ── Query-specific vs query-independent signals ──────────────────────
        //
        // RRF (Cormack, Clarke & Büttcher, SIGIR 2009) fuses *rankings of the
        // same query*. PageRank and freshness are GLOBAL rankings over the whole
        // store — they say nothing about this query. Fusing them as RRF lists
        // injected every cached URL into the candidate set with a query-blind
        // contribution, so the most-linked page in the store could outrank the
        // page that actually answers the query. That is a precision defect.
        //
        // Fix: PageRank and freshness are now applied only as *tie-breakers and
        // mild boosts* on candidates the query already retrieved, never as
        // candidate generators.

        // Collect the query-relevant candidate set first.
        let candidate_urls: std::collections::HashSet<String> = rrf.keys().cloned().collect();

        // Graph authority: small additive boost, query-gated to candidates only.
        // Grounded in the BM25F / learning-to-rank practice of treating
        // authority as a feature that breaks ties, not a retrieval signal.
        for (url, score) in pagerank.iter() {
            if !candidate_urls.contains(url) {
                continue;
            }
            let entry = rrf.get_mut(url).expect("candidate present");
            let boost = (score / score.max(1.0)).clamp(0.0, 1.0) * 0.05;
            entry.score += boost;
            entry.graph_score = Some(entry.graph_score.unwrap_or(0.0) + boost);
            if !entry.signals.contains(&"graph".to_string()) {
                entry.signals.push("graph".to_string());
            }
        }

        // Freshness: same treatment — a mild recency boost for candidates only.
        let freshness = self.freshness_scores_top(limit).await?;
        for (url, _) in freshness.iter() {
            if !candidate_urls.contains(url) {
                continue;
            }
            let entry = rrf.get_mut(url).expect("candidate present");
            entry.score += 0.03;
            entry.freshness_score = Some(entry.freshness_score.unwrap_or(0.0) + 0.03);
            if !entry.signals.contains(&"freshness".to_string()) {
                entry.signals.push("freshness".to_string());
            }
        }

        let mut hits: Vec<HybridHit> = rrf.into_values().collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(hits)
    }
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
            .with_context(|| format!("read content info for `{url}`"))?;
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
                       avg((julianday('now') - julianday(discovered_at)))
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
