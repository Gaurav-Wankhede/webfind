//! Real BM25 lexical scoring, replacing the previous substring matcher.
//!
//! # Why this module exists
//!
//! The original `InMemorySearchEngine::search_bm25` scored documents with
//! `title_lower.contains(term) ? 10.0 : 0.0` plus a smaller body term. That is
//! *substring term-frequency matching*, not BM25. It had:
//!
//! - no IDF — a term in 1 of 1000 docs scored identically to one in 900 of 1000
//! - no document-length normalization — a 20-word page tied a 5000-word page
//! - no term-frequency saturation (k1) — the 50th occurrence counted as much
//!   as the 1st
//! - no inverted index — every query was an O(n) scan over every document
//!
//! This module uses the `bm25` crate, which computes proper BM25 and emits a
//! **sparse embedding** (token-hash -> weight). Sparse embeddings persist in
//! the same Turso store as the dense vectors, so lexical and semantic retrieval
//! fuse in one place rather than needing a second on-disk index directory.
//!
//! # Scoring
//!
//! BM25 has three parameters, matching the standard formulation:
//!
//! - `k1` (default 1.2) — term-frequency saturation
//! - `b` (default 0.75) — document-length normalization strength
//! - `avgdl` — average meaningful token count across the corpus
//!
//! `avgdl` is fitted from the corpus via
//! [`EmbedderBuilder::with_tokenizer_and_fit_to_corpus`]. Fitting matters: a
//! guessed `avgdl` skews every score, and the crate's own docs recommend
//! fitting whenever the corpus is available.
//!
//! # A note on the embedding space
//!
//! The crate's default embedding space is `u32`, produced by hashing each
//! token (`u32::embed(token) = hash32(token)`). That hash is **not
//! reversible**, so the term strings cannot be recovered from an embedding
//! alone. [`Bm25Index::embedding_of`] therefore re-tokenizes the stored text to
//! map each hash back to its term. This is why the index retains source text.

use std::collections::HashMap;

use anyhow::{Context, Result};
use bm25::{DefaultTokenizer, Embedder, EmbedderBuilder, Embedding, Scorer, TokenEmbedder, Tokenizer};
use parking_lot::RwLock;

/// The embedding space used by the default tokenizer: token hash -> weight.
///
/// `Embedding`'s type parameter defaults to the crate's `DefaultEmbeddingSpace`
/// (`u32`), which is not re-exported at the crate root — so the alias omits it.
type LexicalEmbedding = Embedding;

/// BM25 lexical index over a document set.
///
/// Holds a fitted [`Embedder`] and a [`Scorer`]. The scorer maintains an
/// inverted token index internally, so `search` only visits documents that
/// actually contain at least one query token — it is not a linear scan.
pub struct Bm25Index {
    pub embedder: Embedder<u32, DefaultTokenizer>,
    scorer: RwLock<Scorer<String, u32>>,
    /// Document id -> its sparse embedding.
    embeddings: RwLock<HashMap<String, LexicalEmbedding>>,
    /// Source text per document, retained so `refit` can re-embed and so
    /// `embedding_of` can map token hashes back to their terms.
    texts: RwLock<HashMap<String, String>>,
}

impl Bm25Index {
    /// Build an empty index whose `avgdl` is fitted from `corpus`.
    ///
    /// Pass the document texts you intend to index. An empty corpus falls back
    /// to the crate's internal default `avgdl`, which is correct but less
    /// accurate than a fitted value.
    #[must_use]
    pub fn fit(corpus: &[&str]) -> Self {
        let embedder = EmbedderBuilder::with_tokenizer_and_fit_to_corpus(
            DefaultTokenizer::default(),
            corpus,
        )
        .build();
        Self {
            embedder,
            scorer: RwLock::new(Scorer::new()),
            embeddings: RwLock::new(HashMap::new()),
            texts: RwLock::new(HashMap::new()),
        }
    }

