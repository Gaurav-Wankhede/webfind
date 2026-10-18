//! Match-based example extraction.
//!
//! Converts fetched page content into typed [`ExtractedExample`]s. Extraction is
//! *match-driven*: an example is emitted only when the content actually
//! contains a detectable artifact (fenced code block, LaTeX, SMILES, markdown
//! table, mermaid/dot diagram, or an image). Pages with none of these produce
//! an empty list and therefore never trigger the explanation pipeline.

use regex::Regex;
use std::sync::LazyLock;

use crate::pipeline::types::{ExtractedExample, ExampleType};
use crate::schema::content::StructuredContent;

/// Fenced code block: ```lang ... ``` or ~~~lang ... ~~~
static RE_FENCED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)```([A-Za-z0-9_+#.-]*)\s*\n(.*?)```").expect("valid fenced regex")
});

/// Display / inline LaTeX: $$...$$ or \[...\] or $...$
static RE_LATEX_BLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)\$\$(.+?)\$\$|\\\[(.+?)\\\]").expect("valid latex regex")
});

/// Markdown table: header row + delimiter row of dashes/pipes.
static RE_TABLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?ms)^\|(.+)\|\s*\n^\|\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)+\|").expect("valid table regex")
});

/// Mermaid / dot / graphviz diagram fence.
static RE_MERMAID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)```(mermaid|dot|graphviz|plantuml)\s*\n(.*?)```").expect("valid diagram regex")
});

/// Markdown image: ![alt](url)
static RE_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").expect("valid image regex"));

/// SMILES / InChI chemical notation.
static RE_SMILES: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b((?:[a-z][a-z]?\d*|\[[^]]+\]|[()=#\\/+\-@]){6,})\b").expect("valid smiles regex")
});

/// Extract every typed example present in `content`.
pub fn extract_examples(content: &StructuredContent) -> Vec<ExtractedExample> {
    let mut out = Vec::new();
    let md = &content.content_markdown;

    // Prefer already-parsed DOM entities (they carry language + line counts).
    if !content.entities.code_blocks.is_empty() {
        for cb in &content.entities.code_blocks {
            let lang = cb.language.clone().or_else(|| detect_language(&cb.code));
            out.push(ExtractedExample {
                example_type: ExampleType::Code,
                content: cb.code.clone(),
                language: lang,
                source_url: content.url.clone(),
                source_tier: crate::engine::source_tier::SourceTier::Unknown,
                ..Default::default()
            });
        }
    } else {
        for cap in RE_FENCED.captures_iter(md) {
            let lang = cap.get(1).map(|m| m.as_str().trim()).filter(|s| !s.is_empty());
            let body = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
            if body.len() < 8 {
                continue;
            }
            out.push(ExtractedExample {
                example_type: ExampleType::Code,
                content: body.to_string(),
                language: lang.map(str::to_string),
                source_url: content.url.clone(),
                source_tier: crate::engine::source_tier::SourceTier::Unknown,
                ..Default::default()
            });
        }
    }

    // Diagrams (checked before generic code so a mermaid fence is typed correctly).
    for cap in RE_MERMAID.captures_iter(md) {
        let kind = cap.get(1).map(|m| m.as_str().to_lowercase()).unwrap_or_default();
        let body = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if body.len() < 6 {
            continue;
        }
        out.push(ExtractedExample {
            example_type: ExampleType::Diagram,
            content: body.to_string(),
            mermaid: if kind == "mermaid" { Some(body.to_string()) } else { None },
            language: Some(kind),
            source_url: content.url.clone(),
            source_tier: crate::engine::source_tier::SourceTier::Unknown,
            ..Default::default()
        });
    }

    // Math.
    for cap in RE_LATEX_BLOCK.captures_iter(md) {
        let body = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str().trim())
            .unwrap_or("");
        if body.len() < 3 {
            continue;
        }
        out.push(ExtractedExample {
            example_type: ExampleType::Math,
            content: body.to_string(),
            latex: Some(body.to_string()),
            source_url: content.url.clone(),
            source_tier: crate::engine::source_tier::SourceTier::Unknown,
            ..Default::default()
        });
    }

    // Chemical notation.
    for cap in RE_SMILES.captures_iter(md) {
        let body = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        if looks_like_smiles(body) {
            out.push(ExtractedExample {
                example_type: ExampleType::Chemical,
                content: body.to_string(),
                smiles: Some(body.to_string()),
                source_url: content.url.clone(),
                source_tier: crate::engine::source_tier::SourceTier::Unknown,
                ..Default::default()
            });
        }
    }

    // Tables (prefer parsed DOM tables; fall back to markdown scanning).
    if !content.entities.tables.is_empty() {
        for t in &content.entities.tables {
            let headers = t.headers.join(" | ");
            let mut body = format!("| {} |\n|{}|\n", headers, "-".repeat(headers.len()));
            for row in t.rows.iter().take(20) {
                body.push_str(&format!("| {} |\n", row.join(" | ")));
            }
            out.push(ExtractedExample {
                example_type: ExampleType::Table,
                content: body,
                markdown: Some("table".to_string()),
                source_url: content.url.clone(),
                source_tier: crate::engine::source_tier::SourceTier::Unknown,
                ..Default::default()
            });
        }
    } else {
        for cap in RE_TABLE.captures_iter(md) {
            let whole = cap.get(0).map(|m| m.as_str()).unwrap_or("");
            let table = whole.lines().take(21).collect::<Vec<_>>().join("\n");
            out.push(ExtractedExample {
                example_type: ExampleType::Table,
                content: table,
                markdown: Some("table".to_string()),
                source_url: content.url.clone(),
                source_tier: crate::engine::source_tier::SourceTier::Unknown,
                ..Default::default()
            });
        }
    }

    // Images.
    for cap in RE_IMAGE.captures_iter(md) {
        let alt = cap.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let url = cap.get(2).map(|m| m.as_str().trim()).unwrap_or("");
        if url.is_empty() {
            continue;
        }
        out.push(ExtractedExample {
            example_type: ExampleType::Image,
            content: alt.to_string(),
            image_url: Some(url.to_string()),
            alt_text: if alt.is_empty() { None } else { Some(alt.to_string()) },
            source_url: content.url.clone(),
            source_tier: crate::engine::source_tier::SourceTier::Unknown,
            ..Default::default()
        });
    }

    out
}

