use std::collections::HashSet;
use chrono::{DateTime, Utc};
use scraper::{Html, Selector};
use serde_json::Value;
use crate::schema::content::{ImageInfo, OpenGraph, TwitterCard};

fn extract_domain(url: &str) -> String {
    crate::engine::util::extract_domain(url).unwrap_or_default()
}

#[derive(Debug, Default)]
pub(crate) struct PageMeta {
    pub title: Option<String>,
    pub description: Option<String>,
    pub canonical_url: Option<String>,
    pub published_time: Option<String>,
    pub modified_time: Option<String>,
    pub updated_time: Option<String>,
    pub last_modified: Option<String>,
    pub author: Option<String>,
}

pub(crate) fn extract_meta(doc: &Html) -> PageMeta {
    let mut title = None;
    let mut description = None;
    let mut canonical_url = None;
    let mut published_time = None;
    let mut modified_time = None;
    let mut updated_time = None;
    let mut last_modified = None;
    let mut author = None;

    // <title>
    if let Ok(sel) = Selector::parse("title")
        && let Some(el) = doc.select(&sel).next() {
            let t: String = el.text().collect::<Vec<_>>().join("").trim().to_string();
            if !t.is_empty() {
                title = Some(t);
            }
        }

    // <meta> tags
    if let Ok(sel) = Selector::parse("meta[name],meta[property]") {
        for el in doc.select(&sel) {
            let name = el
                .value()
                .attr("name")
                .or_else(|| el.value().attr("property"));
            let content = el.value().attr("content");
            if let (Some(name), Some(content)) = (name, content) {
                match name {
                    "description" => {
                        if description.is_none() {
                            description = Some(content.to_string());
                        }
                    }
                    "author" => author = Some(content.to_string()),
                    "article:published_time"
                    | "datePublished"
                    | "publish_date"
                    | "date"
                    | "publish-date" => {
                        if published_time.is_none() {
                            published_time = Some(content.to_string());
                        }
                    }
                    "article:modified_time"
                    | "dateModified"
                    | "modified_time"
                    | "modified-date" => {
                        if modified_time.is_none() {
                            modified_time = Some(content.to_string());
                        }
                    }
                    "updated_time" | "dateUpdated" | "updated-date" => {
                        if updated_time.is_none() {
                            updated_time = Some(content.to_string());
                        }
                    }
                    "last-modified" | "last_modified"
                        if last_modified.is_none() => {
                            last_modified = Some(content.to_string());
                        }
                    _ => {}
                }
            }
        }
    }

    // <link rel="canonical">
    if let Ok(sel) = Selector::parse("link[rel=canonical]")
        && let Some(el) = doc.select(&sel).next()
            && let Some(href) = el.value().attr("href") {
                canonical_url = Some(href.to_string());
            }

    PageMeta {
        title,
        description,
        canonical_url,
        published_time,
        modified_time,
        updated_time,
        last_modified,
        author,
    }
}

// ── OpenGraph ───────────────────────────────────────────────────────────────

pub(crate) fn extract_open_graph(doc: &Html) -> Option<OpenGraph> {
    let mut og = OpenGraph {
        title: None,
        r#type: None,
        image: None,
        url: None,
        description: None,
        site_name: None,
        locale: None,
        article_author: None,
        article_published_time: None,
        article_modified_time: None,
        article_section: None,
        article_tags: vec![],
    };

    if let Ok(sel) = Selector::parse("meta[property^=\"og:\"]") {
        for el in doc.select(&sel) {
            let prop = el.value().attr("property")?;
            let content = el.value().attr("content")?;
            match prop {
                "og:title" => og.title = Some(content.to_string()),
                "og:type" => og.r#type = Some(content.to_string()),
                "og:image" => og.image = Some(content.to_string()),
                "og:url" => og.url = Some(content.to_string()),
                "og:description" => og.description = Some(content.to_string()),
                "og:site_name" => og.site_name = Some(content.to_string()),
                "og:locale" => og.locale = Some(content.to_string()),
                "article:author" => og.article_author = Some(content.to_string()),
                "article:published_time" => og.article_published_time = Some(content.to_string()),
                "article:modified_time" => og.article_modified_time = Some(content.to_string()),
                "article:section" => og.article_section = Some(content.to_string()),
                "article:tag" => og.article_tags.push(content.to_string()),
                _ => {}
            }
        }
    }

    if og.title.is_some() || og.description.is_some() {
        Some(og)
    } else {
        None
    }
}

// ── Twitter Card ────────────────────────────────────────────────────────────

