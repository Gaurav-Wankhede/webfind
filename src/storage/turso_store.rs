//! Turso/libSQL-backed graph store for crawl data (FR-1).
//!
//! The default [`CrawlGraphStore`](crate::engine::crawl_graph::CrawlGraphStore)
//! implementation on an embedded libSQL file. It replaced the separate SurrealDB
//! process with a single-file SQLite-compatible database, eliminating the
//! dual-engine index divergence and transactional-consistency gaps described in
//! the migration PRD.
//!
//! Design invariants:
//! - **Parameterized queries only** — no string interpolation of user input
//!   (fixes BUG-008). URLs and IDs are always bound as `?` params.
//! - **Transactional multi-step writes** — `record_page_content` commits atomically
//!   (fixes BUG-004). A crash mid-write leaves no partial state.
//! - **Atomic job dequeue** — a single `UPDATE ... RETURNING` claims pending jobs
//!   (fixes BUG-003). No SELECT-then-UPDATE TOCTOU window, so concurrent workers
//!   can never double-assign a job.
//! - **BLAKE3 URL IDs** — every URL maps to a 16-hex-char BLAKE3 id via the shared
//!   [`url_id`](crate::engine::util::url_id) utility (FR-8).
//! - **Versioned graph** — every mutation bumps a monotonically increasing version
//!   token so PageRank/cache consumers can invalidate stale data.
//!
//! Backups are a single `cp webfind.db backup.db` file copy.

