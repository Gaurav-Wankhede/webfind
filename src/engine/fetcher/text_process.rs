use unicode_segmentation::UnicodeSegmentation;
use super::container::purge_doc_boilerplate;
use super::url_util::html_escape;
use crate::schema::response::Keyword;
use std::sync::LazyLock;

pub(crate) fn extract_page_keywords(text: &str, max: usize) -> Vec<Keyword> {
    if text.len() < 100 {
        return vec![];
    }

    let stop_words: Vec<String> = stop_words::get(stop_words::Language::English)
        .iter()
        .map(|s| (*s).to_string())
        .collect();

    let rake = keyword_extraction::rake::Rake::new(
        keyword_extraction::rake::RakeParams::WithDefaults(text, &stop_words),
    );

    let ranked = rake.get_ranked_keyword_scores(max);

    ranked
        .into_iter()
        .enumerate()
        .map(|(i, (word, score))| Keyword {
            text: word,
            tfidf_score: score as f64,
            rank: (i + 1) as u32,
        })
        .collect()
}

// ── Word / Sentence Counting ───────────────────────────────────────────────

pub fn count_words(text: &str) -> u32 {
    text.unicode_words().count() as u32
}

pub(crate) fn count_sentences(text: &str) -> u32 {
    text.unicode_sentences().count() as u32
}

pub fn estimate_reading_time(word_count: u32) -> u32 {
    (word_count as f64 / 238.0 * 60.0).ceil() as u32
}

// ── Text Normalization ──────────────────────────────────────────────────────

pub(crate) fn normalize_text(text: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    text.nfc()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Stage-1 streaming pre-pass: strip boilerplate elements at byte level with
/// lol_html before any DOM is built. Removes `nav`, `footer`, `aside`,
/// `script`, `style`, `iframe`, and `noscript` entirely so htmd never sees
/// their text. This is the fastcrw `CleanDepth::Structural` pattern.
pub(crate) fn clean_html_streaming(html: &str) -> String {
    use lol_html::{element, RewriteStrSettings};

    let settings = RewriteStrSettings {
        element_content_handlers: vec![
            element!("script, style, noscript, svg, nav, footer, header", |el| {
                el.remove();
                Ok(())
            }),
        ],
        ..RewriteStrSettings::default()
    };

    lol_html::rewrite_str(html, settings).unwrap_or_else(|_| html.to_owned())
}

pub(crate) fn content_score(html: &str) -> f64 {
    let total = html.len() as f64;
    if total < 1.0 {
        return 0.0;
    }
    // Strip tags cheaply with a char-level scan (no DOM needed for scoring).
    let text_chars = {
        let mut inside_tag = false;
        let mut count = 0usize;
        for c in html.chars() {
            match c {
                '<' => inside_tag = true,
                '>' => inside_tag = false,
                _ if !inside_tag => count += 1,
                _ => {}
            }
        }
        count as f64
    };
    let density = text_chars / total;
    if text_chars < 1.0 {
        return 0.0;
    }
    density * text_chars.ln()
}

pub(crate) fn html_to_markdown(html: &str) -> String {
    // Stage 1 — streaming structural strip (lol_html, byte-level, no DOM alloc)
    let stripped = clean_html_streaming(html);

    // Stage 2 — true GFM Markdown via htmd (replaces html2text)
    // Large width prevents artificial line-breaks across code blocks and long links.
    let converter = htmd::HtmlToMarkdown::builder()
        .skip_tags(vec!["script", "style"])
        .build();
    let raw_md = converter
        .convert(&stripped)
        .unwrap_or_else(|_| stripped.clone());

    // Stage 3 — remove residual community-widget / boilerplate markers
    purge_doc_boilerplate(&raw_md)
}

// ── Excerpt + SPA-shell helpers ─────────────────────────────────────────────

/// Remove inline JS/CSS source and HTML comments from extracted text.
///
/// Readability and doc-container heuristics can hand back raw `<script>` /
/// `<style>` bodies. Left in place they dominate keyword extraction and
/// reading-score math (all scores become meaningless) and render as visible
/// garbage in the report. This strips those blocks while preserving real prose.
pub(crate) fn strip_code_blocks(text: &str) -> String {
    static RE_SCRIPT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?is)<script[^>]*>.*?</script>").expect("valid script regex")
    });
    static RE_STYLE: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?is)<style[^>]*>.*?</style>").expect("valid style regex")
    });
    static RE_COMMENT: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"(?s)<!--.*?-->").expect("valid comment regex")
    });

    let cleaned = RE_SCRIPT.replace_all(text, " ");
    let cleaned = RE_STYLE.replace_all(&cleaned, " ");
    let cleaned = RE_COMMENT.replace_all(&cleaned, " ");
    // Collapse the whitespace the removals introduced.
    let flattened = cleaned
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string();

    // JS-SIGNATURE GUARD: SPA shells sometimes return the bundle source as
    // *plain text* (no <script> wrapper) because the server rendered it into the
    // response body. Detect that shape and drop it so it never reaches keyword
    // extraction or the reading-score math.
    // Segment-level stripping: a page often yields `CSS/JS junk + real prose`
    // in one blob. Rather than discarding the whole thing (losing the prose)
    // or keeping it (poisoning keywords), drop only the junk segments.
    strip_junk_segments(&flattened)
}

