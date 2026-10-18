use super::{TursoStore, GRAPH_VERSION_KEY};
use anyhow::{Context, Result};
use libsql::params;

impl TursoStore {

    /// Idempotent schema setup.
    pub(crate) async fn init_schema(&self) -> Result<()> {
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
                    embedding     BLOB,
                    simhash       INTEGER
                );

                CREATE TABLE IF NOT EXISTS link_edges (
                    id          INTEGER PRIMARY KEY AUTOINCREMENT,
                    source_id   TEXT NOT NULL REFERENCES url_nodes(id),
                    target_id   TEXT NOT NULL REFERENCES url_nodes(id),
                    anchor_text TEXT,
                    recorded_at TEXT NOT NULL,
                    -- Dependency-graph edge typing. A hyperlink, a keyword
                    -- co-occurrence, a citation, and a parse-tree parent
                    -- relation are fundamentally different relationships and
                    -- must not collapse into one untyped edge. Defaults keep
                    -- pre-existing rows valid as plain hyperlinks.
                    edge_type   TEXT NOT NULL DEFAULT 'hyperlink',
                    weight      REAL NOT NULL DEFAULT 1.0,
                    created_at  TEXT
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
                -- NOTE: the typed-edge index on (edge_type, weight) is created
                -- AFTER the forward migration below, not here. On a database
                -- created before those columns existed, `CREATE TABLE IF NOT
                -- EXISTS` leaves the table untouched and this batch would fail
                -- with "no such column: edge_type" before the ALTER ran.

                -- Token layer: the output of the tokenizer stage. Each row is
                -- one token occurrence in one document, so a token's postings
                -- list is a plain index scan on `text`.
                CREATE TABLE IF NOT EXISTS tokens (
                    id       TEXT PRIMARY KEY,
                    doc_id   TEXT NOT NULL,
                    text     TEXT NOT NULL,
                    kind     TEXT NOT NULL DEFAULT 'word',
                    pos      INTEGER NOT NULL DEFAULT 0,
                    byte_start INTEGER,
                    byte_end   INTEGER
                );
                CREATE INDEX IF NOT EXISTS idx_tokens_text   ON tokens(text);
                CREATE INDEX IF NOT EXISTS idx_tokens_doc    ON tokens(doc_id);

                -- Typed relations between tokens (co-occurrence, dependency).
                CREATE TABLE IF NOT EXISTS token_edges (
                    from_token TEXT NOT NULL,
                    to_token   TEXT NOT NULL,
                    relation   TEXT NOT NULL DEFAULT 'cooccurrence',
                    weight     REAL NOT NULL DEFAULT 1.0,
                    doc_id     TEXT,
                    PRIMARY KEY (from_token, to_token, relation)
                );
                CREATE INDEX IF NOT EXISTS idx_token_edges_from ON token_edges(from_token);
                CREATE INDEX IF NOT EXISTS idx_token_edges_to   ON token_edges(to_token);

                -- Parse-tree layer: one row per named syntax node, linked to
                -- its parent so the tree is reconstructible by a recursive CTE.
                CREATE TABLE IF NOT EXISTS parse_nodes (
                    id         TEXT PRIMARY KEY,
                    doc_id     TEXT NOT NULL,
                    parent_id  TEXT,
                    kind       TEXT NOT NULL,
                    start_byte INTEGER NOT NULL DEFAULT 0,
                    end_byte   INTEGER NOT NULL DEFAULT 0,
                    depth      INTEGER NOT NULL DEFAULT 0
                );
                CREATE INDEX IF NOT EXISTS idx_parse_nodes_doc    ON parse_nodes(doc_id);
                CREATE INDEX IF NOT EXISTS idx_parse_nodes_parent ON parse_nodes(parent_id);

                -- Sparse BM25 vectors (lexical) stored beside the dense
                -- embeddings (semantic), so both retrieval signals live in one
                -- store and fuse via RRF with no second on-disk index.
                CREATE TABLE IF NOT EXISTS sparse_vectors (
                    doc_id  TEXT PRIMARY KEY,
                    terms   BLOB NOT NULL,
                    weights BLOB NOT NULL
                );

                -- Dense embedding vectors, one row per document.
                CREATE TABLE IF NOT EXISTS dense_vectors (
                    doc_id    TEXT PRIMARY KEY,
                    embedding BLOB NOT NULL,
                    dim       INTEGER NOT NULL
                );
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

        // Seed the fts version to match initial graph version so clean empty databases
        // skip redundant FTS rebuilds until actual page content is indexed.
        self.conn
            .execute(
                "INSERT OR IGNORE INTO graph_meta (key, value) VALUES ('fts_version', 1)",
                (),
            )
            .await
            .context("seed fts version")?;

        // Forward migration: ensure the `excerpt` column exists on `url_nodes`.
        // `CREATE TABLE IF NOT EXISTS` cannot retrofit columns onto a pre-existing
        // database, so a file created before this column was added gets upgraded
        // in place rather than erroring at write time.
        self.ensure_column("url_nodes", "excerpt").await?;

        // Forward migration: dependency-graph edge typing. A database created
        // before these columns existed holds only untyped hyperlinks, so the
        // defaults ('hyperlink', 1.0) are the correct backfill — no data
        // rewrite is needed, and existing rows keep their meaning.
        self.ensure_column("link_edges", "edge_type").await?;
        self.ensure_column_typed("link_edges", "weight", "REAL NOT NULL DEFAULT 1.0")
            .await?;
        self.ensure_column("link_edges", "created_at").await?;

        // The typed-edge index must be created only AFTER the columns above
        // exist. Creating it inside the schema batch would fail on any database
        // written before this migration, because `CREATE TABLE IF NOT EXISTS`
        // does not retrofit columns onto an existing table.
        self.conn
            .execute(
                "CREATE INDEX IF NOT EXISTS idx_link_edges_typed \
                 ON link_edges(edge_type, weight DESC)",
                (),
            )
            .await
            .context("create typed-edge index")?;

        Ok(())
    }

    /// Add a column to a table if it does not already exist.
    pub(crate) async fn ensure_column(&self, table: &str, column: &str) -> Result<()> {
        self.ensure_column_typed(table, column, "TEXT").await
    }

    /// Add a column with an explicit type and default, if it does not exist.
    ///
    /// [`ensure_column`] always declares `TEXT`; numeric columns need their own
    /// type and a default so pre-existing rows are valid rather than NULL.
    pub(crate) async fn ensure_column_typed(
        &self,
        table: &str,
        column: &str,
        type_and_default: &str,
    ) -> Result<()> {
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
                .execute(
                    &format!("ALTER TABLE {table} ADD COLUMN {column} {type_and_default}"),
                    (),
                )
                .await
                .with_context(|| format!("add `{column}` column to `{table}`"))?;
        }
        Ok(())
    }
}
