use super::*;
use scraper::Selector;
use scraper::Html;

pub(crate) fn clean_content_html(html: &str) -> String {
    // Step 1: strip HTML comments
    let no_comments = strip_html_comments(html);
    // Step 2: parse fragment via HTML5 spec parser
    let doc = Html::parse_fragment(&no_comments);

    // Step 3: traverse the body and rebuild clean HTML.
    // If <body ...> is not present (e.g. standalone container fragments like <main> or <article>),
    // traverse from the root elements of the fragment directly.
    let mut out = String::with_capacity(no_comments.len());
    if let Ok(body_sel) = Selector::parse("body")
        && let Some(body) = doc.select(&body_sel).next()
    {
        traverse_clean(body, &mut out);
    } else {
        for child in doc.root_element().children() {
            if let Some(child_el) = scraper::ElementRef::wrap(child) {
                traverse_clean(child_el, &mut out);
            }
        }
    }

    // Step 4: normalize whitespace
    normalize_html_whitespace(&out)
}

/// Tags whose entire subtree is kept (structural + semantic content).
/// Matches PLAN.md spec: h1-h6, p, div, span, standard inline formatting.
const CLEAN_KEEP: &[&str] = &[
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "p",
    "div",
    "span",
    "a",
    "ul",
    "ol",
    "li",
    "table",
    "tr",
    "td",
    "th",
    "pre",
    "code",
    "blockquote",
    "em",
    "strong",
    "i",
    "b",
    "u",
    "br",
    "hr",
    "img",
    "figure",
    "figcaption",
    "dl",
    "dt",
    "dd",
    "abbr",
    "cite",
    "q",
    "sub",
    "sup",
    "section",
    "article",
    "main",
];

/// Tags that are completely removed including their content.
/// Matches PLAN.md spec: script, style, nav, header, footer, aside, form, button, head, noscript.
const CLEAN_REMOVE: &[&str] = &[
    "script", "style", "noscript", "iframe", "canvas", "svg", "nav", "header", "footer", "aside",
    "form", "button", "input", "select", "textarea", "label", "option", "head", "link", "meta",
    "dialog",
];

/// Helper to identify noisy classes/IDs for cookies, anchors, sidebar menus, and copy buttons
pub(crate) fn is_noise_element(el: &scraper::ElementRef<'_>) -> bool {
    let val = el.value();
    
    // 1. Aria hidden decorative elements
    if val.attr("aria-hidden") == Some("true") {
        return true;
    }

    // 2. Class-based noise check
    const NOISE_CLASS_SUBSTRINGS: &[&str] = &[
        "anchor", "headerlink", "src-link", "srclink", "view-source",
        "copy-button", "copy-icon", "collapse-toggle", "cookie", "consent",
        "onetrust", "didomi", "sidebar", "extra-services", "submission-history",
        "katex-html", "bc-table", "baseline-indicator", "breadcrumb",
        "mw-editsection", "citation", "post-signature", "user-info",
        "js-voting-container", "js-vote-count", "comments-link", "subline",
        "file-navigation", "repository-content-header", "blob-num",
        "line-numbers", "subscribe", "newsletter", "social-share",
        "share-bar", "reaction", "clap", "author-card", "author-bio",
        "related-posts", "recommended", "topic-timeline", "topic-map",
        "page-rating", "feedback-prompt", "affiliate", "ad-disclosure",
        "sponsored", "disclaimer", "modal", "popup", "overlay",
        "signup-wall", "lead-form", "quote-widget", "mortgage-calculator",
        "rate-quote", "contact-agent", "enrollment", "cta-banner",
        "no-print", "print-only",
    ];

    if let Some(classes) = val.attr("class") {
        let cls = classes.to_ascii_lowercase();
        if cls == "reference" || NOISE_CLASS_SUBSTRINGS.iter().any(|needle| cls.contains(needle)) {
            return true;
        }
    }

    // 3. ID-based noise check
    const NOISE_ID_SUBSTRINGS: &[&str] = &[
        "cookie", "consent", "onetrust", "sidebar", "mw-navigation",
        "subscribe", "newsletter", "modal", "popup", "disclaimer",
    ];

    if let Some(id) = val.attr("id") {
        let id_lower = id.to_ascii_lowercase();
        if id_lower == "toc" || NOISE_ID_SUBSTRINGS.iter().any(|needle| id_lower.contains(needle)) {
            return true;
        }
    }

    false
}

/// Recursive DOM traversal: rebuilds clean HTML from the scraper tree.
fn traverse_clean(el: scraper::ElementRef<'_>, out: &mut String) {
    let tag = el.value().name();
    let tag_lower = tag.to_ascii_lowercase();

    if CLEAN_REMOVE.contains(&tag_lower.as_str()) || is_noise_element(&el) {
        return;
    }

    // Void / self-closing elements — emit inline
    if tag_lower == "br" {
        out.push_str("<br>");
        return;
    }
    if tag_lower == "hr" {
        out.push_str("<hr>");
        return;
    }
    if tag_lower == "img" {
        if let Some(src) = el.value().attr("src") {
            let alt = el.value().attr("alt").unwrap_or("");
            out.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\">",
                html_escape(src),
                html_escape(alt)
            ));
        }
        return;
    }

    // Unknown / custom elements — strip entirely (e.g. <div class="ad">, <tracking-widget>)
    if !CLEAN_KEEP.contains(&tag_lower.as_str()) && !CLEAN_REMOVE.contains(&tag_lower.as_str()) {
        // Unknown tag: skip it + its content
        return;
    }

    // Open the tag with preserved attributes for critical elements
    out.push('<');
    out.push_str(tag_lower.as_str());

    // Preserve href on links (traceability / citation)
    if tag_lower == "a"
        && let Some(href) = el.value().attr("href") {
            out.push_str(&format!(" href=\"{}\"", html_escape(href)));
        }

    // Preserve code language classes (e.g. class="language-rust" or class="highlight-python")
    if (tag_lower == "code" || tag_lower == "pre")
        && let Some(cls) = el.value().attr("class") {
            let lang_tokens: Vec<&str> = cls.split_whitespace()
                .filter(|c| c.starts_with("language-") || c.starts_with("lang-") || c.starts_with("highlight-"))
                .collect();
            if !lang_tokens.is_empty() {
                out.push_str(&format!(" class=\"{}\"", html_escape(&lang_tokens.join(" "))));
            }
        }

    out.push('>');

    // Recursively process children
    for child in el.children() {
        if let Some(child_el) = scraper::ElementRef::wrap(child) {
            traverse_clean(child_el, out);
        } else if let Some(text) = child.value().as_text() {
            out.push_str(text);
        }
    }

    out.push_str("</");
    out.push_str(tag_lower.as_str());
    out.push('>');
    out.push('\n');
}

/// Strip HTML comments (`<!-- ... -->`) from a string.
pub(crate) fn strip_html_comments(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // Check for <!--
        if i + 3 < len
            && bytes[i] == b'<'
            && bytes[i + 1] == b'!'
            && bytes[i + 2] == b'-'
            && bytes[i + 3] == b'-'
        {
            // Skip until -->
            i += 4;
            while i + 2 < len && !(bytes[i] == b'-' && bytes[i + 1] == b'-' && bytes[i + 2] == b'>')
            {
                i += 1;
            }
            i += 3; // skip -->
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }
    result
}

/// Collapse consecutive whitespace (including newlines) into single spaces,
/// then trim leading/trailing whitespace.
pub(crate) fn normalize_html_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}