/// Split text into whitespace-delimited segments and keep only those that read
/// like natural prose. Junk segments (stylesheet rules, bundle source) are
/// dropped; real sentences survive even when glued to junk by the extractor.
pub(crate) fn strip_junk_segments(text: &str) -> String {
    if text.is_empty() {
        return String::new();
    }
    // Whole blob is junk -> nothing to keep.
    if looks_like_javascript(text) || looks_like_css(text) {
        return String::new();
    }
    // Only filter when the text actually looks like a code/prose mixture.
    // Filtering always would destroy legitimate prose: a sentence such as
    // "A function or function pointer." is short and would be dropped, which
    // silently truncated markdown output on documentation pages.
    if !text.contains(['{', '}']) {
        return text.trim().to_string();
    }
    let kept: Vec<&str> = text
        .split_inclusive('.')
        .flat_map(|sentence| split_glued_junk(sentence))
        .filter(|seg| looks_like_prose(seg))
        .collect();
    if kept.is_empty() {
        return String::new();
    }
    kept.join(" ").trim().to_string()
}

/// A sentence can have junk glued onto its front or back with no separator
/// (`...}body{font:16px/1.6 system-ui} This domain is for use...`). Cut at the
/// first brace boundary that separates code from words.
pub(crate) fn split_glued_junk(sentence: &str) -> Vec<&str> {
    // If there are no braces, the segment is already homogeneous.
    if !sentence.contains(['{', '}']) {
        return vec![sentence];
    }
    let mut parts: Vec<&str> = Vec::new();
    let mut rest = sentence;
    while let Some(idx) = rest.find(['{', '}']) {
        let (head, tail) = rest.split_at(idx);
        let head = head.trim();
        if looks_like_prose(head) {
            parts.push(head);
        }
        // Advance past the brace so we don't re-match the same position.
        rest = &tail[1..];
    }
    let tail = rest.trim();
    if !tail.is_empty() {
        parts.push(tail);
    }
    parts
}

/// True when a short span reads like natural prose rather than code.
pub(crate) fn looks_like_prose(seg: &str) -> bool {
    let trimmed = seg.trim();
    if trimmed.len() < 24 {
        return false;
    }
    let chars = trimmed.chars().count();
    let letters = trimmed.chars().filter(|c| c.is_alphabetic()).count();
    let whitespace = trimmed.chars().filter(|c| c.is_whitespace()).count();
    let braces = trimmed.matches(['{', '}', ';']).count();
    let letter_ratio = letters as f64 / chars as f64;
    let ws_ratio = whitespace as f64 / chars as f64;
    if braces > 0 {
        return false;
    }
    letter_ratio > 0.80 && ws_ratio > 0.10
}

/// Heuristic: true when the text is a stylesheet fragment rather than prose.
///
/// Some extractors emit the page's `<style>` body as plain text (no wrapper),
/// producing `html{color-scheme:light dark;...}body{font:16px/1.6 ...}`.
/// Style rules have selector+braces, `;`-terminated declarations, and no
/// sentence punctuation — a shape natural prose never takes.
pub(crate) fn looks_like_css(text: &str) -> bool {
    if text.len() < 40 {
        return false;
    }
    // Style signatures: declaration separators and braces.
    let semis = text.matches(';').count();
    let braces = text.matches(['{', '}']).count();
    if semis == 0 && braces == 0 {
        return false;
    }
    let chars = text.chars().count();
    let whitespace = text.chars().filter(|c| c.is_whitespace()).count();
    let whitespace_ratio = whitespace as f64 / chars as f64;
    // Prose has sentence ends; CSS declarations end with ';'.
    let sentence_ends = text.matches(['.', '!', '?']).count();
    let words = text.split_whitespace().count().max(1);

    semis >= 1 && braces >= 2 && whitespace_ratio < 0.15 && sentence_ends < (words / 20).max(1)
}