/// True when a candidate token really is a SMILES string rather than ordinary
/// prose: requires ring closures / branches / bonds and a chemistry-weighted
/// character mix.
fn looks_like_smiles(s: &str) -> bool {
    if s.len() < 6 {
        return false;
    }
    let has_ring_or_branch = s.contains(['(', ')']) || s.chars().any(|c| c.is_ascii_digit());
    let has_bond = s.contains(['=', '#', '@']);
    let atoms: usize = s
        .chars()
        .filter(|c| matches!(c, 'C' | 'N' | 'O' | 'S' | 'P' | 'F' | 'c' | 'n' | 'o'))
        .count();
    has_ring_or_branch && (has_bond || atoms >= 3)
}

/// Lightweight language detection for fenced blocks without a declared lang.
fn detect_language(code: &str) -> Option<String> {
    let c = code.trim_start();
    if c.starts_with("fn ") || c.contains("-> ") && c.contains("let ") {
        Some("rust".to_string())
    } else if c.starts_with("def ") || c.starts_with("import ") || c.contains("self.") {
        Some("python".to_string())
    } else if c.starts_with("const ") || c.starts_with("let ") || c.starts_with("function") {
        Some("javascript".to_string())
    } else if c.starts_with("SELECT ") || c.starts_with("CREATE TABLE") {
        Some("sql".to_string())
    } else if c.starts_with('{') && c.ends_with('}') {
        Some("json".to_string())
    } else {
        None
    }
}

/// True when at least one example type matched — the match-based gate that
/// decides whether the explanation pipeline runs at all.
pub fn has_examples(examples: &[ExtractedExample]) -> bool {
    !examples.is_empty()
}



#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal `StructuredContent` for extractor tests.
    ///
    /// `StructuredContent` has no `Default` impl, so the fields the extractor
    /// reads are set explicitly and the rest are filled with empty/zero values.
    fn content_with(md: &str) -> StructuredContent {
        StructuredContent {
            url: "https://example.com/page".to_string(),
            final_url: "https://example.com/page".to_string(),
            status_code: 200,
            title: "test".to_string(),
            description: None,
            canonical_url: None,
            language: "en".to_string(),
            language_confidence: 1.0,
            published_at: None,
            modified_at: None,
            author: None,
            site_name: None,
            content_text: md.to_string(),
            content_html: String::new(),
            content_markdown: md.to_string(),
            excerpt: String::new(),
            word_count: 0,
            char_count: 0,
            sentence_count: 0,
            reading_time_seconds: 0,
            reading_ease: 0.0,
            grade_level: 0.0,
            keywords: Vec::new(),
            open_graph: None,
            twitter_card: None,
            json_ld: Vec::new(),
            schema_type: None,
            images: Vec::new(),
            internal_links: Vec::new(),
            external_links: Vec::new(),
            favicon: None,
            rss_url: None,
            normalized_text: String::new(),
            fetched_at: chrono::DateTime::UNIX_EPOCH,
            fetch_duration_ms: 0,
            html_size_bytes: 0,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            content_type: "text/html".to_string(),
            content_type_header: "text/html".to_string(),
            is_paywalled: false,
            is_valid_content: true,
            entities: Default::default(),
        }
    }

    #[test]
    fn extracts_fenced_code_with_language() {
        let md = "Intro.\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\nOutro.";
        let c = content_with(md);
        let ex = extract_examples(&c);
        assert!(!ex.is_empty(), "fenced code must produce an example");
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Code
            && e.content.contains("fn main")));
    }

    #[test]
    fn extracts_latex_and_smiles() {
        let md = "Energy $$E = mc^2$$ and ethanol SMILES C(C)O or benzene c1ccccc1 is common.";
        let c = content_with(md);
        let ex = extract_examples(&c);
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Math));
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Chemical));
    }

    #[test]
    fn extracts_markdown_table() {
        let md = "| Name | Value |\n|------|-------|\n| a | 1 |\n| b | 2 |";
        let c = content_with(md);
        let ex = extract_examples(&c);
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Table));
    }

    #[test]
    fn extracts_mermaid_diagram() {
        let md = "```mermaid\ngraph LR\nA-->B\n```";
        let c = content_with(md);
        let ex = extract_examples(&c);
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Diagram
            && e.mermaid.is_some()));
    }

    #[test]
    fn plain_prose_produces_no_examples() {
        // The match-based gate: no artifact => no examples.
        let c = content_with(
            "This is an ordinary paragraph of prose. It has sentences and words but              contains no code, no math, no tables and no diagrams at all.",
        );
        let ex = extract_examples(&c);
        assert!(ex.is_empty(), "prose-only page must yield zero examples, got {ex:?}");
        assert!(!has_examples(&ex));
    }

    #[test]
    fn extracts_images_with_alt_text() {
        let md = "See ![architecture diagram](https://x.com/d.png) for details.";
        let c = content_with(md);
        let ex = extract_examples(&c);
        assert!(ex.iter().any(|e| e.example_type == ExampleType::Image
            && e.alt_text.as_deref() == Some("architecture diagram")));
    }
}
