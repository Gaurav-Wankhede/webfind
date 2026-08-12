use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tantivy::collector::TopDocs;
use tantivy::directory::MmapDirectory;
use tantivy::query::QueryParser;
use tantivy::schema::*;
use tantivy::{
    DateTime as TantivyDateTime, DocAddress, Index, IndexReader, IndexWriter, ReloadPolicy,
    TantivyDocument,
};

use crate::schema::content::StructuredContent;
use crate::schema::response::{ScoreBreakdown, SearchResult};

/// Tantivy field handles for fast document construction.
#[derive(Debug, Clone)]
pub struct WebfindFields {
    pub url: Field,
    pub title: Field,
    pub body: Field,
    pub domain: Field,
    pub language: Field,
    pub excerpt: Field,
    pub crawled_at: Field,
    pub published_at: Field,
    pub modified_at: Field,
    pub author: Field,
    pub site_name: Field,
    pub word_count: Field,
    pub reading_ease: Field,
    pub grade_level: Field,
    pub schema_type: Field,
    pub is_paywalled: Field,
    pub ssl_valid: Field,
}

impl WebfindFields {
    /// Extract field handles from a built schema.
    pub fn from_schema(schema: &Schema) -> Result<Self> {
        Ok(Self {
            url: schema
                .get_field("url")
                .context("missing 'url' field in schema")?,
            title: schema
                .get_field("title")
                .context("missing 'title' field in schema")?,
            body: schema
                .get_field("body")
                .context("missing 'body' field in schema")?,
            domain: schema
                .get_field("domain")
                .context("missing 'domain' field in schema")?,
            language: schema
                .get_field("language")
                .context("missing 'language' field in schema")?,
            excerpt: schema
                .get_field("excerpt")
                .context("missing 'excerpt' field in schema")?,
            crawled_at: schema
                .get_field("crawled_at")
                .context("missing 'crawled_at' field in schema")?,
            published_at: schema
                .get_field("published_at")
                .context("missing 'published_at' field in schema")?,
            modified_at: schema
                .get_field("modified_at")
                .context("missing 'modified_at' field in schema")?,
            author: schema
                .get_field("author")
                .context("missing 'author' field in schema")?,
            site_name: schema
                .get_field("site_name")
                .context("missing 'site_name' field in schema")?,
            word_count: schema
                .get_field("word_count")
                .context("missing 'word_count' field in schema")?,
            reading_ease: schema
                .get_field("reading_ease")
                .context("missing 'reading_ease' field in schema")?,
            grade_level: schema
                .get_field("grade_level")
                .context("missing 'grade_level' field in schema")?,
            schema_type: schema
                .get_field("schema_type")
                .context("missing 'schema_type' field in schema")?,
            is_paywalled: schema
                .get_field("is_paywalled")
                .context("missing 'is_paywalled' field in schema")?,
            ssl_valid: schema
                .get_field("ssl_valid")
                .context("missing 'ssl_valid' field in schema")?,
        })
    }
}

/// Build the webfind Tantivy schema.
pub fn build_schema() -> (Schema, WebfindFields) {
    let mut builder = Schema::builder();

    // Text fields — tokenized + indexed for BM25
    let url = builder.add_text_field("url", TEXT | STORED);
    let title = builder.add_text_field("title", TEXT | STORED);
    let body = builder.add_text_field("body", TEXT | STORED);
    let domain = builder.add_text_field("domain", STRING | STORED);
    let language = builder.add_text_field("language", STRING | STORED);
    let excerpt = builder.add_text_field("excerpt", TEXT | STORED);

    // Numeric fields — stored for retrieval, fast for sorting
    let crawled_at = builder.add_date_field("crawled_at", STORED | INDEXED);
    let published_at = builder.add_date_field("published_at", STORED);
    let modified_at = builder.add_date_field("modified_at", STORED);
    let author = builder.add_text_field("author", STRING | STORED);
    let site_name = builder.add_text_field("site_name", STRING | STORED);
    let word_count = builder.add_u64_field("word_count", STORED);
    let reading_ease = builder.add_f64_field("reading_ease", STORED);
    let grade_level = builder.add_f64_field("grade_level", STORED);
    let schema_type = builder.add_text_field("schema_type", STRING | STORED);
    let is_paywalled = builder.add_bool_field("is_paywalled", STORED);
    let ssl_valid = builder.add_bool_field("ssl_valid", STORED);

    let schema = builder.build();
    let fields = WebfindFields {
        url,
        title,
        body,
        domain,
        language,
        excerpt,
        crawled_at,
        published_at,
        modified_at,
        author,
        site_name,
        word_count,
        reading_ease,
        grade_level,
        schema_type,
        is_paywalled,
        ssl_valid,
    };

    (schema, fields)
}

