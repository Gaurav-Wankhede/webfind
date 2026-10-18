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
use bytes::Bytes;
use chrono::{DateTime, Utc};
use libsql::{Builder, Cipher, Connection, EncryptionConfig, TransactionBehavior, params};


use crate::engine::crawl_graph::{
    CrawlGraphStore, CrawlJob, DiscoverySource, EdgeType, LinkEdge, TraversalDirection,
    TraversalNode, UrlNode,
};
use crate::engine::util::{extract_domain, url_id};
use crate::schema::content::PageContentRecord;

/// Name of the graph-version key stored in `graph_meta`.
pub(crate) const GRAPH_VERSION_KEY: &str = "graph_version";

/// Turso/libSQL-backed graph store.
///
/// The `Connection` is cheaply cloneable and `Send + Sync`, so a single store can
/// be shared across concurrent async tasks (`Arc<TursoStore>`). Writes that must
/// be atomic run inside `BEGIN IMMEDIATE` transactions to serialize concurrent
/// writers and avoid `SQLITE_BUSY`.
pub mod schema;
pub mod crawl_graph;
pub mod audit;
pub mod crud;
pub mod dedup;
pub mod search;
pub mod graph;

pub use dedup::{QualityVerdict, TursoSearchHit, HybridHit};

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
    pub async fn build_vector_index(&self) -> Result<()> {
        {
            let guard = self
                .vector_index
                .lock()
                .map_err(|_| anyhow::anyhow!("vector index lock poisoned"))?;
            if guard.is_some() {
                return Ok(());
            }
        }
        let path = self.db_path();
        let index = crate::storage::diskann_index::DiskAnnIndex::build_or_open(self, &path)
            .await
            .context("build DiskANN vector index")?;
        let mut guard = self
            .vector_index
            .lock()
            .map_err(|_| anyhow::anyhow!("vector index lock poisoned"))?;
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

    fn source_str(source: DiscoverySource) -> &'static str {
        match source {
            DiscoverySource::Seed => "seed",
            DiscoverySource::Sitemap => "sitemap",
            DiscoverySource::LlmsTxt => "llms_txt",
            DiscoverySource::LinkCrawl => "link_crawl",
            DiscoverySource::ExternalLink => "external_link",
            DiscoverySource::LiveSearch => "live_search",
        }
    }

    fn parse_source(s: &str) -> DiscoverySource {
        match s {
            "sitemap" => DiscoverySource::Sitemap,
            "llms_txt" => DiscoverySource::LlmsTxt,
            "link_crawl" => DiscoverySource::LinkCrawl,
            "external_link" => DiscoverySource::ExternalLink,
            "live_search" => DiscoverySource::LiveSearch,
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

}


fn embedding_to_bytes(embedding: Vec<f32>) -> Vec<u8> {
    embedding
        .into_iter()
        .flat_map(|f| f.to_le_bytes())
        .collect()
}

#[cfg(test)]
mod tests;