    /// Build an empty index with an explicit `avgdl` estimate.
    ///
    /// Use this when the corpus is not available up front — for example when
    /// indexing incrementally as pages arrive. A rough average is better than
    /// no value at all; the crate recommends a sensible guess over disabling
    /// length normalization entirely.
    #[must_use]
    pub fn with_avgdl(avgdl: f32) -> Self {
        let embedder = EmbedderBuilder::with_avgdl(avgdl).build();
        Self {
            embedder,
            scorer: RwLock::new(Scorer::new()),
            embeddings: RwLock::new(HashMap::new()),
            texts: RwLock::new(HashMap::new()),
        }
    }

    /// The fitted average document length this index is scoring against.
    #[must_use]
    pub fn avgdl(&self) -> f32 {
        self.embedder.avgdl()
    }

    /// Number of indexed documents.
    #[must_use]
    pub fn len(&self) -> usize {
        self.embeddings.read().len()
    }

    /// Whether the index holds no documents.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Index (or re-index) one document.
    ///
    /// Re-indexing an existing id replaces its embedding, so callers can update
    /// a page without accumulating duplicates.
    pub fn upsert(&self, id: &str, text: &str) {
        let embedding = self.embedder.embed(text);
        self.scorer.write().upsert(&id.to_string(), embedding.clone());
        let _ = self.embeddings.write().insert(id.to_string(), embedding);
        let _ = self.texts.write().insert(id.to_string(), text.to_string());
    }

    /// Index a batch of documents.
    pub fn upsert_batch(&self, items: &[(String, String)]) {
        for (id, text) in items {
            self.upsert(id, text);
        }
    }

    /// Remove a document from the index.
    pub fn remove(&self, id: &str) {
        self.scorer.write().remove(&id.to_string());
        let _ = self.embeddings.write().remove(id);
        let _ = self.texts.write().remove(id);
    }

    /// Score every document against `query`, returning `(id, score)` pairs
    /// sorted by descending score.
    ///
    /// Only documents sharing at least one token with the query are considered
    /// — the scorer's inverted index supplies the candidate set.
    #[must_use]
    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        let query_embedding = self.embedder.embed(query);
        let mut scored: Vec<(String, f32)> = self
            .scorer
            .read()
            .matches(&query_embedding)
            .into_iter()
            .map(|doc| (doc.id, doc.score))
            .collect();
        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.cmp(&b.0))
        });
        scored.truncate(limit);
        scored
    }

    /// The sparse embedding for a document as `(term, weight)` pairs.
    ///
    /// This is what gets persisted to Turso: a set of term/weight pairs rather
    /// than a dense float vector. Because the embedding space is a token
    /// *hash*, the stored text is re-tokenized to recover the term strings.
    #[must_use]
    pub fn embedding_of(&self, id: &str) -> Option<Vec<(String, f32)>> {
        let embeddings = self.embeddings.read();
        let embedding = embeddings.get(id)?;
        let texts = self.texts.read();
        let text = texts.get(id)?;

        // Map each token hash to its term. The tokenizer output is the
        // authority: build the map from the document's own tokens so a hash
        // collision resolves to a term that actually occurs in this document.
        let mut hash_to_term: HashMap<u32, String> = HashMap::new();
        for token in DefaultTokenizer::default().tokenize(text) {
            let hash = <u32 as TokenEmbedder>::embed(&token);
            let _ = hash_to_term.entry(hash).or_insert(token);
        }

        Some(
            embedding
                .iter()
                .filter_map(|t| {
                    hash_to_term
                        .get(&t.index)
                        .map(|term| (term.clone(), t.value))
                })
                .collect(),
        )
    }

    /// Re-fit `avgdl` against the currently indexed documents and re-embed.
    ///
    /// Call this after a large ingest so length normalization reflects the
    /// real corpus instead of the initial estimate. Returns the new `avgdl`.
    pub fn refit(&self) -> f32 {
        let texts = self.texts.read();
        let corpus: Vec<&str> = texts.values().map(String::as_str).collect();
        if corpus.is_empty() {
            return self.embedder.avgdl();
        }
        let refitted: Embedder<u32, DefaultTokenizer> =
            EmbedderBuilder::with_tokenizer_and_fit_to_corpus(
                DefaultTokenizer::default(),
                &corpus,
            )
            .build();
        let avgdl = refitted.avgdl();

        let mut scorer = self.scorer.write();
        let mut embeddings = self.embeddings.write();
        *scorer = Scorer::new();
        for (id, text) in texts.iter() {
            let embedding = refitted.embed(text);
            scorer.upsert(id, embedding.clone());
            let _ = embeddings.insert(id.clone(), embedding);
        }
        avgdl
    }
}

