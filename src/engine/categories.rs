use anyhow::{Context, Result};
use libsql::{Connection, params};
use serde::{Deserialize, Serialize};

/// A category (domain, topic tag, or content type) with its frequency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub name: String,
    pub category_type: CategoryType,
    pub count: u64,
}

/// Type of category.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CategoryType {
    Domain,
    TopicTag,
    ContentType,
    Language,
}

impl CategoryType {
    /// Returns the lowercase string representation for template comparisons.
    pub fn as_str(&self) -> &'static str {
        match self {
            CategoryType::Domain => "domain",
            CategoryType::TopicTag => "topictag",
            CategoryType::ContentType => "contenttype",
            CategoryType::Language => "language",
        }
    }
}

/// Service for computing category statistics from the Turso knowledge graph.
///
/// Replaces the SurrealDB-backed implementation. The Turso `url_nodes` schema
/// carries `domain`, so the domain dimension is fully supported; topic tags and
/// content types are not currently persisted in the embedded schema and yield
/// no categories (documented limitation, not a regression from Turso mode).
pub struct CategoryService {
    conn: Connection,
}

impl CategoryService {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    /// Fetch the top categories across all supported dimensions.
    pub async fn top_categories(&self, limit: usize) -> Result<Vec<Category>> {
        let mut categories = Vec::new();

        // Top domains (the only dimension persisted in the embedded schema).
        match self.top_domains(limit).await {
            Ok(mut d) => categories.append(&mut d),
            Err(e) => tracing::warn!("top domains failed: {}", e),
        }

        // Sort by count desc, then by name
        categories.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
        categories.truncate(limit);
        Ok(categories)
    }

    async fn top_domains(&self, limit: usize) -> Result<Vec<Category>> {
        let mut rows = self
            .conn
            .query(
                r#"
                SELECT domain, COUNT(*) AS cnt
                FROM url_nodes
                WHERE crawled = 1 AND domain != ''
                GROUP BY domain
                ORDER BY cnt DESC
                LIMIT ?1
                "#,
                params![limit.clamp(1, 500) as i64],
            )
            .await
            .context("query top domains")?;

        let mut out = Vec::new();
        loop {
            let row = match rows.next().await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(e) => return Err(e).context("iterate top domains"),
            };
            let name: String = row.get(0).unwrap_or_default();
            let count: i64 = row.get(1).unwrap_or(0);
            if !name.is_empty() && count > 0 {
                out.push(Category {
                    name,
                    category_type: CategoryType::Domain,
                    count: count as u64,
                });
            }
        }
        Ok(out)
    }
}
