use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use robotstxt::DefaultMatcher;
use sitemap::reader::{SiteMapEntity, SiteMapReader};
use tokio::join;
use tracing::{debug, warn};

/// Parsed robots.txt rules and sitemap references.
#[derive(Debug, Clone, Default)]
pub struct RobotsPolicy {
    pub raw: String,
    /// Sitemap URLs referenced inside robots.txt.
    pub sitemap_urls: Vec<String>,
    /// Crawl delay in milliseconds (minimum value across matching directives).
    pub crawl_delay_ms: Option<u64>,
}

impl RobotsPolicy {
    /// Check whether a URL may be crawled under this robots.txt policy.
    pub fn is_allowed(&self, url: &str) -> bool {
        if self.raw.is_empty() {
            return true;
        }
        let mut matcher = DefaultMatcher::default();
        matcher.one_agent_allowed_by_robots(&self.raw, "webfind", url)
    }

    /// Parse a robots.txt body. Exposed for unit testing and for callers that
    /// already fetched the file.
    pub fn parse(text: &str) -> Self {
        let mut sitemap_urls = Vec::new();
        let mut crawl_delay_ms: Option<u64> = None;
        let mut current_user_agent: Option<String> = None;

        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once(':') {
                let key = key.trim().to_lowercase();
                let value = value.trim();
                match key.as_str() {
                    "user-agent" => {
                        current_user_agent = Some(value.to_lowercase());
                    }
                    "sitemap" => {
                        sitemap_urls.push(value.to_string());
                    }
                    "crawl-delay" => {
                        // Apply to wildcard or webfind-specific groups.
                        let applies = current_user_agent
                            .as_ref()
                            .map(|ua| ua == "*" || ua.contains("webfind"))
                            .unwrap_or(true);
                        if applies && let Ok(secs) = value.parse::<f64>() {
                            let ms = (secs * 1000.0) as u64;
                            crawl_delay_ms = Some(crawl_delay_ms.map(|d| d.min(ms)).unwrap_or(ms));
                        }
                    }
                    _ => {}
                }
            }
        }

        Self {
            raw: text.to_string(),
            sitemap_urls,
            crawl_delay_ms,
        }
    }
}

/// A single URL discovered from a sitemap with its structural metadata.
#[derive(Debug, Clone)]
pub struct SitemapUrl {
    pub url: String,
    pub lastmod: Option<DateTime<Utc>>,
    pub changefreq: Option<String>,
    pub priority: f32,
}

/// One curated link from an `llms.txt` / `llm.txt` file.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmsEntry {
    pub name: String,
    pub url: String,
    pub description: Option<String>,
}

/// A titled group of curated links inside an LLM index file.
#[derive(Debug, Clone, PartialEq)]
pub struct LlmsSection {
    pub heading: String,
    pub entries: Vec<LlmsEntry>,
}

/// Parsed `/llms.txt` (Jeremy Howard proposal, llmstxt.org) or the `llm.txt`
/// compatibility variant. The file is the site owner's curated briefing: which
/// pages matter, in what order. It does NOT grant access — robots.txt remains
/// the access control layer — but it tells a research crawler the site's
/// declared content surface, which we use to auto-map crawl depth and to keep
/// link-following inside the site's own map.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LlmsIndex {
    pub site_name: Option<String>,
    pub summary: Option<String>,
    pub sections: Vec<LlmsSection>,
}

impl LlmsIndex {
    /// Parse an llms.txt body per the llmstxt.org proposal:
    /// optional BOM, H1 site name, blockquote summary, optional context
    /// paragraphs, then zero or more H2 sections whose entries are markdown
    /// links (`- [Name](url)` optionally followed by `: description`).
    pub fn parse(text: &str) -> Self {
        let mut index = LlmsIndex::default();
        let mut current_section: Option<LlmsSection> = None;
        let mut context_summary = String::new();

        for raw_line in text.lines() {
            let line = raw_line.trim_start_matches('\u{feff}').trim();
            if line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix("# ") {
                if index.site_name.is_none() {
                    index.site_name = Some(name.trim().to_string());
                }
            } else if let Some(summary) = line.strip_prefix("> ") {
                if !context_summary.is_empty() {
                    context_summary.push(' ');
                }
                context_summary.push_str(summary.trim());
            } else if let Some(heading) = line.strip_prefix("## ") {
                if let Some(section) = current_section.take() {
                    index.sections.push(section);
                }
                current_section = Some(LlmsSection {
                    heading: heading.trim().to_string(),
                    entries: Vec::new(),
                });
            } else if let Some(rest) = line.strip_prefix("- ")
                && let Some(entry) = Self::parse_entry(rest) {
                    let section = current_section.get_or_insert_with(|| LlmsSection {
                        heading: String::new(),
                        entries: Vec::new(),
                    });
                    section.entries.push(entry);
                }
            // Non-heading context paragraphs are deliberately ignored: the
            // proposal allows free-form prose that has no crawlable structure.
        }

        if let Some(section) = current_section.take() {
            index.sections.push(section);
        }
        if !context_summary.is_empty() {
            index.summary = Some(context_summary);
        }
        index
    }