pub(crate) fn extract_twitter_card(doc: &Html) -> Option<TwitterCard> {
    let mut tc = TwitterCard {
        card: None,
        site: None,
        creator: None,
        title: None,
        description: None,
        image: None,
    };

    if let Ok(sel) = Selector::parse("meta[name^=\"twitter:\"]") {
        for el in doc.select(&sel) {
            let name = el.value().attr("name")?;
            let content = el.value().attr("content")?;
            match name {
                "twitter:card" => tc.card = Some(content.to_string()),
                "twitter:site" => tc.site = Some(content.to_string()),
                "twitter:creator" => tc.creator = Some(content.to_string()),
                "twitter:title" => tc.title = Some(content.to_string()),
                "twitter:description" => tc.description = Some(content.to_string()),
                "twitter:image" => tc.image = Some(content.to_string()),
                _ => {}
            }
        }
    }

    if tc.card.is_some() || tc.title.is_some() {
        Some(tc)
    } else {
        None
    }
}

// ── JSON-LD ─────────────────────────────────────────────────────────────────

pub(crate) fn extract_json_ld(doc: &Html) -> Vec<Value> {
    let mut results = vec![];
    if let Ok(sel) = Selector::parse("script[type=\"application/ld+json\"]") {
        for el in doc.select(&sel) {
            let text: String = el.text().collect();
            if let Ok(val) = serde_json::from_str::<Value>(&text) {
                results.push(val);
            }
        }
    }
    results
}

// ── Links ───────────────────────────────────────────────────────────────────

pub(crate) fn extract_links(doc: &Html, base_url: &str) -> (Vec<String>, Vec<String>) {
    let base_domain = extract_domain(base_url);
    let mut internal = vec![];
    let mut external = vec![];
    let mut seen = HashSet::new();

    if let Ok(sel) = Selector::parse("a[href]") {
        for el in doc.select(&sel) {
            if let Some(href) = el.value().attr("href") {
                let href = href.trim();
                if href.is_empty() || href.starts_with('#') || href.starts_with("javascript:") {
                    continue;
                }
                if !seen.insert(href) {
                    continue;
                }

                let full_url = if href.starts_with("http") {
                    href.to_string()
                } else if href.starts_with("//") {
                    format!("https:{}", href)
                } else if href.starts_with('/') {
                    if let Some(pos) = base_url.find("://") {
                        let scheme_end = pos + 3;
                        if let Some(slash_pos) = base_url[scheme_end..].find('/') {
                            format!("{}{}", &base_url[..scheme_end + slash_pos], href)
                        } else {
                            format!("{}{}", base_url, href)
                        }
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };

                let link_domain = extract_domain(&full_url);
                if link_domain == base_domain || link_domain.is_empty() {
                    internal.push(full_url);
                } else {
                    external.push(full_url);
                }
            }
        }
    }

    (internal, external)
}

// ── Images ──────────────────────────────────────────────────────────────────

pub(crate) fn extract_images(doc: &Html) -> Vec<ImageInfo> {
    let mut images = vec![];
    if let Ok(sel) = Selector::parse("img[src]") {
        for el in doc.select(&sel) {
            if let Some(src) = el.value().attr("src") {
                let alt = el.value().attr("alt").map(String::from);
                let width = el.value().attr("width").and_then(|w| w.parse::<u32>().ok());
                let height = el
                    .value()
                    .attr("height")
                    .and_then(|h| h.parse::<u32>().ok());
                images.push(ImageInfo {
                    url: src.to_string(),
                    alt,
                    width,
                    height,
                });
            }
        }
    }
    images
}

// ── Favicon ─────────────────────────────────────────────────────────────────

pub(crate) fn extract_favicon(doc: &Html) -> Option<String> {
    if let Ok(sel) = Selector::parse("link[rel~=\"icon\"]")
        && let Some(el) = doc.select(&sel).next()
            && let Some(href) = el.value().attr("href") {
                return Some(href.to_string());
            }
    Some("/favicon.ico".to_string())
}

// ── RSS ─────────────────────────────────────────────────────────────────────

pub(crate) fn extract_rss(doc: &Html) -> Option<String> {
    if let Ok(sel) = Selector::parse("link[type=\"application/rss+xml\"]")
        && let Some(el) = doc.select(&sel).next()
            && let Some(href) = el.value().attr("href") {
                return Some(href.to_string());
            }
    None
}

// ── Language Detection ──────────────────────────────────────────────────────

pub(crate) fn detect_language(text: &str, html: &str) -> (String, f64) {
    if let Some(info) = whatlang::detect(text) {
        let lang = info.lang().code().to_string();
        let conf = info.confidence();
        return (lang, conf);
    }

    // Fallback: check html lang attribute
    let doc = Html::parse_document(html);
    if let Ok(sel) = Selector::parse("html[lang]")
        && let Some(el) = doc.select(&sel).next()
            && let Some(lang) = el.value().attr("lang") {
                let code = lang.split('-').next().unwrap_or("en").to_string();
                return (code, 0.5);
            }

    ("en".to_string(), 0.3)
}

// ── Date Parsing ────────────────────────────────────────────────────────────

pub(crate) fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    dateparser::parse(s).ok()
}

// ── Keyword Extraction ──────────────────────────────────────────────────────