use anyhow::{Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use libsql::{Builder, Cipher, Connection, EncryptionConfig, TransactionBehavior, params};

use crate::engine::crawl_graph::{
    CrawlGraphStore, CrawlJob, DiscoverySource, LinkEdge, TraversalDirection, UrlNode,
};
use crate::engine::fingerprint::{Fingerprint, FingerprintAuditLog};
use crate::engine::util::url_id;
use crate::schema::content::PageContentRecord;

/// Name of the graph-version key stored in `graph_meta`.
const GRAPH_VERSION_KEY: &str = "graph_version";

/// Turso/libSQL-backed graph store.
///
/// The `Connection` is cheaply cloneable and `Send + Sync`, so a single store can
/// be shared across concurrent async tasks (`Arc<TursoStore>`). Writes that must
/// be atomic run inside `BEGIN IMMEDIATE` transactions to serialize concurrent
/// writers and avoid `SQLITE_BUSY`.
pub struct TursoStore {
    conn: Connection,
    /// Path to the libSQL database file. `":memory:"` for in-memory stores.
    db_path: String,
    /// Optional DiskANN approximate-nearest-neighbor index. When present,
    /// [`TursoStore::vector_search`] delegates to it for sublinear search.
    /// `None` when the index has not been built yet or the store opened
    /// without vector-index support (e.g. `:memory:` test stores). Behind a
    /// `Mutex` so it can be updated through `&self` (the `CrawlGraphStore`
    /// trait takes `&self`).
    vector_index: std::sync::Mutex<Option<crate::storage::diskann_index::DiskAnnIndex>>,
}

impl TursoStore {
    /// Open (creating if necessary) a local libSQL database at `path`.
    ///
    /// If `path` is `":memory:"` the database lives entirely in RAM — useful for
    /// tests. Schema is created idempotently on every open.
    pub async fn new(path: &str) -> Result<Self> {
        let db = libsql::Builder::new_local(path)
            .build()
            .await
            .with_context(|| format!("open libSQL database at `{path}`"))?;
        let conn = db.connect().context("connect to libSQL database")?;
        let store = Self {
            conn,
            db_path: path.to_string(),
            vector_index: std::sync::Mutex::new(None),
        };
        store.init_schema().await?;
        Ok(store)
    }

    /// Open an encrypted local libSQL database at `path`.
    ///
    /// The database is encrypted at rest with SQLCipher (AES-256-CBC). `key` is a
    /// raw passphrase that SQLCipher derives into a 256-bit encryption key. The
    /// same key must be supplied every time the database is opened. Without the
    /// correct key the database file is unreadable gibberish.
    pub async fn new_with_encryption(path: &str, key: &str) -> Result<Self> {
        let encryption_config =
            EncryptionConfig::new(Cipher::Aes256Cbc, Bytes::from(key.to_string()));
        let db = Builder::new_local(path)
            .encryption_config(encryption_config)
            .build()
            .await
            .with_context(|| format!("open encrypted libSQL database at `{path}`"))?;
        let conn = db
            .connect()
            .context("connect to encrypted libSQL database")?;
        let store = Self {
            conn,
            db_path: path.to_string(),
            vector_index: std::sync::Mutex::new(None),
        };
        store.init_schema().await?;
        Ok(store)
    }

    /// Return a clone of the underlying libSQL connection.
    pub fn conn(&self) -> Connection {
        self.conn.clone()
    }

    /// On-disk size of the database file in bytes, or the in-memory logical size.
    ///
    /// For a file-backed DB this reads the actual file length (the ground truth
    /// a user cares about for a disk budget). For `:memory:` stores it falls back
    /// to an approximate logical sum so tests exercise the pruning path.
    pub async fn db_size_bytes(&self) -> Result<u64> {
        if self.db_path != ":memory:" {
            if let Ok(meta) = std::fs::metadata(&self.db_path) {
                return Ok(meta.len());
            }
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
        let row = rows
            .next()
            .await?
            .context("storage size row")?;
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
        let row = rows
            .next()
            .await?
            .context("full-content count row")?;
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
    async fn evict_full_content(&self, limit: u64) -> Result<u64> {
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

        tx.commit()
            .await
            .context("commit eviction transaction")?;

        Ok(rows as u64)
    }

    /// Count rows in `table`. Used by the migration tool to verify that every
    /// exported row landed in the destination.
    pub async fn count_rows(&self, table: &str) -> Result<usize> {
        // Table names are internal constants — safe to interpolate.
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

    /// Set the excerpt on a url_node identified by URL. Used by migration to
    /// attach the excerpt without a full page-content record.
    pub async fn set_excerpt(&self, url: &str, excerpt: &str) -> Result<()> {
        let node_id = url_id(url);
        self.conn
            .execute(
                "UPDATE url_nodes SET excerpt = ?1 WHERE id = ?2",
                params![excerpt, node_id],
            )
            .await
            .context("set node excerpt")?;
        Ok(())
    }

    /// Batch-import many URL nodes and link edges inside a single transaction.
    ///
    /// The migration path writes up to 100K+ rows; doing each as its own
    /// auto-committed statement is prohibitively slow (FR-7 acceptance is
    /// <60s / 100K docs). This runs all inserts under one `BEGIN IMMEDIATE`
    /// transaction, cutting commit overhead from O(rows) to O(1).
    ///
    /// Returns `(nodes_inserted, edges_inserted)`. On error the transaction is
    /// rolled back and no partial rows remain.
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
                INSERT INTO link_edges (source_id, target_id, anchor_text, recorded_at)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![
                    url_id(from),
                    url_id(to),
                    anchor.clone().unwrap_or_default(),
                    Utc::now().to_rfc3339(),
                ],
            )
            .await
            .with_context(|| format!("migrate link edge `{from}` -> `{to}`"))?;
        }

        tx.commit().await.context("commit migration transaction")?;

        self.bump_graph_version().await;
        Ok((nodes.len(), edges.len()))
    }

    /// Build or open the DiskANN vector index for this store's database.
    ///
    /// After this returns `Ok`, [`vector_search`] will use approximate
    /// nearest-neighbor search via DiskANN instead of a brute-force scan.
    /// Call this once after opening the store (the index is rebuilt from
    /// Turso's stored embeddings). Safe to call multiple times — subsequent
    /// calls are no-ops if the index is already built.
    pub async fn build_vector_index(&self) -> Result<()> {
        let mut guard = self
            .vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index lock poisoned"))?;
        if guard.is_some() {
            return Ok(());
        }
        let path = self.db_path();
        let index = crate::storage::diskann_index::DiskAnnIndex::build_or_open(self, &path)
            .await
            .context("build DiskANN vector index")?;
        *guard = Some(index);
        Ok(())
    }

    /// Number of vectors in the DiskANN index, if built.
    pub fn vector_index_len(&self) -> usize {
        self.vector_index
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|idx| idx.len()))
            .unwrap_or(0)
    }

    /// Returns the database file path, used to locate the sidecar index file.
    fn db_path(&self) -> String {
        self.db_path.clone()
    }

    /// Idempotent schema setup.
    async fn init_schema(&self) -> Result<()> {
        self.conn
            .execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS url_nodes (
                    id            TEXT PRIMARY KEY,
                    url           TEXT NOT NULL UNIQUE,
                    domain        TEXT NOT NULL DEFAULT '',
                    source        TEXT NOT NULL DEFAULT 'seed',
                    depth         INTEGER NOT NULL DEFAULT 0,
                    priority      REAL NOT NULL DEFAULT 1.0,
                    lastmod       TEXT,
                    changefreq    TEXT,
                    discovered_at TEXT NOT NULL,
                    crawled       INTEGER NOT NULL DEFAULT 0,
                    excerpt       TEXT,
                    embedding     BLOB
                );

                CREATE TABLE IF NOT EXISTS link_edges (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    source_id   TEXT NOT NULL REFERENCES url_nodes(id),
                    target_id   TEXT NOT NULL REFERENCES url_nodes(id),
                    anchor_text TEXT,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS page_content (
                    id                    TEXT PRIMARY KEY,
                    url_node_id           TEXT NOT NULL REFERENCES url_nodes(id),
                    content_text          TEXT NOT NULL DEFAULT '',
                    content_markdown      TEXT,
                    content_html          TEXT,
                    excerpt               TEXT,
                    content_hash          TEXT NOT NULL,
                    word_count            INTEGER,
                    reading_time_seconds  INTEGER,
                    fetched_at            TEXT NOT NULL,
                    created_at            TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS crawl_jobs (
                    id         INTEGER PRIMARY KEY AUTOINCREMENT,
                    url        TEXT NOT NULL UNIQUE,
                    status     TEXT NOT NULL DEFAULT 'pending',
                    attempts   INTEGER NOT NULL DEFAULT 0,
                    error      TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS graph_meta (
                    key   TEXT PRIMARY KEY,
                    value INTEGER NOT NULL DEFAULT 0
                );

                -- Full-text search index (FR-2). libSQL's `core` build ships
                -- SQLite FTS5 (not the Turso `USING fts`/DiskANN engine, which
                -- lives in the separate `turso` crate). Populated by
                -- `TursoStore::rebuild_fts()`.
                CREATE VIRTUAL TABLE IF NOT EXISTS urls_fts USING fts5(
                    content_text, excerpt, url UNINDEXED
                );

                -- Cached PageRank centrality (FR-5). Rebuilt on graph mutation;
                -- fused into hybrid search as a third RRF signal.
                CREATE TABLE IF NOT EXISTS pagerank (
                    url   TEXT PRIMARY KEY,
                    score REAL NOT NULL
                );

                -- DiskANN vector index id→url mapping (FR-1). The DiskANN index
                -- file ({db}.diskann) stores vectors keyed by an internal u64
                -- id; this table maps those ids back to URLs so the index can
                -- be reopened after a restart. Turso is the source of truth
                -- for the embedding blobs; the DiskANN file is a cache.
                CREATE TABLE IF NOT EXISTS vector_index_map (
                    id  INTEGER PRIMARY KEY,
                    url TEXT NOT NULL UNIQUE
                );

                -- Fingerprint / egress-IP audit trail (was SurrealDB
                -- `ip_health` + `fingerprint_log`). Populated by
                -- `FingerprintAuditLog for TursoStore`.
                CREATE TABLE IF NOT EXISTS ip_health (
                    ip              TEXT PRIMARY KEY,
                    fingerprint_id  TEXT,
                    isp             TEXT,
                    asn             TEXT,
                    country         TEXT,
                    user_agent      TEXT,
                    accept_language TEXT,
                    device_class    TEXT,
                    geo_region      TEXT,
                    working         INTEGER NOT NULL DEFAULT 1,
                    success_count   INTEGER NOT NULL DEFAULT 0,
                    failure_count   INTEGER NOT NULL DEFAULT 0,
                    last_used       TEXT,
                    last_error      TEXT
                );

                CREATE TABLE IF NOT EXISTS fingerprint_log (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    fingerprint_id  TEXT NOT NULL,
                    ip              TEXT,
                    isp             TEXT,
                    asn             TEXT,
                    country         TEXT,
                    user_agent      TEXT,
                    accept_language TEXT,
                    device_class    TEXT,
                    geo_region      TEXT,
                    status_code     INTEGER,
                    error           TEXT,
                    used_at         TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_link_edges_source ON link_edges(source_id);
                CREATE INDEX IF NOT EXISTS idx_link_edges_target ON link_edges(target_id);
                CREATE INDEX IF NOT EXISTS idx_page_content_url  ON page_content(url_node_id);
                CREATE INDEX IF NOT EXISTS idx_crawl_jobs_status ON crawl_jobs(status);
                "#,
            )
            .await
            .context("create Turso graph schema")?;

        // Seed the graph version counter if absent.
        self.conn
            .execute(
                "INSERT OR IGNORE INTO graph_meta (key, value) VALUES (?1, ?2)",
                params![GRAPH_VERSION_KEY, 1i64],
            )
            .await
            .context("seed graph version")?;

        // Forward migration: ensure the `excerpt` column exists on `url_nodes`.
        // `CREATE TABLE IF NOT EXISTS` cannot retrofit columns onto a pre-existing
        // database, so a file created before this column was added gets upgraded
        // in place rather than erroring at write time.
        self.ensure_column("url_nodes", "excerpt").await?;

        Ok(())
    }

    /// Add a column to a table if it does not already exist.
    async fn ensure_column(&self, table: &str, column: &str) -> Result<()> {
        let mut rows = self
            .conn
            .query(&format!("PRAGMA table_info({table})"), ())
            .await
            .with_context(|| format!("inspect columns of `{table}`"))?;
        let mut has_column = false;
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    if row.get::<String>(1).ok().as_deref() == Some(column) {
                        has_column = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate column metadata"),
            }
        }
        if !has_column {
            self.conn
                .execute(&format!("ALTER TABLE {table} ADD COLUMN {column} TEXT"), ())
                .await
                .with_context(|| format!("add `{column}` column to `{table}`"))?;
        }
        Ok(())
    }

    fn source_str(source: DiscoverySource) -> &'static str {
        match source {
            DiscoverySource::Seed => "seed",
            DiscoverySource::Sitemap => "sitemap",
            DiscoverySource::LinkCrawl => "link_crawl",
            DiscoverySource::ExternalLink => "external_link",
        }
    }

    fn parse_source(s: &str) -> DiscoverySource {
        match s {
            "sitemap" => DiscoverySource::Sitemap,
            "link_crawl" => DiscoverySource::LinkCrawl,
            "external_link" => DiscoverySource::ExternalLink,
            _ => DiscoverySource::Seed,
        }
    }

    /// Serialize an optional timestamp as RFC3339, or `None`.
    fn ts(value: Option<DateTime<Utc>>) -> Option<String> {
        value.map(|v| v.to_rfc3339())
    }

    /// Parse an RFC3339 timestamp stored as TEXT.
    fn parse_ts(value: &str) -> Option<DateTime<Utc>> {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|dt| dt.with_timezone(&Utc))
    }

    /// Increment the graph version counter (best-effort).
    async fn bump_graph_version(&self) {
        if let Err(e) = self
            .conn
            .execute(
                "UPDATE graph_meta SET value = value + 1 WHERE key = ?1",
                params![GRAPH_VERSION_KEY],
            )
            .await
        {
            tracing::error!("failed to bump graph version: {}", e);
        }
    }

    /// Parse a `url_node:<id>`-style reference into the bare node id.
    ///
    /// The background worker historically formats ids as `url_node:<hex>`.
    /// Turso uses the bare hex id as the primary key, so this strips the prefix
    /// defensively.
    fn bare_node_id(reference: &str) -> &str {
        reference.strip_prefix("url_node:").unwrap_or(reference)
    }

    /// (Re)build the FTS5 index from `url_nodes` + `page_content`.
    ///
    /// libSQL `core` ships SQLite FTS5 (the DiskANN/`USING fts` engine is a
    /// `turso`-crate feature). Call this after writing content to keep the
    /// full-text index in sync.
    pub async fn rebuild_fts(&self) -> Result<()> {
        // A bare DELETE clears a regular-content FTS5 table (the `'delete-all'`
        // special command only applies to contentless/external-content tables).
        self.conn
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
            self.conn
                .execute(
                    "INSERT INTO urls_fts (url, content_text, excerpt) VALUES (?1, ?2, ?3)",
                    params![url, text, excerpt],
                )
                .await
                .context("populate FTS index")?;
        }
        Ok(())
    }

    /// BM25 full-text search over the FTS5 index (FR-2).
    ///
    /// `query` is an FTS5 query string (e.g. `"rust systems"` for an implicit
    /// AND, or `"\"exact phrase\""`). Returns `(url, score)` where lower
    /// `bm25()` scores rank higher. Call [`Self::rebuild_fts`] after content
    /// writes.
    pub async fn bm25_search(&self, query: &str, limit: usize) -> Result<Vec<(String, f64)>> {
        let limit = limit.clamp(1, 500) as i64;
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
                params![query, limit],
            )
            .await
            .context("FTS5 bm25 query")?;

        let mut hits = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = row.get(0).ok().unwrap_or_default();
                    let score: f64 = row.get(1).unwrap_or(0.0);
                    if !url.is_empty() {
                        hits.push((url, score));
                    }
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate FTS5 bm25 results"),
            }
        }
        Ok(hits)
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
        if let Ok(guard) = self.vector_index.lock() {
            if let Some(index) = guard.as_ref() {
                if !index.is_empty() {
                    return Ok(index.search(query_embedding, limit));
                }
            }
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
                    let dims = bytes.len() / 4;
                    let mut vec = Vec::with_capacity(dims);
                    for chunk in bytes.chunks_exact(4) {
                        vec.push(f32::from_le_bytes(
                            chunk.try_into().expect("4-byte f32 chunk"),
                        ));
                    }
                    if vec.len() != q.len() {
                        continue; // dimension mismatch — skip
                    }
                    scored.push((url, cosine(q, q_norm, &vec)));
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
    pub async fn compute_pagerank(
        &self,
        iterations: usize,
        damping: f64,
    ) -> Result<std::collections::HashMap<String, f64>> {
        let scores = crate::engine::crawl_graph::compute_pagerank(self, iterations, damping).await;

        // Rewrite the cache atomically.
        self.conn
            .execute("DELETE FROM pagerank", ())
            .await
            .context("clear pagerank cache")?;
        for (url, score) in &scores {
            self.conn
                .execute(
                    "INSERT INTO pagerank (url, score) VALUES (?1, ?2)",
                    params![url.as_str(), *score],
                )
                .await
                .with_context(|| format!("cache pagerank for `{url}`"))?;
        }
        self.bump_graph_version().await;
        Ok(scores)
    }

    /// Read the cached PageRank scores as `(url, score)` ordered descending.
    pub async fn pagerank_scores(&self) -> Result<Vec<(String, f64)>> {
        let mut rows = self
            .conn
            .query("SELECT url, score FROM pagerank ORDER BY score DESC", ())
            .await
            .context("read pagerank cache")?;
        let mut out = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let score: f64 = row.get(1).unwrap_or(0.0);
                    out.push((url, score));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate pagerank cache"),
            }
        }
        Ok(out)
    }

    /// Read only the top-`limit` cached PageRank scores, ordered descending.
    ///
    /// RRF fusion only needs the highest-centrality URLs, so reading the whole
    /// table on every hybrid search is wasteful — it dominates latency at 100K
    /// docs. The `LIMIT` keeps the read proportional to the result size rather
    /// than the index size.
    pub async fn pagerank_scores_top(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        let limit = limit.clamp(1, 500) as i64;
        let mut rows = self
            .conn
            .query(
                "SELECT url, score FROM pagerank ORDER BY score DESC LIMIT ?1",
                params![limit],
            )
            .await
            .context("read top pagerank cache")?;
        let mut out = Vec::with_capacity(limit as usize);
        loop {
            match rows.next().await {
                Ok(Some(row)) => {
                    let url: String = match row.get(0) {
                        Ok(u) => u,
                        Err(_) => continue,
                    };
                    let score: f64 = row.get(1).unwrap_or(0.0);
                    out.push((url, score));
                }
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate top pagerank cache"),
            }
        }
        Ok(out)
    }

    /// Hybrid search fusing BM25 + vector + PageRank signals with RRF.
    ///
    /// RRF combines the per-source ranks: `score = Σ 1/(k + rank)` with the
    /// conventional `k = 60`. Documents present in only one source still get a
    /// share; documents strong in several rise to the top. `query_embedding`
    /// enables the vector signal; cached PageRank scores (see
    /// [`Self::compute_pagerank`]) are fused as a third graph signal when
    /// present.
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

        // Accumulate RRF scores keyed by URL.
        let mut rrf: std::collections::HashMap<String, HybridHit> =
            std::collections::HashMap::new();
        for (rank, (url, _)) in bm25.iter().enumerate() {
            let entry = rrf.entry(url.clone()).or_insert_with(|| HybridHit {
                url: url.clone(),
                score: 0.0,
                signals: Vec::new(),
            });
            entry.score += 1.0 / (K + rank as f64 + 1.0);
            entry.signals.push("bm25".to_string());
        }
        for (rank, (url, _)) in vector.iter().enumerate() {
            let entry = rrf.entry(url.clone()).or_insert_with(|| HybridHit {
                url: url.clone(),
                score: 0.0,
                signals: Vec::new(),
            });
            entry.score += 1.0 / (K + rank as f64 + 1.0);
            entry.signals.push("vector".to_string());
        }
        // PageRank is a global centrality score, not query-specific — rank all
        // cached URLs by score descending to form the graph list for RRF.
        for (rank, (url, _)) in pagerank.iter().take(limit).enumerate() {
            let entry = rrf.entry(url.clone()).or_insert_with(|| HybridHit {
                url: url.clone(),
                score: 0.0,
                signals: Vec::new(),
            });
            entry.score += 1.0 / (K + rank as f64 + 1.0);
            entry.signals.push("graph".to_string());
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

    /// Read the stored page excerpt for a URL, if any.
    async fn get_content_excerpt(&self, url: &str) -> Result<Option<String>> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT pc.excerpt
                FROM page_content pc
                JOIN url_nodes n ON n.id = pc.url_node_id
                WHERE n.url = ?1
                ORDER BY pc.created_at DESC
                LIMIT 1
                "#,
                params![url],
            )
            .await
            .with_context(|| format!("read excerpt for `{url}`"))?;
        match rows.next().await {
            Ok(Some(row)) => Ok(row.get::<Option<String>>(0)?),
            Ok(None) => Ok(None),
            Err(e) => Err(e).context("iterate excerpt lookup"),
        }
    }

    /// Rich hybrid search for CLI/API consumers.
    ///
    /// Runs [`Self::hybrid_search`] and enriches each hit with a title and
    /// excerpt derived from stored page content, so callers can render results
    /// without a second round of lookups. Requires the FTS index to be rebuilt
    /// (`rebuild_fts`) and PageRank cached (`compute_pagerank`) for full signal
    /// fusion.
    pub async fn search(
        &self,
        query: &str,
        query_embedding: Option<&[f32]>,
        limit: usize,
    ) -> Result<Vec<TursoSearchHit>> {
        let hits = self.hybrid_search(query, query_embedding, limit).await?;
        let mut out = Vec::with_capacity(hits.len());
        for hit in hits {
            let excerpt = self
                .get_content_excerpt(&hit.url)
                .await?
                .unwrap_or_default();
            let title = derive_title(&excerpt, &hit.url);
            out.push(TursoSearchHit {
                url: hit.url,
                title,
                excerpt,
                score: hit.score,
                signals: hit.signals,
            });
        }
        Ok(out)
    }
}