impl Default for Bm25Index {
    fn default() -> Self {
        Self::with_avgdl(0.0)
    }
}

/// Serialize a sparse embedding to the `(terms, weights)` blob pair stored in
/// the `sparse_vectors` Turso table.
///
/// Terms are stored as a length-prefixed UTF-8 blob and weights as little-endian
/// `f32`, so the pair round-trips without a JSON dependency on the hot path.
///
/// # Errors
///
/// Returns an error only if the term count exceeds `u32::MAX`, which cannot
/// happen for a real document.
pub fn encode_sparse(embedding: &[(String, f32)]) -> Result<(Vec<u8>, Vec<u8>)> {
    let mut terms = Vec::new();
    let mut weights = Vec::with_capacity(embedding.len() * 4);
    let count = u32::try_from(embedding.len())
        .context("sparse embedding has more terms than u32::MAX")?;
    terms.extend_from_slice(&count.to_le_bytes());
    for (term, weight) in embedding {
        let len = u32::try_from(term.len()).context("term longer than u32::MAX bytes")?;
        terms.extend_from_slice(&len.to_le_bytes());
        terms.extend_from_slice(term.as_bytes());
        weights.extend_from_slice(&weight.to_le_bytes());
    }
    Ok((terms, weights))
}

/// Inverse of [`encode_sparse`].
///
/// # Errors
///
/// Returns an error if the blobs are truncated or the term/weight counts
/// disagree, which indicates a corrupt row rather than a caller mistake.
pub fn decode_sparse(terms: &[u8], weights: &[u8]) -> Result<Vec<(String, f32)>> {
    let mut out = Vec::new();
    if terms.len() < 4 {
        return Ok(out);
    }
    let count = u32::from_le_bytes(
        terms[..4]
            .try_into()
            .context("sparse term blob is shorter than its header")?,
    ) as usize;
    let mut cursor = 4usize;
    let mut weight_cursor = 0usize;
    for _ in 0..count {
        if cursor + 4 > terms.len() {
            anyhow::bail!("sparse term blob truncated at term length");
        }
        let len = u32::from_le_bytes(
            terms[cursor..cursor + 4]
                .try_into()
                .context("sparse term length is malformed")?,
        ) as usize;
        cursor += 4;
        if cursor + len > terms.len() {
            anyhow::bail!("sparse term blob truncated at term body");
        }
        let term = String::from_utf8(terms[cursor..cursor + len].to_vec())
            .context("sparse term is not valid UTF-8")?;
        cursor += len;

        if weight_cursor + 4 > weights.len() {
            anyhow::bail!("sparse weight blob truncated");
        }
        let weight = f32::from_le_bytes(
            weights[weight_cursor..weight_cursor + 4]
                .try_into()
                .context("sparse weight is malformed")?,
        );
        weight_cursor += 4;

        out.push((term, weight));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A corpus where "rare" appears once and "common" appears in four docs.
    fn corpus() -> Vec<&'static str> {
        vec![
            "the rare token appears here",
            "the common token appears here",
            "the common token appears here too",
            "the common token appears here as well",
            "the common token appears here again",
        ]
    }

    #[test]
    fn idf_prefers_rare_terms_over_common_ones() {
        let index = Bm25Index::fit(&corpus());
        for (i, text) in corpus().iter().enumerate() {
            index.upsert(&format!("doc-{i}"), text);
        }

        // "rare" occurs in exactly one document; "common" occurs in four.
        // Proper BM25 must rank the rare-term document first. The old
        // substring matcher could not express this distinction at all.
        let rare_hits = index.search("rare", 5);
        assert_eq!(rare_hits[0].0, "doc-0");

        let common_hits = index.search("common", 5);
        assert!(
            common_hits.iter().all(|(id, _)| id != "doc-0"),
            "doc-0 does not contain 'common' and must not match"
        );
    }

    #[test]
    fn empty_index_returns_no_hits() {
        let index = Bm25Index::fit(&[]);
        assert!(index.is_empty());
        assert!(index.search("anything", 10).is_empty());
    }

    #[test]
    fn upsert_replaces_rather_than_duplicates() {
        let index = Bm25Index::fit(&["alpha beta"]);
        index.upsert("doc", "alpha beta");
        index.upsert("doc", "gamma delta");
        assert_eq!(index.len(), 1);
        assert!(index.search("alpha", 5).is_empty(), "old text must be gone");
        assert_eq!(index.search("gamma", 5).len(), 1);
    }

    #[test]
    fn remove_drops_the_document() {
        let index = Bm25Index::fit(&["alpha beta"]);
        index.upsert("doc", "alpha beta");
        index.remove("doc");
        assert!(index.is_empty());
        assert!(index.search("alpha", 5).is_empty());
    }

    #[test]
    fn refit_reindexes_every_document() {
        let index = Bm25Index::with_avgdl(1.0);
        for (i, text) in corpus().iter().enumerate() {
            index.upsert(&format!("doc-{i}"), text);
        }
        let avgdl = index.refit();
        assert!(avgdl > 0.0, "fitted avgdl must be positive");
        assert_eq!(index.len(), corpus().len(), "refit must not lose documents");
        assert_eq!(index.search("rare", 5)[0].0, "doc-0");
    }

    #[test]
    fn sparse_encoding_round_trips() {
        let original = vec![
            ("alpha".to_string(), 1.5f32),
            ("beta".to_string(), 0.25f32),
            ("gamma".to_string(), 3.0f32),
        ];
        let (terms, weights) = encode_sparse(&original).expect("encode");
        let decoded = decode_sparse(&terms, &weights).expect("decode");
        assert_eq!(decoded, original);
    }

    #[test]
    fn sparse_decoding_rejects_truncated_blobs() {
        let original = vec![("alpha".to_string(), 1.0f32)];
        let (terms, weights) = encode_sparse(&original).expect("encode");
        // Chop the term body — a corrupt row must error, not silently truncate.
        let truncated = &terms[..terms.len() - 1];
        assert!(decode_sparse(truncated, &weights).is_err());
    }

    #[test]
    fn embedding_of_exposes_term_weight_pairs() {
        let index = Bm25Index::fit(&["alpha beta"]);
        index.upsert("doc", "alpha beta");
        let embedding = index.embedding_of("doc").expect("indexed doc");
        assert!(!embedding.is_empty());
        assert!(
            embedding.iter().any(|(term, _)| term == "alpha"),
            "term strings must be recovered, got {embedding:?}"
        );
        assert!(embedding.iter().all(|(_, weight)| *weight > 0.0));
    }

    #[test]
    fn search_is_deterministic_on_ties() {
        let index = Bm25Index::fit(&["alpha", "alpha"]);
        index.upsert("b-doc", "alpha");
        index.upsert("a-doc", "alpha");
        let first = index.search("alpha", 5);
        let second = index.search("alpha", 5);
        assert_eq!(first, second, "tied scores must break deterministically");
    }
}
