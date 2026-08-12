use crate::schema::content::StructuredContent;
use crate::schema::response::{Keyword, SearchResult};
use chrono::DateTime;

/// Shared rendering context that extracts common fields once.
/// This avoids duplicated field access across different output formats.
#[derive(Debug, Clone)]
pub struct RenderContext {
    pub title: String,
    pub url: String,
    pub author: Option<String>,
    pub published_at: Option<DateTime<chrono::Utc>>,
    pub language: String,
    pub language_confidence: f64,
    pub word_count: u32,
    pub reading_time_seconds: u32,
    pub reading_ease: f64,
    pub grade_level: f64,
    pub excerpt: String,
    pub keywords: Vec<Keyword>,
    pub internal_links: Vec<String>,
    pub external_links: Vec<String>,
    pub is_valid_content: bool,
    pub fetch_duration_ms: u64,
    pub ssl_valid: bool,
    pub is_paywalled: bool,
    pub favicon: Option<String>,
    pub snippet: String,
}

impl RenderContext {
    /// Create from StructuredContent (fetch report)
    pub fn from_content(content: &StructuredContent, snippet: String) -> Self {
        Self {
            title: content.title.clone(),
            url: content.url.clone(),
            author: content.author.clone(),
            published_at: content.published_at,
            language: content.language.clone(),
            language_confidence: content.language_confidence,
            word_count: content.word_count,
            reading_time_seconds: content.reading_time_seconds,
            reading_ease: content.reading_ease,
            grade_level: content.grade_level,
            excerpt: content.excerpt.clone(),
            keywords: content.keywords.clone(),
            internal_links: content.internal_links.clone(),
            external_links: content.external_links.clone(),
            is_valid_content: content.is_valid_content,
            fetch_duration_ms: content.fetch_duration_ms,
            ssl_valid: content.ssl_valid,
            is_paywalled: content.is_paywalled,
            favicon: content.favicon.clone(),
            snippet,
        }
    }

    /// Create from SearchResult (search response)
    pub fn from_result(result: &SearchResult) -> Self {
        Self {
            title: result.title.clone(),
            url: result.url.clone(),
            author: result.author.clone(),
            published_at: result.published_at,
            language: result.language.clone(),
            language_confidence: 1.0, // Not available in SearchResult
            word_count: result.snippet.split_whitespace().count() as u32,
            reading_time_seconds: result
                .content
                .as_ref()
                .map(|c| c.reading_time_seconds)
                .unwrap_or(0),
            reading_ease: 0.0, // Not available
            grade_level: 0.0,  // Not available
            excerpt: result.snippet.clone(),
            keywords: result.keywords.clone().unwrap_or_default(),
            internal_links: vec![],
            external_links: vec![],
            is_valid_content: true,
            fetch_duration_ms: 0,
            ssl_valid: true,
            is_paywalled: false,
            favicon: result.favicon.clone(),
            snippet: result.snippet.clone(),
        }
    }
}

/// Output format trait - implement for each format
pub trait RenderFormat {
    fn render_header(&self, ctx: &RenderContext) -> String;
    fn render_metadata(&self, ctx: &RenderContext) -> String;
    fn render_excerpt(&self, ctx: &RenderContext) -> String;
    fn render_keywords(&self, ctx: &RenderContext, show: bool) -> String;
    fn render_links(&self, ctx: &RenderContext, show: bool) -> String;
    fn render_footer(&self) -> String;

    fn render(&self, ctx: &RenderContext, show_links: bool, show_keywords: bool) -> String {
        let mut out = String::new();
        out.push_str(&self.render_header(ctx));
        out.push_str(&self.render_metadata(ctx));
        out.push_str(&self.render_excerpt(ctx));
        if show_keywords {
            out.push_str(&self.render_keywords(ctx, true));
        }
        if show_links {
            out.push_str(&self.render_links(ctx, true));
        }
        out.push_str(&self.render_footer());
        out
    }
}

/// Box-drawing format (used by print_fetch_report and to_report)
pub struct BoxFormat;

impl RenderFormat for BoxFormat {
    fn render_header(&self, ctx: &RenderContext) -> String {
        let status = if ctx.is_valid_content {
            "OK"
        } else {
            "EXTRACTION_FAILED"
        };
        format!(
            "┌─────────────────────────────────────────────────────────────┐\n\
             │                    WEBFIND FETCH REPORT                    │\n\
             ├─────────────────────────────────────────────────────────────┤\n\
             │ Status:       {:<44} │\n\
             ├─────────────────────────────────────────────────────────────┤\n",
            status
        )
    }