    /// Parse a single `- [Name](url): description` list item.
    fn parse_entry(rest: &str) -> Option<LlmsEntry> {
        let name_end = rest.find("](")?;
        let name = rest[..name_end].strip_prefix('[')?.trim().to_string();
        let after_name = &rest[name_end + 2..];
        let url_end = after_name.find(')')?;
        let url = after_name[..url_end].trim().to_string();
        if name.is_empty() || url.is_empty() {
            return None;
        }
        let tail = after_name[url_end + 1..].trim();
        let description = tail.strip_prefix(':').map(|d| d.trim().to_string());
        Some(LlmsEntry {
            name,
            url,
            description: description.filter(|d| !d.is_empty()),
        })
    }

    /// All entry URLs across every section, in document order.
    pub fn urls(&self) -> impl Iterator<Item = &str> {
        self.sections
            .iter()
            .flat_map(|s| s.entries.iter())
            .map(|e| e.url.as_str())
    }
}

/// Complete structural blueprint of a site: robots rules + all sitemap URLs.
/// Designed to feed both the crawler queue and a SurrealDB graph vector store.
#[derive(Debug, Clone, Default)]
pub struct SiteBlueprint {
    pub domain: String,
    pub robots: RobotsPolicy,
    /// URLs discovered from sitemap.xml and robots.txt-referenced sitemaps,
    /// deduplicated and sorted by descending priority.
    pub sitemap_urls: Vec<SitemapUrl>,
    /// Curated LLM index from `/llms.txt` (or the `llm.txt` variant), if the
    /// site publishes one. The site's own statement of which pages matter.
    pub llms: Option<LlmsIndex>,
    /// Topic keywords extracted from sampling top sitemap pages.
    pub topic_keywords: Vec<String>,
}

/// Fetches and parses robots.txt and sitemap.xml truly in parallel.
/// Handles sitemap indexes recursively with bounded depth.
pub struct SiteExplorer {
    client: reqwest::Client,
    max_sitemap_depth: usize,
}

