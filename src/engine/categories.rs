use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use tracing::warn;

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

/// Service for computing category statistics from the knowledge graph.
pub struct CategoryService {
    db: Arc<surrealdb::Surreal<surrealdb::engine::any::Any>>,
}

impl CategoryService {
    pub fn new(db: Arc<surrealdb::Surreal<surrealdb::engine::any::Any>>) -> Self {
        Self { db }
    }

    /// Fetch the top categories across all dimensions.
    pub async fn top_categories(&self, limit: usize) -> Result<Vec<Category>> {
        let mut categories = Vec::new();

        // Top domains
        match self.top_domains(limit).await {
            Ok(mut d) => categories.append(&mut d),
            Err(e) => warn!("top domains failed: {}", e),
        }

        // Top topic tags
        match self.top_topic_tags(limit).await {
            Ok(mut t) => categories.append(&mut t),
            Err(e) => warn!("top topic tags failed: {}", e),
        }

        // Top content types
        match self.top_content_types(limit).await {
            Ok(mut c) => categories.append(&mut c),
            Err(e) => warn!("top content types failed: {}", e),
        }

        // Sort by count desc, then by name
        categories.sort_by(|a, b| {
            b.count
                .cmp(&a.count)
                .then_with(|| a.name.cmp(&b.name))
        });
        categories.truncate(limit);
        Ok(categories)
    }

    async fn top_domains(&self, limit: usize) -> Result<Vec<Category>> {
        let sql = r#"
            SELECT domain, count() AS cnt FROM url_node
            WHERE crawled = true AND domain != ''
            GROUP BY domain
            ORDER BY cnt DESC
            LIMIT $limit
        "#;

        let mut response = self.db.query(sql).bind(("limit", limit as i64)).await?;
        let rows: Vec<JsonValue> = response.take(0)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let name = extract_string(&row, "domain");
            let count = extract_number(&row, "cnt");
            if !name.is_empty() && count > 0 {
                out.push(Category {
                    name,
                    category_type: CategoryType::Domain,
                    count,
                });
            }
        }
        Ok(out)
    }

    async fn top_topic_tags(&self, limit: usize) -> Result<Vec<Category>> {
        let sql = r#"
            SELECT topic_tags, count() AS cnt FROM url_node
            WHERE crawled = true AND topic_tags != NONE
            GROUP BY topic_tags
            LIMIT $limit
        "#;

        let mut response = self.db.query(sql).bind(("limit", limit as i64)).await?;
        let rows: Vec<JsonValue> = response.take(0)?;

        // Flatten topic_tags arrays and count occurrences
        let mut counts: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
        for row in rows {
            let count = extract_number(&row, "cnt");
            if let JsonValue::Object(obj) = &row {
                if let Some(JsonValue::Array(tags)) = obj.get("topic_tags") {
                    for tag in tags {
                        if let JsonValue::String(t) = tag {
                            *counts.entry(t.clone()).or_insert(0) += count;
                        }
                    }
                }
            }
            if let JsonValue::Array(arr) = &row {
                if let Some(JsonValue::Object(obj)) = arr.first() {
                    if let Some(JsonValue::Array(tags)) = obj.get("topic_tags") {
                        for tag in tags {
                            if let JsonValue::String(t) = tag {
                                *counts.entry(t.clone()).or_insert(0) += count;
                            }
                        }
                    }
                }
            }
        }

        let mut out: Vec<Category> = counts
            .into_iter()
            .filter(|(_, c)| *c > 0)
            .map(|(name, count)| Category {
                name,
                category_type: CategoryType::TopicTag,
                count,
            })
            .collect();
        out.sort_by(|a, b| b.count.cmp(&a.count));
        out.truncate(limit);
        Ok(out)
    }

    async fn top_content_types(&self, limit: usize) -> Result<Vec<Category>> {
        let sql = r#"
            SELECT content_type, count() AS cnt FROM url_node
            WHERE crawled = true AND content_type != NONE
            GROUP BY content_type
            ORDER BY cnt DESC
            LIMIT $limit
        "#;

        let mut response = self.db.query(sql).bind(("limit", limit as i64)).await?;
        let rows: Vec<JsonValue> = response.take(0)?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let name = extract_string(&row, "content_type");
            let count = extract_number(&row, "cnt");
            if !name.is_empty() && count > 0 {
                out.push(Category {
                    name,
                    category_type: CategoryType::ContentType,
                    count,
                });
            }
        }
        Ok(out)
    }
}

fn extract_string(value: &JsonValue, key: &str) -> String {
    if let JsonValue::Object(obj) = value {
        if let Some(v) = obj.get(key) {
            if let JsonValue::String(s) = v {
                return s.clone();
            }
        }
    }
    if let JsonValue::Array(arr) = value {
        if let Some(first) = arr.first() {
            return extract_string(first, key);
        }
    }
    String::new()
}

fn extract_number(value: &JsonValue, key: &str) -> u64 {
    if let JsonValue::Object(obj) = value {
        if let Some(v) = obj.get(key) {
            return match v {
                JsonValue::Number(n) => n.as_u64().unwrap_or(0),
                _ => 0,
            };
        }
    }
    if let JsonValue::Array(arr) = value {
        if let Some(first) = arr.first() {
            return extract_number(first, key);
        }
    }
    0
}