    fn render_metadata(&self, ctx: &RenderContext) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "│ URL:          {:<44} │\n",
            truncate(&ctx.url, 44)
        ));
        out.push_str(&format!(
            "│ Title:        {:<44} │\n",
            truncate(&ctx.title, 44)
        ));
        out.push_str(&format!(
            "│ Language:     {:<44} │\n",
            format!("{} ({:.0}%)", ctx.language, ctx.language_confidence * 100.0)
        ));
        out.push_str(&format!("│ Words:        {:<44} │\n", ctx.word_count));
        out.push_str(&format!(
            "│ Reading time: {:<44} │\n",
            format!("{}s", ctx.reading_time_seconds)
        ));
        out.push_str(&format!(
            "│ Reading ease: {:<44} │\n",
            format!("{:.1}", ctx.reading_ease)
        ));
        out.push_str(&format!(
            "│ Grade level:  {:<44} │\n",
            format!("{:.1}", ctx.grade_level)
        ));
        out.push_str(&format!(
            "│ SSL:          {:<44} │\n",
            if ctx.ssl_valid { "valid" } else { "invalid" }
        ));
        out.push_str(&format!(
            "│ Paywalled:    {:<44} │\n",
            if ctx.is_paywalled { "yes" } else { "no" }
        ));
        out.push_str(&format!(
            "│ Fetched in:   {:<44} │\n",
            format!("{}ms", ctx.fetch_duration_ms)
        ));

        if let Some(ref author) = ctx.author {
            out.push_str(&format!("│ Author:       {:<44} │\n", truncate(author, 44)));
        }
        if let Some(ref published) = ctx.published_at {
            out.push_str(&format!(
                "│ Published:    {:<44} │\n",
                published.format("%Y-%m-%d %H:%M UTC")
            ));
        }
        out.push_str("├─────────────────────────────────────────────────────────────┤\n");
        out
    }

    fn render_excerpt(&self, ctx: &RenderContext) -> String {
        let mut out = String::new();
        out.push_str("│ EXCERPT                                                   │\n");
        out.push_str("├─────────────────────────────────────────────────────────────┤\n");
        for line in word_wrap(&ctx.excerpt, 59) {
            out.push_str(&format!("│ {:<59} │\n", line));
        }
        out
    }

    fn render_keywords(&self, ctx: &RenderContext, show: bool) -> String {
        if !show || ctx.keywords.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str("├─────────────────────────────────────────────────────────────┤\n");
        out.push_str("│ KEYWORDS                                                  │\n");
        out.push_str("├─────────────────────────────────────────────────────────────┤\n");
        let kw_str: Vec<String> = ctx
            .keywords
            .iter()
            .take(10)
            .map(|k| format!("{} ({:.1})", k.text, k.tfidf_score))
            .collect();
        for line in word_wrap(&kw_str.join(", "), 59) {
            out.push_str(&format!("│ {:<59} │\n", line));
        }
        out
    }

    fn render_links(&self, ctx: &RenderContext, show: bool) -> String {
        if !show {
            return String::new();
        }
        let mut out = String::new();
        if !ctx.internal_links.is_empty() {
            out.push_str(&format!(
                "├─────────────────────────────────────────────────────────────┤\n\
                 │ INTERNAL LINKS ({:<44}) │\n\
                 ├─────────────────────────────────────────────────────────────┤\n",
                ctx.internal_links.len()
            ));
            for link in ctx.internal_links.iter().take(10) {
                out.push_str(&format!("│  {:<58} │\n", truncate(link, 58)));
            }
        }
        if !ctx.external_links.is_empty() {
            out.push_str(&format!(
                "├─────────────────────────────────────────────────────────────┤\n\
                 │ EXTERNAL LINKS ({:<44}) │\n\
                 ├─────────────────────────────────────────────────────────────┤\n",
                ctx.external_links.len()
            ));
            for link in ctx.external_links.iter().take(10) {
                out.push_str(&format!("│  {:<58} │\n", truncate(link, 58)));
            }
        }
        out
    }

    fn render_footer(&self) -> String {
        "└─────────────────────────────────────────────────────────────┘\n".to_string()
    }
}

/// Markdown format (used by print_fetch_markdown and to_markdown)
pub struct MarkdownFormat;

impl RenderFormat for MarkdownFormat {
    fn render_header(&self, ctx: &RenderContext) -> String {
        format!("# {}\n\n", ctx.title)
    }