/// Low-level Tantivy index manager.
pub struct TantivyStore {
    path: PathBuf,
    index: Index,
    fields: WebfindFields,
    writer: Option<IndexWriter>,
}

impl TantivyStore {
    /// Open or create an index at the given directory.
    /// If an existing index has an incompatible schema, it is backed up and
    /// a fresh index is created.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create index dir: {}", path.display()))?;

        let (schema, _) = build_schema();

        let dir = MmapDirectory::open(&path).context("failed to open mmap directory")?;
        let index = if Index::exists(&dir).unwrap_or(false) {
            let existing = Index::open(dir).context("failed to open existing index")?;
            if existing.schema() != schema {
                tracing::warn!(
                    "index schema mismatch at {}; backing up and rebuilding",
                    path.display()
                );
                drop(existing);
                let backup = path.with_extension("old");
                if backup.exists() {
                    std::fs::remove_dir_all(&backup).with_context(|| {
                        format!("failed to remove old index backup: {}", backup.display())
                    })?;
                }
                std::fs::rename(&path, &backup).with_context(|| {
                    format!("failed to back up old index: {}", path.display())
                })?;
                std::fs::create_dir_all(&path)
                    .with_context(|| format!("failed to recreate index dir: {}", path.display()))?;
                Index::create_in_dir(&path, schema).context("failed to create fresh index")?
            } else {
                existing
            }
        } else {
            Index::create_in_dir(&path, schema).context("failed to create index")?
        };

        let fields = WebfindFields::from_schema(&index.schema())?;