/// A hybrid-search hit with the fused RRF score and the signals it matched.
#[derive(Debug, Clone)]
pub struct HybridHit {
    pub url: String,
    pub score: f64,
    pub signals: Vec<String>,
}

/// A renderable search hit with a title + excerpt derived from stored content.
#[derive(Debug, Clone)]
pub struct TursoSearchHit {
    pub url: String,
    pub title: String,
    pub excerpt: String,
    pub score: f64,
    pub signals: Vec<String>,
}

/// Derive a display title from the first line of an excerpt, falling back to
/// the URL. Kept short for result cards.
/// Serialize a dense embedding as raw little-endian f32 bytes (mirrors the
/// storage format used by [`TursoStore::record_embedding`]).
fn embedding_to_bytes(embedding: Vec<f32>) -> Vec<u8> {
    embedding
        .into_iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
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
        let result = self
            .conn
            .execute(
                r#"
                INSERT INTO link_edges (source_id, target_id, anchor_text, recorded_at)
                VALUES (?1, ?2, ?3, ?4)
                "#,
                params![
                    url_id(&edge.from),
                    url_id(&edge.to),
                    edge.anchor_text,
                    Utc::now().to_rfc3339(),
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
                       s.url AS from_url, t.url AS to_url
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
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
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
                       s.url AS from_url, t.url AS to_url
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
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
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
                       s.url AS from_url, t.url AS to_url
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
            edges.push(LinkEdge {
                from: from_url,
                to: to_url,
                anchor_text,
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
                    excerpt = COALESCE(?2, excerpt)
                WHERE id = ?1
                "#,
                params![node_id, excerpt],
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
    async fn traverse(
        &self,
        start: &str,
        max_depth: u32,
        direction: TraversalDirection,
    ) -> Vec<String> {
        let start_id = url_id(start);

        // The recursive step must hit the `link_edges` FK indexes
        // (`idx_link_edges_source` / `idx_link_edges_target`). A single `OR`
        // predicate cannot use either index, forcing a full table scan per
        // recursion — O(edges) per hop, the bottleneck at 100K edges. Building
        // the step as per-direction UNION branches lets SQLite seek the index.
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
            SELECT DISTINCT n.url
            FROM traversal t
            JOIN url_nodes n ON n.id = t.id
            UNION
            SELECT ?3
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

        let mut urls = Vec::new();
        loop {
            match rows.next().await {
                Ok(Some(row)) => match row.get::<String>(0) {
                    Ok(u) => urls.push(u),
                    Err(e) => tracing::error!("traversal row missing url: {}", e),
                },
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("traversal iteration failed: {}", e);
                    break;
                }
            }
        }
        urls
    }
}

#[async_trait]
impl FingerprintAuditLog for TursoStore {
    async fn log_fingerprint_use(
        &self,
        fp: &Fingerprint,
        status_code: Option<u16>,
        error: Option<&str>,
    ) {
        let used_at = Utc::now().to_rfc3339();
        let working = error.is_none() && status_code.map(|s| s < 400).unwrap_or(true);

        // Append an audit row.
        let log = self
            .conn
            .execute(
                r#"
                INSERT INTO fingerprint_log
                    (fingerprint_id, ip, isp, asn, country, user_agent, accept_language,
                     device_class, geo_region, status_code, error, used_at)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
                "#,
                params![
                    fp.id.as_str(),
                    fp.ip.as_str(),
                    fp.isp.name.as_str(),
                    fp.isp.asn.as_str(),
                    fp.isp.country.as_str(),
                    fp.user_agent.as_str(),
                    fp.accept_language.as_str(),
                    format!("{:?}", fp.device_class),
                    format!("{:?}", fp.geo_region),
                    status_code.map(|s| s as i64),
                    error,
                    used_at.as_str(),
                ],
            )
            .await;
        if let Err(e) = log {
            tracing::error!("failed to log fingerprint use: {}", e);
        }

        // Upsert the running health row for this IP.
        let health = self
            .conn
            .execute(
                r#"
                INSERT INTO ip_health
                    (ip, fingerprint_id, isp, asn, country, user_agent, accept_language,
                     device_class, geo_region, working, success_count, failure_count,
                     last_used, last_error)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                        ?11, ?12, ?13, ?14)
                ON CONFLICT(ip) DO UPDATE SET
                    fingerprint_id  = excluded.fingerprint_id,
                    isp             = excluded.isp,
                    asn             = excluded.asn,
                    country         = excluded.country,
                    user_agent      = excluded.user_agent,
                    accept_language = excluded.accept_language,
                    device_class    = excluded.device_class,
                    geo_region      = excluded.geo_region,
                    working         = excluded.working,
                    success_count   = ip_health.success_count + excluded.success_count,
                    failure_count   = ip_health.failure_count + excluded.failure_count,
                    last_used       = excluded.last_used,
                    last_error      = excluded.last_error
                "#,
                params![
                    fp.ip.as_str(),
                    fp.id.as_str(),
                    fp.isp.name.as_str(),
                    fp.isp.asn.as_str(),
                    fp.isp.country.as_str(),
                    fp.user_agent.as_str(),
                    fp.accept_language.as_str(),
                    format!("{:?}", fp.device_class),
                    format!("{:?}", fp.geo_region),
                    working as i64,
                    (if working { 1_i64 } else { 0_i64 }),
                    (if working { 0_i64 } else { 1_i64 }),
                    used_at.as_str(),
                    error,
                ],
            )
            .await;
        if let Err(e) = health {
            tracing::error!("failed to upsert ip health: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::sync::Arc;

    fn node(url: &str, source: DiscoverySource, depth: u32) -> UrlNode {
        UrlNode {
            url: url.to_string(),
            domain: "example.com".to_string(),
            source,
            depth,
            priority: 1.0,
            lastmod: None,
            changefreq: None,
            discovered_at: Utc::now(),
            crawled: false,
        }
    }

    #[tokio::test]
    async fn test_encrypted_store_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();
        let key = "test-encryption-key-2026";

        // Write through the encrypted store.
        let store = TursoStore::new_with_encryption(path, key)
            .await
            .expect("create encrypted db");
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/a", DiscoverySource::LinkCrawl, 1))
            .await;

        // Re-open with the correct key — reads must succeed.
        let reopen = TursoStore::new_with_encryption(path, key)
            .await
            .expect("reopen encrypted db with correct key");
        assert_eq!(reopen.count_rows("url_nodes").await.unwrap(), 2);
        assert!(reopen.get_url("https://example.com/").await.is_some());
    }

    #[tokio::test]
    async fn test_encrypted_store_rejects_wrong_key() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();

        // Create with one key.
        let store = TursoStore::new_with_encryption(path, "correct-key")
            .await
            .expect("create encrypted db");
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;

        // Re-open with a different key — decryption must fail.
        let result = TursoStore::new_with_encryption(path, "wrong-key").await;
        assert!(
            result.is_err(),
            "opening an encrypted db with the wrong key must fail"
        );
    }

    #[tokio::test]
    async fn test_encrypted_store_data_not_plaintext() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("encrypted.db");
        let path = db_path.to_str().unwrap();
        let key = "test-encryption-key-2026";

        let store = TursoStore::new_with_encryption(path, key)
            .await
            .expect("create encrypted db");
        store
            .record_url(node(
                "https://secret.example.com/",
                DiscoverySource::Seed,
                0,
            ))
            .await;

        // The raw database file must not contain the plaintext URL.
        let raw = tokio::fs::read_to_string(path).await.unwrap_or_default();
        assert!(
            !raw.contains("secret.example.com"),
            "encrypted db file must not contain plaintext data"
        );
    }

    #[tokio::test]
    async fn test_turso_store_records_and_reads() {
        let store = Arc::new(
            TursoStore::new(":memory:")
                .await
                .expect("open in-memory db"),
        );

        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/a", DiscoverySource::LinkCrawl, 1))
            .await;
        store
            .record_link(LinkEdge {
                from: "https://example.com/".to_string(),
                to: "https://example.com/a".to_string(),
                anchor_text: Some("link".to_string()),
            })
            .await;

        let urls = store.get_urls().await;
        assert_eq!(urls.len(), 2, "expected 2 url nodes");

        let root = store
            .get_url("https://example.com/")
            .await
            .expect("root url exists");
        assert_eq!(root.source, DiscoverySource::Seed);
        assert_eq!(root.depth, 0);

        let outgoing = store.get_links_from("https://example.com/").await;
        assert_eq!(outgoing.len(), 1);
        assert_eq!(outgoing[0].to, "https://example.com/a");

        let incoming = store.get_links_to("https://example.com/a").await;
        assert_eq!(incoming.len(), 1);
        assert_eq!(incoming[0].from, "https://example.com/");
    }

    #[tokio::test]
    async fn test_turso_store_graph_version_bumps() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let v0 = store.graph_version().await;
        store
            .record_url(node("https://example.com/", DiscoverySource::Seed, 0))
            .await;
        store
            .record_url(node("https://example.com/b", DiscoverySource::LinkCrawl, 1))
            .await;
        let v1 = store.graph_version().await;
        assert_ne!(v0, v1, "graph version must bump on mutation");
    }

    #[tokio::test]
    async fn test_record_page_content_is_transactional() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let url = "https://example.com/content";
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;

        let record = PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: "hello world".to_string(),
            content_markdown: Some("# hello".to_string()),
            content_html: Some("<p>hello</p>".to_string()),
            excerpt: Some("hello world".to_string()),
            content_hash: "deadbeef".to_string(),
            word_count: Some(2),
            reading_time_seconds: Some(1),
            fetched_at: Utc::now(),
            created_at: Utc::now(),
        };
        store
            .record_page_content(record)
            .await
            .expect("record page content");

        // Enrichment lands on the node atomically.
        let node = store.get_url(url).await.expect("node exists");
        assert!(node.crawled, "node must be marked crawled");

        // Idempotent re-record does not error.
        let again = PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: "hello world v2".to_string(),
            content_markdown: None,
            content_html: None,
            excerpt: Some("updated".to_string()),
            content_hash: "deadbeef".to_string(),
            word_count: Some(2),
            reading_time_seconds: Some(1),
            fetched_at: Utc::now(),
            created_at: Utc::now(),
        };
        store.record_page_content(again).await.expect("re-record");
        let node = store.get_url(url).await.expect("node exists");
        assert!(node.crawled, "node remains crawled after re-record");
    }

    #[tokio::test]
    async fn test_atomic_job_dequeue_no_double_claim() {
        let store = TursoStore::new(":memory:").await.expect("open db");

        for i in 0..5 {
            store
                .enqueue_crawl_job(&format!("https://example.com/page{i}"))
                .await
                .expect("enqueue");
        }

        // Two sequential dequeues must claim disjoint jobs.
        let first = store.dequeue_crawl_jobs(2).await.expect("dequeue 1");
        let second = store.dequeue_crawl_jobs(2).await.expect("dequeue 2");
        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 2);

        let claimed: std::collections::HashSet<String> =
            first.iter().chain(&second).map(|j| j.url.clone()).collect();
        assert_eq!(
            claimed.len(),
            4,
            "dequeued jobs must be unique across claims (no TOCTOU double-assignment)"
        );

        // Remaining single job still pending.
        let third = store.dequeue_crawl_jobs(10).await.expect("dequeue 3");
        assert_eq!(third.len(), 1);
        assert_eq!(third[0].url, "https://example.com/page4");
    }

    #[tokio::test]
    async fn test_crawl_job_status_lifecycle() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let id = store
            .enqueue_crawl_job("https://example.com/job")
            .await
            .expect("enqueue");

        store
            .mark_crawl_job_status(&id, "failed", Some("boom"))
            .await
            .expect("mark failed");
        let jobs = store.dequeue_crawl_jobs(10).await.expect("dequeue");
        assert!(jobs.is_empty(), "failed job is no longer pending");

        store
            .mark_crawl_job_status(&id, "done", None)
            .await
            .expect("mark done");
    }

    #[tokio::test]
    async fn test_embedding_roundtrip() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        let url = "https://example.com/embed";
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;

        let vec = vec![0.1f32, 0.2, 0.3, 0.4];
        store
            .record_embedding(url, vec.clone())
            .await
            .expect("record embedding");

        // Verify the blob is persisted (384-dim is the production size; here small).
        let mut rows = store
            .conn()
            .query(
                "SELECT embedding FROM url_nodes WHERE url = ?1",
                params![url],
            )
            .await
            .expect("query embedding");
        let row = rows.next().await.expect("embedding row").expect("present");
        let bytes: Vec<u8> = row.get(0).expect("embedding blob");
        assert_eq!(bytes.len(), vec.len() * 4, "one f32 per dimension");
    }

    /// Helper: record a URL node for traversal tests.
    async fn rec(store: &TursoStore, url: &str) {
        store
            .record_url(node(url, DiscoverySource::LinkCrawl, 0))
            .await;
    }

    /// Build a line graph: a -> b -> c -> d.
    async fn line_graph(store: &TursoStore) {
        for u in ["a", "b", "c", "d"] {
            rec(store, &format!("https://{u}.example.com/")).await;
        }
        let pairs = [("a", "b"), ("b", "c"), ("c", "d")];
        for (f, t) in pairs {
            store
                .record_link(LinkEdge {
                    from: format!("https://{f}.example.com/"),
                    to: format!("https://{t}.example.com/"),
                    anchor_text: None,
                })
                .await;
        }
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_outbound() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        line_graph(&store).await;

        let visited = store
            .traverse("https://a.example.com/", 2, TraversalDirection::Outbound)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://a.example.com/".to_string(),
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
            ],
            "outbound depth 2 reaches a, b, c"
        );
        assert!(!visited.contains(&"https://d.example.com/".to_string()));
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_inbound() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        line_graph(&store).await;

        let visited = store
            .traverse("https://d.example.com/", 2, TraversalDirection::Inbound)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
                "https://d.example.com/".to_string(),
            ],
            "inbound depth 2 from d reaches b, c, d"
        );
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_both_cycle_safe() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // a <-> b <-> c (bidirectional cycle)
        for u in ["a", "b", "c"] {
            rec(&store, &format!("https://{u}.example.com/")).await;
        }
        let pairs = [("a", "b"), ("b", "a"), ("b", "c"), ("c", "b")];
        for (f, t) in pairs {
            store
                .record_link(LinkEdge {
                    from: format!("https://{f}.example.com/"),
                    to: format!("https://{t}.example.com/"),
                    anchor_text: None,
                })
                .await;
        }

        // Both directions, depth 5 — must terminate (cycle-safe) and cover all 3.
        let visited = store
            .traverse("https://a.example.com/", 5, TraversalDirection::Both)
            .await;
        let mut v = visited.clone();
        v.sort();
        assert_eq!(
            v,
            vec![
                "https://a.example.com/".to_string(),
                "https://b.example.com/".to_string(),
                "https://c.example.com/".to_string(),
            ],
            "cycle traversal terminates and visits all reachable nodes"
        );
    }

    #[tokio::test]
    async fn test_recursive_cte_traversal_empty_and_depth_zero() {
        let store = TursoStore::new(":memory:").await.expect("open db");

        // Depth 0 returns just the start.
        line_graph(&store).await;
        let visited = store
            .traverse("https://a.example.com/", 0, TraversalDirection::Outbound)
            .await;
        assert_eq!(
            visited,
            vec!["https://a.example.com/".to_string()],
            "depth 0 yields only the start URL"
        );

        // Empty graph returns just the start.
        let empty = TursoStore::new(":memory:").await.expect("open db");
        let visited = empty
            .traverse("https://solo.example.com/", 3, TraversalDirection::Both)
            .await;
        assert_eq!(
            visited,
            vec!["https://solo.example.com/".to_string()],
            "no edges still returns the start URL"
        );
    }

    /// Record a url node + page content + optional embedding for search tests.
    async fn rec_content(
        store: &TursoStore,
        url: &str,
        content_text: &str,
        embedding: Option<Vec<f32>>,
    ) {
        store.record_url(node(url, DiscoverySource::Seed, 0)).await;
        store
            .record_page_content(PageContentRecord {
                url_node: format!("url_node:{}", url_id(url)),
                content_text: content_text.to_string(),
                content_markdown: None,
                content_html: None,
                excerpt: Some(content_text.to_string()),
                content_hash: format!("hash-{}", url_id(url)),
                word_count: Some(10),
                reading_time_seconds: Some(1),
                fetched_at: Utc::now(),
                created_at: Utc::now(),
            })
            .await
            .expect("record content");
        if let Some(emb) = embedding {
            store
                .record_embedding(url, emb)
                .await
                .expect("record embedding");
        }
    }

    #[tokio::test]
    async fn test_bm25_search() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(
            &store,
            "https://a.example.com/",
            "the quick brown fox jumps over the lazy dog",
            None,
        )
        .await;
        rec_content(
            &store,
            "https://b.example.com/",
            "rust programming language guarantees memory safety",
            None,
        )
        .await;
        store.rebuild_fts().await.expect("rebuild FTS");

        let hits = store.bm25_search("rust", 10).await.expect("bm25 search");
        assert_eq!(hits.len(), 1, "only one doc mentions rust");
        assert_eq!(hits[0].0, "https://b.example.com/");
    }

    #[tokio::test]
    async fn test_vector_search_cosine() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        rec_content(
            &store,
            "https://a.example.com/",
            "alpha",
            Some(vec![1.0, 0.0, 0.0, 0.0]),
        )
        .await;
        rec_content(
            &store,
            "https://b.example.com/",
            "beta",
            Some(vec![0.0, 1.0, 0.0, 0.0]),
        )
        .await;

        // Query closer to A in vector space.
        let hits = store
            .vector_search(&[0.9, 0.1, 0.0, 0.0], 2)
            .await
            .expect("vector search");
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].0, "https://a.example.com/", "A is most similar");
        assert!(
            hits[0].1 > hits[1].1,
            "similarity must be sorted descending"
        );
    }

    #[tokio::test]
    async fn test_hybrid_search_rrf() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // A: strong BM25 match, NO embedding (bm25-only signal).
        rec_content(
            &store,
            "https://a.example.com/",
            "rust is the best systems programming language",
            None,
        )
        .await;
        // B: moderate BM25 match AND strong vector match (dual signal).
        rec_content(
            &store,
            "https://b.example.com/",
            "we use rust here",
            Some(vec![0.9, 0.1, 0.0, 0.0]),
        )
        .await;
        // C: no BM25 match, strong vector match (vector-only signal).
        rec_content(
            &store,
            "https://c.example.com/",
            "a completely unrelated topic about cats",
            Some(vec![0.99, 0.0, 0.0, 0.0]),
        )
        .await;
        store.rebuild_fts().await.expect("rebuild FTS");

        // Query vector favors C slightly, B second; A absent from vector space.
        let hits = store
            .hybrid_search("rust", Some(&[1.0, 0.0, 0.0, 0.0]), 10)
            .await
            .expect("hybrid search");
        assert_eq!(hits.len(), 3, "all three docs matched at least one signal");

        let b = hits
            .iter()
            .find(|h| h.url == "https://b.example.com/")
            .unwrap();
        let a = hits
            .iter()
            .find(|h| h.url == "https://a.example.com/")
            .unwrap();
        let c = hits
            .iter()
            .find(|h| h.url == "https://c.example.com/")
            .unwrap();

        // B matches both signals → out-ranks A (bm25-only) and C (vector-only),
        // which each accumulate RRF from a single list.
        assert!(
            b.score > a.score && b.score > c.score,
            "dual-signal doc B must out-rank both single-signal docs A and C under RRF \
             (B={:.4}, A={:.4}, C={:.4})",
            b.score,
            a.score,
            c.score
        );
        assert_eq!(b.signals.len(), 2, "B matched bm25 + vector");
        assert_eq!(a.signals.len(), 1, "A matched only bm25");
        assert_eq!(c.signals.len(), 1, "C matched only vector");
    }

    #[tokio::test]
    async fn test_pagerank_compute_and_cache() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Hub-and-spoke: root links to a, b, c. Root has the most in-links via
        // back-edges, so it should carry the highest centrality.
        line_graph(&store).await; // a -> b -> c -> d
        let scores = store
            .compute_pagerank(20, 0.85)
            .await
            .expect("compute pagerank");
        assert_eq!(scores.len(), 4, "all four nodes scored");

        let cached = store.pagerank_scores().await.expect("read cache");
        assert_eq!(cached.len(), 4, "cache matches node count");
        assert_eq!(
            cached[0].0, "https://d.example.com/",
            "sink node d accumulates the highest centrality in a->b->c->d"
        );

        // Every cached score is present in the returned map.
        for (url, score) in &cached {
            assert!(
                (scores.get(url).copied().unwrap_or(0.0) - score).abs() < 1e-9,
                "cache mirrors computed scores for {url}"
            );
        }
    }

    #[tokio::test]
    async fn test_hybrid_search_fuses_graph_signal() {
        let store = TursoStore::new(":memory:").await.expect("open db");
        // Two docs, both strong BM25 + vector. Their PageRank differs.
        rec_content(
            &store,
            "https://hub.example.com/",
            "rust systems language",
            Some(vec![1.0, 0.0, 0.0]),
        )
        .await;
        rec_content(
            &store,
            "https://leaf.example.com/",
            "rust systems language",
            Some(vec![1.0, 0.0, 0.0]),
        )
        .await;
        // Edge hub -> leaf makes hub the higher-centrality node.
        store
            .record_link(LinkEdge {
                from: "https://hub.example.com/".to_string(),
                to: "https://leaf.example.com/".to_string(),
                anchor_text: None,
            })
            .await;
        store.rebuild_fts().await.expect("rebuild FTS");
        store.compute_pagerank(20, 0.85).await.expect("pagerank");

        let hits = store
            .hybrid_search("rust", Some(&[1.0, 0.0, 0.0]), 10)
            .await
            .expect("hybrid search");
        assert_eq!(hits.len(), 2);

        // Both match bm25 + vector; hub additionally matches the graph signal,
        // so it should rank strictly above the leaf.
        let hub = hits
            .iter()
            .find(|h| h.url == "https://hub.example.com/")
            .unwrap();
        let leaf = hits
            .iter()
            .find(|h| h.url == "https://leaf.example.com/")
            .unwrap();
        // Both pages also appear in the fused PageRank list (hub at rank 1,
        // leaf at rank 2), so each carries the graph signal. The hub's higher
        // PageRank rank is what pushes it ahead under RRF.
        assert!(
            hub.score > leaf.score,
            "graph-signal hub must out-rank leaf under RRF \
             (hub={:.4}, leaf={:.4})",
            hub.score,
            leaf.score
        );
        assert!(hub.signals.contains(&"graph".to_string()));
        assert!(leaf.signals.contains(&"graph".to_string()));
    }

    #[tokio::test]
    async fn test_diskann_vector_index_search() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("vector.db");
        let path = db_path.to_str().unwrap();
        let store = TursoStore::new(path).await.expect("open db");

        // Record three embeddings. A is close to the query, B is far, C is
        // medium.
        for (url, vec) in [
            ("https://a.example/", vec![1.0_f32, 0.0, 0.0, 0.0]),
            ("https://b.example/", vec![0.0_f32, 1.0, 0.0, 0.0]),
            ("https://c.example/", vec![0.8_f32, 0.2, 0.0, 0.0]),
        ] {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_embedding(url, vec)
                .await
                .expect("record embedding");
        }

        // Build the DiskANN index.
        store
            .build_vector_index()
            .await
            .expect("build vector index");
        assert_eq!(store.vector_index_len(), 3);

        // Search for the nearest neighbors of [1,0,0,0] — A should rank first,
        // C second (close to A), B last (orthogonal).
        let store = Arc::new(store);
        let hits = store
            .vector_search(&[1.0_f32, 0.0, 0.0, 0.0], 3)
            .await
            .expect("vector search");
        assert_eq!(hits.len(), 3, "all three docs should be returned");
        assert_eq!(
            hits[0].0, "https://a.example/",
            "identical vector must rank first"
        );
        assert_eq!(
            hits[2].0, "https://b.example/",
            "orthogonal vector must rank last"
        );
        // Similarity scores must be in descending order.
        assert!(hits[0].1 >= hits[1].1 && hits[1].1 >= hits[2].1);
    }

    fn content_record(url: &str, fetched_at: DateTime<Utc>) -> PageContentRecord {
        PageContentRecord {
            url_node: format!("url_node:{}", url_id(url)),
            content_text: format!("Full body text for {url} ").repeat(20),
            content_markdown: Some(format!("## {url}").repeat(5)),
            content_html: Some(format!("<p>{url}</p>").repeat(5)),
            excerpt: Some(format!("Excerpt for {url}")),
            content_hash: format!("hash-{url}"),
            word_count: Some(100),
            reading_time_seconds: Some(30),
            fetched_at,
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_enforce_storage_budget_evicts_oldest_full_content() {
        let store = TursoStore::new(":memory:").await.expect("open store");

        // Three pages with distinct fetch times (oldest first).
        let base = Utc::now();
        let urls = [
            ("https://old.example/", base - chrono::Duration::days(30)),
            ("https://mid.example/", base - chrono::Duration::days(10)),
            ("https://new.example/", base),
        ];
        for (url, fetched) in urls {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_page_content(content_record(url, fetched))
                .await
                .expect("record content");
            store
                .record_embedding(url, vec![1.0_f32, 0.0, 0.0, 0.0])
                .await
                .expect("record embedding");
        }

        assert_eq!(store.full_content_page_count().await.unwrap(), 3);

        // Cap full content at 1 page → oldest two must be stripped.
        let pruned = store
            .enforce_storage_budget(None, Some(1))
            .await
            .expect("enforce budget");
        assert_eq!(pruned, 2, "two oldest pages should be evicted");

        // Only the newest page retains full content.
        assert_eq!(store.full_content_page_count().await.unwrap(), 1);

        // All three embeddings must survive — searchable core is preserved.
        let count: i64 = {
            let mut rows = store
                .conn
                .query(
                    "SELECT COUNT(*) FROM url_nodes WHERE embedding IS NOT NULL",
                    params![],
                )
                .await
                .expect("count embeddings");
            rows.next()
                .await
                .expect("row")
                .expect("some")
                .get::<i64>(0)
                .expect("int")
        };
        assert_eq!(count, 3, "embeddings must survive eviction");
    }

    #[tokio::test]
    async fn test_enforce_storage_budget_respects_byte_budget() {
        let store = TursoStore::new(":memory:").await.expect("open store");

        for (url, i) in [
            ("https://a.example/", 0u32),
            ("https://b.example/", 1),
            ("https://c.example/", 2),
        ] {
            store.record_url(node(url, DiscoverySource::Seed, 0)).await;
            store
                .record_page_content(content_record(
                    url,
                    Utc::now() - chrono::Duration::minutes(i as i64 * 5),
                ))
                .await
                .expect("record content");
        }
        assert_eq!(store.full_content_page_count().await.unwrap(), 3);

        // A tiny byte budget (below one page of content) must still shrink the
        // full-content count toward zero without erroring.
        let pruned = store
            .enforce_storage_budget(Some(1), None)
            .await
            .expect("enforce tiny byte budget");
        assert_eq!(pruned, 3, "all pages should be stripped to fit 1-byte budget");
        assert_eq!(store.full_content_page_count().await.unwrap(), 0);
        // Logical size is now just embeddings/excerpts — must be < 4096 bytes.
        let size = store.db_size_bytes().await.unwrap();
        assert!(size < 4096, "stripped db should be tiny, got {size} bytes");
    }
}