    fn render_metadata(&self, ctx: &RenderContext) -> String {
        let mut out = String::new();
        out.push_str(&format!("**URL:** {}\n", ctx.url));
        if let Some(ref author) = ctx.author {
            out.push_str(&format!("**Author:** {}\n", author));
        }
        if let Some(ref published) = ctx.published_at {
            out.push_str(&format!(
                "**Published:** {}\n",
                published.format("%Y-%m-%d %H:%M UTC")
            ));
        }
        out.push_str(&format!(
            "**Language:** {} ({:.0}%)\n",
            ctx.language,
            ctx.language_confidence * 100.0
        ));
        out.push_str(&format!(
            "**Reading time:** {}s | **Words:** {} | **Grade:** {:.1}\n\n",
            ctx.reading_time_seconds, ctx.word_count, ctx.grade_level
        ));
        out
    }

    fn render_excerpt(&self, ctx: &RenderContext) -> String {
        if ctx.excerpt.is_empty() {
            String::new()
        } else {
            format!("> {}\n\n", ctx.excerpt)
        }
    }

    fn render_keywords(&self, ctx: &RenderContext, show: bool) -> String {
        if !show || ctx.keywords.is_empty() {
            return String::new();
        }
        let mut out = String::new();
        out.push_str("## Keywords\n\n");
        for kw in ctx.keywords.iter().take(10) {
            out.push_str(&format!("- {} (score: {:.1})\n", kw.text, kw.tfidf_score));
        }
        out.push('\n');
        out
    }

    fn render_links(&self, ctx: &RenderContext, show: bool) -> String {
        if !show {
            return String::new();
        }
        let mut out = String::new();
        if !ctx.internal_links.is_empty() {
            out.push_str(&format!(
                "## Internal Links ({})\n\n",
                ctx.internal_links.len()
            ));
            for link in ctx.internal_links.iter().take(20) {
                out.push_str(&format!("- {}\n", link));
            }
            out.push('\n');
        }
        if !ctx.external_links.is_empty() {
            out.push_str(&format!(
                "## External Links ({})\n\n",
                ctx.external_links.len()
            ));
            for link in ctx.external_links.iter().take(20) {
                out.push_str(&format!("- {}\n", link));
            }
            out.push('\n');
        }
        out
    }

    fn render_footer(&self) -> String {
        String::new()
    }
}

/// Search response box format (used by report::to_report)
pub struct SearchBoxFormat;

impl RenderFormat for SearchBoxFormat {
    fn render_header(&self, _ctx: &RenderContext) -> String {
        // This will be used with additional response-level context
        // For SearchResponse, we need a different approach
        String::new()
    }

    fn render_metadata(&self, ctx: &RenderContext) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "  #{}  {}  (score: {:.3})\n",
            1,
            ctx.title,
            0.0 // placeholder
        ));
        out.push_str(&format!("      {}\n", ctx.url));
        out.push_str(&format!(
            "      Domain: {} | Lang: {} | Words: {}\n",
            "example.com", ctx.language, ctx.word_count
        ));
        if !ctx.snippet.is_empty() {
            let short: String = ctx.snippet.chars().take(160).collect();
            out.push_str(&format!("      \"{}…\"\n", short));
        }
        out
    }

    fn render_excerpt(&self, _ctx: &RenderContext) -> String {
        String::new()
    }

    fn render_keywords(&self, _ctx: &RenderContext, _show: bool) -> String {
        String::new()
    }

    fn render_links(&self, _ctx: &RenderContext, _show: bool) -> String {
        String::new()
    }

    fn render_footer(&self) -> String {
        String::new()
    }
}

/// Search response markdown format (used by report::to_markdown)
pub struct SearchMarkdownFormat;

impl RenderFormat for SearchMarkdownFormat {
    fn render_header(&self, _ctx: &RenderContext) -> String {
        String::new()
    }

    fn render_metadata(&self, ctx: &RenderContext) -> String {
        format!(
            "## #{} [{}]({})\n\n**Domain:** `{}` | **Score:** {:.3} | **Lang:** {}\n\n",
            1, ctx.title, ctx.url, "example.com", 0.0, ctx.language
        )
    }

    fn render_excerpt(&self, ctx: &RenderContext) -> String {
        if ctx.snippet.is_empty() {
            String::new()
        } else {
            let short: String = ctx.snippet.chars().take(300).collect();
            format!("> {}\n\n", short)
        }
    }

    fn render_keywords(&self, _ctx: &RenderContext, _show: bool) -> String {
        String::new()
    }

    fn render_links(&self, _ctx: &RenderContext, _show: bool) -> String {
        String::new()
    }

    fn render_footer(&self) -> String {
        String::new()
    }
}

pub fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

pub fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        if current.len() + word.len() + 1 > width {
            if !current.is_empty() {
                lines.push(current);
                current = String::new();
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }

    if !current.is_empty() {
        lines.push(current);
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello world", 5), "hell…");
    }

    #[test]
    fn test_word_wrap() {
        let result = word_wrap("hello world this is a test", 10);
        assert!(result.iter().all(|l| l.len() <= 10));
    }
}
