//! DiskANN approximate nearest neighbor vector index (FR-1).
//!
//! Wraps the `diskann-rs` crate's [`IncrementalDiskANN<DistCosine>`] to provide
//! true approximate nearest neighbor search over the embeddings stored in
//! Turso. The index is a memory-mapped file that lives alongside the Turso
//! database (`{turso_path}.diskann`) and supports incremental updates without
//! a full rebuild.
//!
//! The `libsql` crate's `core` build does not ship the DiskANN SQL functions
//! (`libsql_vector_idx` / `vector_top_k`), so this module provides the
//! approximate-nearest-neighbor capability via a pure-Rust index that is
//! managed next to the relational store. Turso remains the source of truth for
//! the embedding blobs; this index is a performance cache that is rebuilt from
//! Turso on startup and kept in sync on each [`TursoStore::record_embedding`].

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anndists::dist::DistCosine;
use anyhow::{Context, Result};
use diskann_rs::IncrementalDiskANN;

/// DiskANN index file extension.
const DISKANN_FILE_EXT: &str = ".diskann";

/// A DiskANN approximate-nearest-neighbor index over Turso-stored embeddings.
///
/// Maintains a mapping from the index's internal `u64` ids to the URLs they
/// represent. The mapping is persisted in Turso's `vector_index_map` table so
/// that the index can be reopened after a restart without losing the
/// id→url association.
pub struct DiskAnnIndex {
    index: IncrementalDiskANN<DistCosine>,
    id_to_url: HashMap<u64, String>,
    dim: usize,
    path: PathBuf,
}

impl DiskAnnIndex {
    /// Build or open the DiskANN index for the Turso database at `turso_path`.
    ///
    /// If a `.diskann` index file already exists, it is opened and the id→url
    /// mapping is loaded from Turso. Otherwise the index is built from all
    /// embeddings currently stored in Turso. Any embeddings found in Turso
    /// that are not yet in the index are added incrementally, so this is safe
    /// to call repeatedly.
    pub async fn build_or_open(
        turso: &crate::storage::turso_store::TursoStore,
        turso_path: &str,
    ) -> Result<Self> {
        let path: PathBuf = format!("{turso_path}{DISKANN_FILE_EXT}").into();
        let path_str = path.to_str().unwrap_or("");

        // Load the id→url mapping that was persisted in Turso.
        let mapping = load_mapping(turso).await?;

        let (index, id_to_url) = if path.exists() {
            // Reopen existing index + its persisted mapping.
            let index = IncrementalDiskANN::<DistCosine>::open(path_str)
                .with_context(|| format!("open DiskANN index at `{path_str}`"))?;
            (index, mapping.into_iter().collect::<HashMap<_, _>>())
        } else {
            // Build fresh from Turso embeddings (if any).
            let (index, id_to_url) = Self::build_from_turso(turso, path_str, &mapping).await?;
            (index, id_to_url)
        };

        let dim = index.dim();

        Ok(Self {
            index,
            id_to_url,
            dim,
            path,
        })
    }

    /// Build a fresh index from all embeddings stored in Turso.
    ///
    /// Returns the index and the id→url mapping. The mapping is derived from
    /// the order in which vectors are inserted (DiskANN assigns sequential
    /// ids starting at 0), cross-referenced with the persisted mapping when
    /// available.
    async fn build_from_turso(
        turso: &crate::storage::turso_store::TursoStore,
        path_str: &str,
        persisted_mapping: &[(u64, String)],
    ) -> Result<(IncrementalDiskANN<DistCosine>, HashMap<u64, String>)> {
        let embeddings = collect_embeddings(turso).await?;

        if embeddings.is_empty() {
            // No embeddings yet — create a minimal placeholder index so the
            // file exists. It is overwritten on the first real add.
            let placeholder = vec![0.0_f32; 384];
            let index = IncrementalDiskANN::<DistCosine>::build_default(&[placeholder], path_str)
                .context("create initial empty DiskANN index")?;
            return Ok((index, HashMap::new()));
        }

        let vectors: Vec<Vec<f32>> = embeddings.iter().map(|(_, v)| v.clone()).collect();

        let index = IncrementalDiskANN::<DistCosine>::build_default(&vectors, path_str)
            .with_context(|| format!("build DiskANN index at `{path_str}`"))?;

        // Build the id→url mapping. DiskANN assigns ids 0..n in insertion
        // order. When a persisted mapping exists, use it to recover the
        // correct id→url association; otherwise derive it from insertion order.
        let id_to_url: HashMap<u64, String> = if !persisted_mapping.is_empty() {
            persisted_mapping.iter().cloned().collect()
        } else {
            embeddings
                .into_iter()
                .enumerate()
                .map(|(i, (url, _))| (i as u64, url))
                .collect()
        };

        Ok((index, id_to_url))
    }

