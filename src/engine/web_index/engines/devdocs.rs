//! DevDocs engine adapter: fast in-memory routing across official documentation sets.
//!
//! Provides instant direct routing to official API references (React, Vue, Rust, Go,
//! Python, TypeScript, Docker, Git, PostgreSQL, etc.) without network overhead.

use async_trait::async_trait;

use crate::engine::web_index::client::Client;
use crate::engine::web_index::util::positional_relevance;
use crate::engine::web_index::{Engine, EngineOptions, Error, Hit};

struct DocSlug {
    slug: &'static str,
    aliases: &'static [&'static str],
    title: &'static str,
    doc_type: &'static str,
}

const DOCS: &[DocSlug] = &[
    DocSlug {
        slug: "rust",
        aliases: &["rust", "cargo", "rustlang"],
        title: "Rust Standard Library",
        doc_type: "Language Documentation",
    },
    DocSlug {
        slug: "typescript",
        aliases: &["typescript", "ts"],
        title: "TypeScript",
        doc_type: "Language",
    },
    DocSlug {
        slug: "javascript",
        aliases: &["javascript", "js", "ecmascript"],
        title: "JavaScript",
        doc_type: "Language",
    },
    DocSlug {
        slug: "python~3.12",
        aliases: &["python", "py", "python3"],
        title: "Python 3.12",
        doc_type: "Language",
    },
    DocSlug {
        slug: "go",
        aliases: &["go", "golang"],
        title: "Go Standard Library",
        doc_type: "Language",
    },
    DocSlug {
        slug: "react",
        aliases: &["react", "reactjs"],
        title: "React",
        doc_type: "JavaScript library",
    },
    DocSlug {
        slug: "vue",
        aliases: &["vue", "vuejs"],
        title: "Vue.js",
        doc_type: "JavaScript framework",
    },
    DocSlug {
        slug: "node",
        aliases: &["node", "nodejs"],
        title: "Node.js",
        doc_type: "Runtime",
    },
    DocSlug {
        slug: "postgresql~16",
        aliases: &["postgres", "postgresql", "pg", "psql"],
        title: "PostgreSQL 16",
        doc_type: "Database",
    },
    DocSlug {
        slug: "sqlite",
        aliases: &["sqlite", "sqlite3"],
        title: "SQLite",
        doc_type: "Database",
    },
    DocSlug {
        slug: "docker",
        aliases: &["docker", "dockerfile", "container"],
        title: "Docker",
        doc_type: "DevOps Tool",
    },
    DocSlug {
        slug: "git",
        aliases: &["git", "github"],
        title: "Git",
        doc_type: "Version Control",
    },
    DocSlug {
        slug: "bash",
        aliases: &["bash", "shell", "sh", "zsh"],
        title: "Bash / Shell",
        doc_type: "Shell Scripting",
    },
    DocSlug {
        slug: "nginx",
        aliases: &["nginx"],
        title: "nginx",
        doc_type: "Web Server",
    },
    DocSlug {
        slug: "tailwindcss",
        aliases: &["tailwind", "tailwindcss"],
        title: "Tailwind CSS",
        doc_type: "CSS Framework",
    },
];

/// DevDocs adapter.
pub struct DevDocsEngine {
    _client: Client,
}

impl DevDocsEngine {
    /// Build the adapter on a shared client.
    #[must_use]
    pub fn new(client: Client) -> Self {
        Self { _client: client }
    }
}

#[async_trait]
impl Engine for DevDocsEngine {
    fn name(&self) -> &'static str {
        "devdocs"
    }

    fn should_query(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        let tokens: Vec<&str> = q
            .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .filter(|t| !t.is_empty())
            .collect();
        DOCS.iter().any(|doc| {
            doc.aliases.iter().any(|alias| tokens.contains(alias))
        })
    }

    async fn search(&self, query: &str, opts: &EngineOptions) -> Result<Vec<Hit>, Error> {
        let tokens: Vec<String> = query
            .to_lowercase()
            .split(|c: char| c.is_whitespace() || c == '-' || c == '_')
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
            .collect();

        if tokens.is_empty() {
            return Ok(Vec::new());
        }

        let mut matched = Vec::new();
        for doc in DOCS {
            if doc.aliases.iter().any(|alias| tokens.contains(&alias.to_string())) {
                matched.push(doc);
            }
        }

        let total = matched.len().min(opts.max_results);
        let mut hits = Vec::with_capacity(total);

        for (i, doc) in matched.into_iter().take(total).enumerate() {
            let base_slug = doc.slug.split('~').next().unwrap_or(doc.slug);
            hits.push(Hit {
                url: format!("https://devdocs.io/{base_slug}"),
                title: format!("DevDocs: {}", doc.title),
                snippet: format!("{} — {}", doc.title, doc.doc_type),
                published_at: None,
                relevance_score: positional_relevance(i, total),
                engine: "devdocs",
            });
        }

        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn matches_devdocs_slugs() {
        let client = Client::new().expect("client creation");
        let engine = DevDocsEngine::new(client);
        let opts = EngineOptions::default();

        let hits = engine.search("how to write rust code", &opts).await.unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].url, "https://devdocs.io/rust");
        assert_eq!(hits[0].engine, "devdocs");
    }
}