        Ok(Self {
            path,
            index,
            fields,
            writer: None,
        })
    }

    /// Get or create an IndexWriter (50 MB memory budget).
    pub fn writer(&mut self) -> Result<&mut IndexWriter> {
        if self.writer.is_none() {
            let w = self
                .index
                .writer(50_000_000)
                .context("failed to create index writer")?;
            self.writer = Some(w);
        }
        Ok(self.writer.as_mut().unwrap())
    }

    /// Commit pending documents to disk.
    pub fn commit(&mut self) -> Result<()> {
        if let Some(writer) = &mut self.writer {
            writer.commit().context("failed to commit index")?;
        }
        Ok(())
    }

    /// Build a reader with auto-reload on commit.
    pub fn reader(&self) -> Result<IndexReader> {
        self.index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()
            .context("failed to build index reader")
    }

    /// Create a QueryParser targeting title + body fields.
    fn query_parser(&self) -> QueryParser {
        QueryParser::for_index(&self.index, vec![self.fields.title, self.fields.body])
    }

    /// Index a single StructuredContent document.
    pub fn index_content(&mut self, content: &StructuredContent) -> Result<()> {
        let fields = self.fields.clone();
        let writer = self.writer()?;

        let crawled = TantivyDateTime::from_timestamp_secs(content.fetched_at.timestamp());
        let published = content
            .published_at
            .map(|dt| TantivyDateTime::from_timestamp_secs(dt.timestamp()))
            .unwrap_or_else(|| TantivyDateTime::from_timestamp_secs(0));
        let modified = content
            .modified_at
            .map(|dt| TantivyDateTime::from_timestamp_secs(dt.timestamp()))
            .unwrap_or_else(|| TantivyDateTime::from_timestamp_secs(0));

        let mut doc = TantivyDocument::default();
        doc.add_text(fields.url, &content.url);
        doc.add_text(fields.title, &content.title);
        doc.add_text(fields.body, &content.content_text);
        doc.add_text(fields.domain, &content_final_domain(&content.url));
        doc.add_text(fields.language, &content.language);
        doc.add_text(fields.excerpt, &content.excerpt);
        doc.add_date(fields.crawled_at, crawled);
        doc.add_date(fields.published_at, published);
        doc.add_date(fields.modified_at, modified);
        if let Some(ref author) = content.author {
            doc.add_text(fields.author, author);
        }
        if let Some(ref site) = content.site_name {
            doc.add_text(fields.site_name, site);
        }
        doc.add_u64(fields.word_count, content.word_count as u64);
        doc.add_f64(fields.reading_ease, content.reading_ease);
        doc.add_f64(fields.grade_level, content.grade_level);
        if let Some(ref st) = content.schema_type {
            doc.add_text(fields.schema_type, st);
        }
        doc.add_bool(fields.is_paywalled, content.is_paywalled);
        doc.add_bool(fields.ssl_valid, content.ssl_valid);

        writer.add_document(doc)?;
        Ok(())
    }

    /// Index a batch of StructuredContent documents.
    pub fn index_batch(&mut self, items: &[&StructuredContent]) -> Result<u64> {
        let mut count = 0u64;
        for item in items {
            self.index_content(item)?;
            count += 1;
        }
        self.commit()?;
        Ok(count)
    }

    /// Search the index with a BM25 query, returning top-N results.
    pub fn search(&self, query_str: &str, limit: usize) -> Result<Vec<Bm25Hit>> {
        let reader = self.reader()?;
        let searcher = reader.searcher();
        let parser = self.query_parser();

        let query = parser
            .parse_query(query_str)
            .map_err(|e| anyhow::anyhow!("query parse error: {}", e))?;

        let top_docs: Vec<(f32, DocAddress)> = searcher
            .search(&query, &TopDocs::with_limit(limit).order_by_score())
            .context("search execution failed")?;

        let mut hits = Vec::with_capacity(top_docs.len());
        for (score, doc_addr) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_addr)?;
            let fields = &self.fields;

            let url = doc
                .get_first(fields.url)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let title = doc
                .get_first(fields.title)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let body = doc
                .get_first(fields.body)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let excerpt = doc
                .get_first(fields.excerpt)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let domain = doc
                .get_first(fields.domain)
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let language = doc
                .get_first(fields.language)
                .and_then(|v| v.as_str())
                .unwrap_or("en")
                .to_string();
            let word_count = doc
                .get_first(fields.word_count)
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;
            let reading_ease = doc
                .get_first(fields.reading_ease)
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            let grade_level = doc
                .get_first(fields.grade_level)
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);

            let crawled_at = doc
                .get_first(fields.crawled_at)
                .and_then(|v| v.as_datetime())
                .map(|dt| {
                    let ts = dt.into_timestamp_secs();
                    DateTime::from_timestamp(ts, 0).unwrap_or_else(Utc::now)
                })
                .unwrap_or_else(Utc::now);

            let published_at = doc
                .get_first(fields.published_at)
                .and_then(|v| v.as_datetime())
                .and_then(|dt| {
                    let ts = dt.into_timestamp_secs();
                    if ts == 0 {
                        None
                    } else {
                        DateTime::from_timestamp(ts, 0)
                    }
                });

            let modified_at = doc
                .get_first(fields.modified_at)
                .and_then(|v| v.as_datetime())
                .and_then(|dt| {
                    let ts = dt.into_timestamp_secs();
                    if ts == 0 {
                        None
                    } else {
                        DateTime::from_timestamp(ts, 0)
                    }
                });

            let author = doc
                .get_first(fields.author)
                .and_then(|v| v.as_str())
                .map(String::from);

            let site_name = doc
                .get_first(fields.site_name)
                .and_then(|v| v.as_str())
                .map(String::from);

            hits.push(Bm25Hit {
                url,
                title,
                body,
                excerpt,
                domain,
                language,
                crawled_at,
                published_at,
                modified_at,
                author,
                site_name,
                word_count,
                reading_ease,
                grade_level,
                bm25_score: score as f64,
            });
        }

        Ok(hits)
    }

    /// Number of indexed documents.
    pub fn doc_count(&self) -> Result<u64> {
        let reader = self.reader()?;
        let searcher = reader.searcher();
        Ok(searcher.num_docs())
    }

    /// Path to the index directory.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Drop the writer (flush + release resources).
    pub fn drop_writer(&mut self) -> Result<()> {
        if self.writer.is_some() {
            self.commit()?;
            self.writer = None;
        }
        Ok(())
    }
}

/// A single BM25 search hit before final ranking.
#[derive(Debug, Clone)]
pub struct Bm25Hit {
    pub url: String,
    pub title: String,
    pub body: String,
    pub excerpt: String,
    pub domain: String,
    pub language: String,
    pub crawled_at: DateTime<Utc>,
    pub published_at: Option<DateTime<Utc>>,
    pub modified_at: Option<DateTime<Utc>>,
    pub author: Option<String>,
    pub site_name: Option<String>,
    pub word_count: u32,
    pub reading_ease: f64,
    pub grade_level: f64,
    pub bm25_score: f64,
}