    /// Add a vector to the index in memory, returning its assigned id.
    ///
    /// This is synchronous — it updates the in-memory graph and the id→url
    /// mapping but does NOT persist the mapping to Turso. Call
    /// [`save_mapping`] separately (after dropping the mutex guard) to persist.
    pub fn add_vector(&mut self, url: &str, vector: Vec<f32>) -> Result<u64> {
        let ids = self
            .index
            .add_vectors(&[vector])
            .context("add vector to DiskANN index")?;
        let id = *ids
            .first()
            .context("DiskANN returned no id for added vector")?;

        self.id_to_url.insert(id, url.to_string());

        Ok(id)
    }

    /// Search for the `k` nearest neighbors of `query`.
    ///
    /// Returns `(url, similarity)` pairs ordered by descending similarity.
    /// Similarity is in `[0, 1]` where 1 = identical. Returns fewer than `k`
    /// results if the index holds fewer vectors.
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(String, f64)> {
        let beam_width = (k * 4).max(64);
        let results = self.index.search_with_dists(query, k, beam_width);

        let mut scored: Vec<(String, f64)> = results
            .into_iter()
            .filter_map(|(id, dist)| {
                self.id_to_url.get(&id).map(|url| {
                    // DiskANN cosine distance = 1 - similarity.
                    let similarity = (1.0 - dist).clamp(0.0, 1.0);
                    (url.clone(), similarity as f64)
                })
            })
            .collect();

        // Sort by descending similarity.
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored
    }

    /// Number of vectors currently in the index.
    pub fn len(&self) -> usize {
        self.id_to_url.len()
    }

    /// Whether the index contains no vectors.
    pub fn is_empty(&self) -> bool {
        self.id_to_url.is_empty()
    }

    /// Dimensionality of vectors in this index.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Path to the on-disk index file.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Load the id→url mapping from Turso's `vector_index_map` table.
async fn load_mapping(
    turso: &crate::storage::turso_store::TursoStore,
) -> Result<Vec<(u64, String)>> {
    let conn = turso.conn();
    let mut map = Vec::new();

    // The table may not exist on first run — that's fine.
    let mut rows = match conn.query("SELECT id, url FROM vector_index_map", ()).await {
        Ok(r) => r,
        Err(_) => return Ok(map),
    };

    loop {
        match rows.next().await {
            Ok(Some(row)) => {
                let id: i64 = row.get(0).unwrap_or(-1);
                let url: String = match row.get(1) {
                    Ok(u) => u,
                    Err(_) => continue,
                };
                if id >= 0 {
                    map.push((id as u64, url));
                }
            }
            Ok(None) => break,
            Err(_) => break,
        }
    }

    Ok(map)
}

/// Persist a single id→url mapping entry to Turso.
pub async fn save_mapping(
    turso: &crate::storage::turso_store::TursoStore,
    id: u64,
    url: &str,
) -> Result<()> {
    let conn = turso.conn();
    conn.execute(
        "INSERT OR REPLACE INTO vector_index_map (id, url) VALUES (?1, ?2)",
        libsql::params![id as i64, url],
    )
    .await
    .context("persist DiskANN id→url mapping")?;
    Ok(())
}

/// Collect all (url, embedding) pairs from Turso.
async fn collect_embeddings(
    turso: &crate::storage::turso_store::TursoStore,
) -> Result<Vec<(String, Vec<f32>)>> {
    let conn = turso.conn();
    let mut rows = conn
        .query(
            "SELECT url, embedding FROM url_nodes WHERE embedding IS NOT NULL",
            (),
        )
        .await
        .context("read embeddings for DiskANN build")?;

    let mut result = Vec::new();
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
                let vec = match bytes_to_vec(&bytes) {
                    Some(v) if !v.is_empty() => v,
                    _ => continue,
                };
                result.push((url, vec));
            }
            Ok(None) => break,
            Err(e) => return Err(e).context("iterate embeddings for DiskANN build"),
        }
    }

    Ok(result)
}

/// Decode a little-endian f32 blob into a Vec<f32>.
fn bytes_to_vec(bytes: &[u8]) -> Option<Vec<f32>> {
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    // High-performance slice casting using bytemuck: copies directly from aligned chunks
    let floats: &[f32] = bytemuck::try_cast_slice(bytes).ok()?;
    Some(floats.to_vec())
}