/// Heuristic: true when the text is dominated by JavaScript/CSS source rather
/// than prose. Only fires when a bundle marker is present *and* real sentence
/// punctuation is scarce, so ordinary code snippets in documentation survive.
pub(crate) fn looks_like_javascript(text: &str) -> bool {
    if text.len() < 40 {
        return false;
    }
    let markers = [
        "function(",
        "document.createElement",
        "document.lastElementChild",
        "scripts.forEach",
        "window.__",
        "addEventListener(",
        "getElementsByTagName",
        "contentDocument",
        "querySelectorAll(",
        "=>",
        "undefined",
        "const ",
        "let ",
        "var ",
    ];
    let hits = markers
        .iter()
        .filter(|m| text.contains(**m))
        .count();
    if hits == 0 {
        return false;
    }
    // Prose is dominated by letters and spaces. Bundle source is dominated by
    // punctuation/operators and is short-worded (identifiers, braces, parens).
    let chars = text.chars().count();
    if chars == 0 {
        return false;
    }
    let letters = text.chars().filter(|c| c.is_alphabetic()).count();
    let whitespace = text.chars().filter(|c| c.is_whitespace()).count();
    let punct = text
        .chars()
        .filter(|c| !c.is_alphanumeric() && !c.is_whitespace())
        .count();
    let letter_ratio = letters as f64 / chars as f64;
    let whitespace_ratio = whitespace as f64 / chars as f64;
    let punct_ratio = punct as f64 / chars as f64;

    // Natural prose: mostly letters, healthy whitespace, little punctuation.
    // JavaScript source: low letter ratio, low whitespace, high punctuation.
    hits >= 2 && letter_ratio < 0.82 && whitespace_ratio < 0.18 && punct_ratio > 0.05
}

/// Derive an excerpt from extracted body text.
///
/// Scans for the first sentence that carries enough real words (>= 8) and
/// returns it, trimmed to `max_chars`. Skips leading boilerplate such as
/// language switcher menus, nav links, and "From Wikipedia..." preamble so the
/// excerpt shows content rather than chrome.
pub(crate) fn derive_excerpt(text: &str, max_chars: usize) -> String {
    const MIN_WORDS: usize = 8;

    for raw in text.split_inclusive(['.', '!', '?', '\n']) {
        let sentence = raw.trim();
        if sentence.is_empty() {
            continue;
        }
        // Skip chrome: sentence must not be a pure list of short link words.
        let words: Vec<&str> = sentence.split_whitespace().collect();
        if words.len() < MIN_WORDS {
            continue;
        }
        let long_words = words.iter().filter(|w| w.chars().count() >= 4).count();
        if long_words < 4 {
            continue;
        }
        // Collapse internal whitespace/newlines to a single space.
        let flat = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
        if flat.len() <= max_chars {
            return flat;
        }
        // Truncate on a word boundary inside the budget.
        let mut out = String::new();
        for word in flat.split_whitespace() {
            if out.len() + word.len() + 1 > max_chars {
                break;
            }
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(word);
        }
        if !out.is_empty() {
            return out;
        }
    }
    text.split_whitespace()
        .take(max_chars.div_ceil(6).max(8))
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// True when the cleaned HTML carries no renderable content — the SPA-shell
/// signature (`<div></div>`, `<body></body>`, or only empty wrappers).
pub(crate) fn is_empty_shell(html: &str) -> bool {
    let text: String = html
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>()
        .to_ascii_lowercase();
    // Strip tags and see whether any character remains.
    let mut stripped = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => stripped.push(ch),
            _ => {}
        }
    }
    stripped.is_empty()
}

/// Wrap extracted visible text in minimal HTML so markdown conversion has
/// something to work with when the source page was a JS-only SPA shell.
pub(crate) fn text_to_html(text: &str) -> String {
    let body = text
        .split('\n')
        .map(|line| line.trim())
        .filter(|l| !l.is_empty())
        .map(|line| format!("<p>{}</p>", html_escape(line)))
        .collect::<Vec<_>>()
        .join("");
    if body.is_empty() {
        return String::new();
    }
    format!("<div>{}</div>", body)
}

// ── Regex entity extraction ───────────────────────────────────────────────
//
// Deterministic, zero-cost entity extraction (emails, phones, addresses, ...)
// applied to the *clean* page text inside the fetch pipeline. This gives the
// LLM machine-checkable facts instead of raw text and avoids paying tokens or
// risking nondeterminism by asking a model to extract them. Regexes are
// compiled once and reused via std LazyLock.