impl SiteExplorer {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .gzip(true)
            .build()
            .context("failed to build site explorer client")?;
        Ok(Self {
            client,
            max_sitemap_depth: 2,
        })
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self {
            client,
            max_sitemap_depth: 2,
        }
    }

    /// Explore the site structure starting from a seed URL.
    /// Fetches `/robots.txt` and `/sitemap.xml` in parallel, then resolves
    /// any additional sitemaps referenced by robots.txt or nested sitemap indexes.
    pub async fn explore(&self, seed: &str) -> Result<SiteBlueprint> {
        let parsed = reqwest::Url::parse(seed).context("invalid seed URL")?;
        let host = parsed.host_str().unwrap_or("").to_lowercase();
        let scheme = parsed.scheme();
        let host_with_port = match parsed.port() {
            Some(port) => format!("{}:{}", host, port),
            None => host.clone(),
        };
        let base = format!("{}://{}", scheme, host_with_port);
        let robots_url = format!("{}/robots.txt", base);
        let sitemap_url = format!("{}/sitemap.xml", base);
        let llms_url = format!("{}/llms.txt", base);
        let llm_url = format!("{}/llm.txt", base);

        // TRUE PARALLEL fetch of robots.txt, the root sitemap, and the
        // site's curated LLM index (llms.txt preferred, llm.txt fallback).
        let (robots_res, sitemap_res, llms_res) = join!(
            Self::fetch_robots(&self.client, &robots_url),
            self.fetch_sitemap(sitemap_url, 0),
            Self::fetch_llms_index(&self.client, &llms_url, &llm_url),
        );

        let robots = robots_res.unwrap_or_else(|e| {
            warn!("robots.txt fetch failed for {}: {}", host, e);
            RobotsPolicy::default()
        });

        let llms = llms_res.unwrap_or_else(|e| {
            warn!("llms.txt/llm.txt fetch failed for {}: {}", host, e);
            None
        });

        let mut sitemap_urls = sitemap_res.unwrap_or_default();

        // Fetch robots.txt-referenced sitemaps in parallel.
        if !robots.sitemap_urls.is_empty() {
            let mut fetches = Vec::new();
            for url in &robots.sitemap_urls {
                fetches.push(self.fetch_sitemap(url, 0));
            }
            let nested = futures::future::join_all(fetches).await;
            for urls in nested.into_iter().flatten() {
                sitemap_urls.extend(urls);
            }
        }

        // Deduplicate by URL, keep highest priority and latest lastmod.
        let mut by_url: HashMap<String, SitemapUrl> = HashMap::new();
        for entry in sitemap_urls {
            by_url
                .entry(entry.url.clone())
                .and_modify(|existing| {
                    existing.priority = existing.priority.max(entry.priority);
                    if let Some(ref new_lm) = entry.lastmod {
                        match existing.lastmod {
                            None => existing.lastmod = Some(*new_lm),
                            Some(ref old_lm) if new_lm > old_lm => existing.lastmod = Some(*new_lm),
                            _ => {}
                        }
                    }
                })
                .or_insert(entry);
        }

        let mut sitemap_urls: Vec<SitemapUrl> = by_url.into_values().collect();
        sitemap_urls.sort_by(|a, b| {
            b.priority
                .partial_cmp(&a.priority)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(SiteBlueprint {
            domain: host,
            robots,
            sitemap_urls,
            llms,
            topic_keywords: Vec::new(),
        })
    }

    /// Sample top sitemap pages by priority and extract topic keywords.
    /// This enriches the blueprint with content-derived topics for better crawl prioritization.
    pub async fn sample_topics(&self, blueprint: &mut SiteBlueprint, sample_size: usize) {
        let sample_urls: Vec<String> = blueprint
            .sitemap_urls
            .iter()
            .take(sample_size)
            .map(|u| u.url.clone())
            .collect();

        if sample_urls.is_empty() {
            return;
        }

        let mut all_keywords: Vec<String> = Vec::new();
        let fetches: Vec<_> = sample_urls
            .iter()
            .map(|url| self.fetch_page_text(url))
            .collect();
        let texts = futures::future::join_all(fetches).await;

        for text in texts.into_iter().flatten() {
            if !text.is_empty() {
                let keywords = super::util::extract_top_words(&text, 10);
                all_keywords.extend(keywords);
            }
        }

        // Deduplicate and keep most frequent keywords.
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for kw in all_keywords {
            *counts.entry(kw).or_insert(0) += 1;
        }
        let mut sorted: Vec<(String, usize)> = counts.into_iter().collect();
        sorted.sort_by_key(|a| std::cmp::Reverse(a.1));
        blueprint.topic_keywords = sorted.into_iter().take(20).map(|(kw, _)| kw).collect();
    }

    /// Fetch a page and extract its visible text content.
    async fn fetch_page_text(&self, url: &str) -> Option<String> {
        let response = self.client.get(url).send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        let html = response.text().await.ok()?;
        // Simple text extraction: strip tags, collect text nodes.
        let mut text = String::new();
        let mut in_tag = false;
        for ch in html.chars() {
            match ch {
                '<' => in_tag = true,
                '>' => in_tag = false,
                _ if !in_tag => text.push(ch),
                _ => {}
            }
        }
        // Collapse whitespace and truncate.
        let cleaned: String = text.split_whitespace().collect::<Vec<&str>>().join(" ");
        Some(cleaned.chars().take(5000).collect())
    }

    async fn fetch_robots(client: &reqwest::Client, url: &str) -> Result<RobotsPolicy> {
        let response = client.get(url).send().await;
        let text = match response {
            Ok(r) if r.status().is_success() => r.text().await.context("read robots.txt body")?,
            Ok(r) => {
                debug!("robots.txt returned status {} at {}", r.status(), url);
                return Ok(RobotsPolicy::default());
            }
            Err(e) => return Err(e.into()),
        };

        Ok(RobotsPolicy::parse(&text))
    }

    /// Fetch the site's curated LLM index: `/llms.txt` first (the canonical
    /// filename from the llmstxt.org proposal), falling back to `/llm.txt`
    /// (a compatibility variant some sites publish). Returns `None` when
    /// neither file exists.
    async fn fetch_llms_index(
        client: &reqwest::Client,
        llms_url: &str,
        llm_url: &str,
    ) -> Result<Option<LlmsIndex>> {
        for url in [llms_url, llm_url] {
            let response = client.get(url).send().await;
            let text = match response {
                Ok(r) if r.status().is_success() => r.text().await.context("read llms.txt body")?,
                Ok(r) => {
                    debug!("llms.txt returned status {} at {}", r.status(), url);
                    continue;
                }
                Err(e) => {
                    debug!("llms.txt fetch failed at {}: {}", url, e);
                    continue;
                }
            };
            let index = LlmsIndex::parse(&text);
            if index.site_name.is_some() || !index.sections.is_empty() {
                debug!("parsed LLM index from {}", url);
                return Ok(Some(index));
            }
        }
        Ok(None)
    }

    async fn fetch_sitemap(&self, url: impl AsRef<str>, depth: usize) -> Result<Vec<SitemapUrl>> {
        let url = url.as_ref();
        if depth > self.max_sitemap_depth {
            debug!("sitemap depth limit reached for {}", url);
            return Ok(Vec::new());
        }

        let response = self.client.get(url).send().await;
        let bytes = match response {
            Ok(r) if r.status().is_success() => r.bytes().await.context("read sitemap body")?,
            Ok(r) => {
                debug!("sitemap returned status {} at {}", r.status(), url);
                return Ok(Vec::new());
            }
            Err(e) => return Err(e.into()),
        };

        // Handle both plain XML and .gz compressed sitemaps.
        let body = if bytes.starts_with(&[0x1f, 0x8b]) {
            let mut decoder = flate2::read::GzDecoder::new(&bytes[..]);
            let mut text = String::new();
            decoder
                .read_to_string(&mut text)
                .context("decode gzip sitemap")?;
            text
        } else {
            String::from_utf8_lossy(&bytes).to_string()
        };

        let (mut urls, nested) = Self::parse_sitemap_body(&body, url)?;
        self.resolve_nested_sitemaps(&mut urls, nested, depth).await;
        Ok(urls)
    }

    /// Synchronous sitemap body parser. Returns discovered URLs and nested
    /// sitemap index URLs. Exposed for unit testing.
    pub fn parse_sitemap_body(
        body: &str,
        source_url: &str,
    ) -> Result<(Vec<SitemapUrl>, Vec<String>)> {
        let cursor = Cursor::new(body.as_bytes());
        let reader = SiteMapReader::new(cursor);
        let mut urls = Vec::new();
        let mut nested_sitemaps = Vec::new();

        for entity in reader {
            match entity {
                SiteMapEntity::Url(entry) => {
                    if let Some(url) = entry.loc.get_url() {
                        let priority = entry.priority.get_priority().unwrap_or(0.5);
                        let lastmod = entry.lastmod.get_time().map(|dt| dt.with_timezone(&Utc));
                        let changefreq = match entry.changefreq {
                            sitemap::structs::ChangeFreq::None => None,
                            other => Some(other.as_str().to_string()),
                        };
                        urls.push(SitemapUrl {
                            url: url.to_string(),
                            lastmod,
                            changefreq,
                            priority,
                        });
                    }
                }
                SiteMapEntity::SiteMap(entry) => {
                    if let Some(url) = entry.loc.get_url() {
                        nested_sitemaps.push(url.to_string());
                    }
                }
                SiteMapEntity::Err(e) => {
                    warn!("sitemap parse error in {}: {}", source_url, e);
                }
            }
        }

        Ok((urls, nested_sitemaps))
    }

    async fn resolve_nested_sitemaps(
        &self,
        urls: &mut Vec<SitemapUrl>,
        nested: Vec<String>,
        depth: usize,
    ) {
        if nested.is_empty() || depth >= self.max_sitemap_depth {
            return;
        }
        let mut fetches = Vec::new();
        for nested_url in nested {
            fetches.push(self.fetch_sitemap(nested_url, depth + 1));
        }
        let nested_results = futures::future::join_all(fetches).await;
        for more in nested_results.into_iter().flatten() {
            urls.extend(more);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_robots_policy_allows_root() {
        let policy = RobotsPolicy {
            raw: "User-agent: *\nDisallow: /private/\n".to_string(),
            sitemap_urls: vec![],
            crawl_delay_ms: None,
        };
        assert!(policy.is_allowed("https://example.com/"));
        assert!(policy.is_allowed("https://example.com/public/page"));
        assert!(!policy.is_allowed("https://example.com/private/secret"));
    }

    #[test]
    fn test_robots_policy_empty_allows_everything() {
        let policy = RobotsPolicy::default();
        assert!(policy.is_allowed("https://example.com/anything"));
    }

    #[test]
    fn test_parse_sitemap_index() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap>
    <loc>https://example.com/sitemap-pages.xml</loc>
    <lastmod>2024-01-15T00:00:00+00:00</lastmod>
  </sitemap>
</sitemapindex>"#;
        let (urls, nested) =
            SiteExplorer::parse_sitemap_body(body, "https://example.com/sitemap.xml").unwrap();
        assert!(urls.is_empty());
        assert_eq!(nested, vec!["https://example.com/sitemap-pages.xml"]);
    }
    #[test]
    fn test_parse_robots_crawl_delay_and_sitemap() {
        let body = "User-agent: *\nCrawl-delay: 1.5\nSitemap: https://example.com/s2.xml\n";
        let policy = RobotsPolicy::parse(body);
        assert_eq!(policy.crawl_delay_ms, Some(1500));
        assert_eq!(policy.sitemap_urls, vec!["https://example.com/s2.xml"]);
    }

    #[test]
    fn test_parse_sitemap_body() {
        let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/page1</loc>
    <lastmod>2024-01-15T00:00:00+00:00</lastmod>
    <changefreq>daily</changefreq>
    <priority>0.8</priority>
  </url>
  <url>
    <loc>https://example.com/page2</loc>
    <priority>0.3</priority>
  </url>
</urlset>"#;
        let (urls, nested) =
            SiteExplorer::parse_sitemap_body(body, "https://example.com/sitemap.xml").unwrap();
        assert!(nested.is_empty());
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0].url, "https://example.com/page1");
        assert!((urls[0].priority - 0.8).abs() < f32::EPSILON);
        assert_eq!(urls[0].changefreq.as_deref(), Some("daily"));
        assert_eq!(urls[1].url, "https://example.com/page2");
    }

    #[test]
    fn test_parse_llms_index_full() {
        // Mirrors the canonical llmstxt.org example: BOM, H1, blockquote
        // summary, context paragraph, then H2 sections with link lists.
        let body = "\u{feff}# Example Company\n\
> Example Company provides examples for everyone.\n\
\n\
Example Company is a fictional company used in examples.\n\
\n\
## Useful Links\n\
- [Main Website](https://example.com/): The main website of Example Company.\n\
- [Documentation](https://docs.example.com/)\n\
- [Blog](https://example.com/blog): News and updates.\n\
\n\
## API Reference\n\
- [REST API](https://api.example.com/v1)\n";
        let index = LlmsIndex::parse(body);
        assert_eq!(index.site_name.as_deref(), Some("Example Company"));
        assert_eq!(
            index.summary.as_deref(),
            Some("Example Company provides examples for everyone.")
        );
        assert_eq!(index.sections.len(), 2);
        assert_eq!(index.sections[0].heading, "Useful Links");
        assert_eq!(index.sections[0].entries.len(), 3);
        assert_eq!(index.sections[0].entries[0].name, "Main Website");
        assert_eq!(index.sections[0].entries[0].url, "https://example.com/");
        assert_eq!(
            index.sections[0].entries[0].description.as_deref(),
            Some("The main website of Example Company.")
        );
        // Entry without description.
        assert_eq!(
            index.sections[0].entries[1].url,
            "https://docs.example.com/"
        );
        assert!(index.sections[0].entries[1].description.is_none());
        assert_eq!(index.sections[1].heading, "API Reference");
        assert_eq!(index.sections[1].entries.len(), 1);
        // urls() flattens all sections in document order.
        let urls: Vec<&str> = index.urls().collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/",
                "https://docs.example.com/",
                "https://example.com/blog",
                "https://api.example.com/v1",
            ]
        );
    }

    #[test]
    fn test_parse_llms_index_garbage_is_empty() {
        let index = LlmsIndex::parse("not an llms.txt at all\nno headings\nrandom prose\n");
        assert!(index.site_name.is_none());
        assert!(index.sections.is_empty());
        // A malformed list item must not panic and yields no entry.
        let index = LlmsIndex::parse("- [broken link without parens\n");
        assert!(index.sections.is_empty() || index.sections[0].entries.is_empty());
    }

    #[test]
    fn test_parse_llm_txt_variant() {
        // The llm.txt compatibility variant carries the same structure.
        let body = "# Docs\n> All docs.\n\n## Guides\n- [Quickstart](https://docs.example.com/quickstart): 5 minutes.\n";
        let index = LlmsIndex::parse(body);
        assert_eq!(index.site_name.as_deref(), Some("Docs"));
        assert_eq!(
            index.sections[0].entries[0].url,
            "https://docs.example.com/quickstart"
        );
    }
}