impl Bm25Hit {
    /// Convert to a final SearchResult with score breakdown.
    pub fn to_search_result(self, rank: u32, max_score: f64) -> SearchResult {
        let normalized = if max_score > 0.0 {
            self.bm25_score / max_score
        } else {
            0.0
        };

        // Use body for snippet if available, fall back to excerpt
        let snippet = if self.body.len() > 20 {
            let end = self
                .body
                .char_indices()
                .nth(200)
                .map(|(i, _)| i)
                .unwrap_or(self.body.len());
            format!("{}…", &self.body[..end])
        } else {
            self.excerpt
        };

        SearchResult {
            rank,
            url: self.url,
            title: self.title,
            snippet,
            domain: self.domain,
            published_at: self.published_at,
            modified_at: self.modified_at,
            crawled_at: self.crawled_at,
            author: self.author,
            site_name: self.site_name,
            score: normalized,
            scores: ScoreBreakdown {
                bm25: normalized,
                vector: None,
                graph: None,
                freshness: None,
                quality: None,
                final_score: normalized,
            },
            content: None,
            keywords: None,
            metrics: None,
            favicon: None,
            thumbnail: None,
            language: self.language,
            content_type: crate::schema::request::ContentType::Any,
        }
    }
}

/// Extract domain from a URL string.
pub fn content_final_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn sample_content(url: &str, title: &str, body: &str) -> StructuredContent {
        StructuredContent {
            url: url.to_string(),
            final_url: url.to_string(),
            status_code: 200,
            title: title.to_string(),
            description: None,
            canonical_url: None,
            language: "en".to_string(),
            language_confidence: 0.95,
            published_at: Some(Utc.with_ymd_and_hms(2025, 6, 15, 10, 0, 0).unwrap()),
            modified_at: None,
            author: None,
            site_name: None,
            content_text: body.to_string(),
            content_html: format!("<p>{}</p>", body),
            content_markdown: body.to_string(),
            excerpt: body.chars().take(200).collect(),
            word_count: body.split_whitespace().count() as u32,
            char_count: body.len() as u32,
            sentence_count: 1,
            reading_time_seconds: 30,
            reading_ease: 65.0,
            grade_level: 8.0,
            keywords: vec![],
            open_graph: None,
            twitter_card: None,
            json_ld: vec![],
            schema_type: None,
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: body.to_string(),
            fetched_at: Utc::now(),
            fetch_duration_ms: 150,
            html_size_bytes: 1024,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            is_paywalled: false,
            is_valid_content: true,
            entities: crate::schema::content::Entities::default(),
        }
    }

    #[test]
    fn test_index_and_search() {
        let tmp = TempDir::new().unwrap();
        let mut store = TantivyStore::open(tmp.path()).unwrap();

        let c1 = sample_content(
            "https://example.com/rust-guide",
            "Rust Programming Guide",
            "Rust is a systems programming language focused on safety and performance.",
        );
        let c2 = sample_content(
            "https://example.com/python-guide",
            "Python Programming Guide",
            "Python is a high-level language known for simplicity and readability.",
        );
        let c3 = sample_content(
            "https://docs.example.com/rust-tutorial",
            "Learn Rust Basics",
            "This tutorial covers Rust ownership, borrowing, and lifetimes.",
        );

        store.index_content(&c1).unwrap();
        store.index_content(&c2).unwrap();
        store.index_content(&c3).unwrap();
        store.commit().unwrap();

        let hits = store.search("rust programming", 10).unwrap();
        assert!(!hits.is_empty(), "should find rust results");
        assert!(
            hits.iter().any(|h| h.title.contains("Rust")),
            "at least one rust result"
        );

        let hits = store.search("python", 10).unwrap();
        assert!(!hits.is_empty(), "should find python results");

        let count = store.doc_count().unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_domain_extraction() {
        assert_eq!(
            content_final_domain("https://www.example.com/path"),
            "example.com"
        );
        assert_eq!(
            content_final_domain("http://blog.example.co.uk/post"),
            "blog.example.co.uk"
        );
        assert_eq!(
            content_final_domain("https://rust-lang.org"),
            "rust-lang.org"
        );
    }
}
