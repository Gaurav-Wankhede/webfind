use scraper::{Html, Selector};
use super::cleaner::is_noise_element;
use super::text_process::normalize_text;

pub(crate) fn extract_h1(doc: &Html) -> Option<String> {
    Selector::parse("h1")
        .ok()
        .and_then(|sel| doc.select(&sel).next())
        .map(|el| el.text().collect::<Vec<_>>().join("").trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Extract semantic documentation container (<article>, <main>, role="main", docs containers).
/// Returns (clean_inner_html, clean_text).
pub(crate) fn extract_doc_container(doc: &Html) -> Option<(String, String)> {
    const DOC_SELECTORS: &[&str] = &[
        "article",
        "main",
        "[role=\"main\"]",
        ".markdown-body",
        ".mw-parser-output",
        "#answers",
        "blockquote.abstract",
        ".article-content",
        ".docs-content",
        ".markdown-section",
        ".documentation",
        ".document",
        ".rfcMarkup",
        "#content",
        "#main-content",
        "#page-content",
    ];

    for sel_str in DOC_SELECTORS {
        if let Ok(sel) = Selector::parse(sel_str)
            && let Some(el) = doc.select(&sel).next() {
                let mut text_parts = Vec::new();
                let skip_sel = Selector::parse(
                    "script,style,noscript,iframe,canvas,svg,nav,header,footer,aside,form,button",
                )
                .ok();
                collect_text_nodes(el, &skip_sel, &mut text_parts);
                let raw_text = normalize_text(&text_parts.join(" "));
                let purged_text = purge_doc_boilerplate(&raw_text);

                if !purged_text.is_empty() && purged_text.len() >= 40 {
                    let inner_html = el.html();
                    return Some((inner_html, purged_text));
                }
            }
    }
    None
}

/// Strip documentation boilerplate, feedback questions, and community widgets.
pub(crate) fn purge_doc_boilerplate(text: &str) -> String {
    let boilerplate_markers = [
        "was this helpful?",
        "give feedback about this article",
        "need more help? try these next steps",
        "was this article helpful?",
        "send feedback about our help center",
        "post to the help community",
        "thanks for reading! subscribe for free",
        "leave a comment",
        "sign up to continue reading",
        "ready for more? subscribe",
        // Cross-domain boilerplate delimiters
        "the information on this website is for informational purposes only and is not intended as medical advice",
        "this information is not intended to be legal advice",
        "this content is for educational purposes only and does not constitute financial advice",
        "we may receive a commission if you click on a link",
        "advertiser disclosure: the offers that appear",
    ];

    let lower = text.to_lowercase();
    let mut cutoff = text.len();

    for marker in boilerplate_markers {
        if let Some(pos) = lower.find(marker)
            && pos < cutoff && pos > 0 {
                cutoff = pos;
            }
    }

    text[..cutoff].trim().to_string()
}

/// Strip scripts, styles, and noscript tags, then collect visible body text.
pub(crate) fn fallback_body_text(doc: &Html) -> String {
    let mut text_parts = Vec::new();
    if let Ok(body_sel) = Selector::parse("body")
        && let Some(body) = doc.select(&body_sel).next() {
            let skip_sel = Selector::parse(
                "script,style,noscript,iframe,canvas,svg,nav,header,footer,aside,form,button",
            )
            .ok();
            collect_text_nodes(body, &skip_sel, &mut text_parts);
        }
    let raw_text = normalize_text(&text_parts.join(" "));
    purge_doc_boilerplate(&raw_text)
}

pub(crate) fn collect_text_nodes(
    node: scraper::ElementRef<'_>,
    skip_sel: &Option<Selector>,
    out: &mut Vec<String>,
) {
    for child in node.children() {
        if let Some(child_el) = scraper::ElementRef::wrap(child) {
            if is_noise_element(&child_el) {
                continue;
            }
            if skip_sel
                .as_ref()
                .map(|_sel| {
                    child_el.value().name() == "script"
                        || child_el.value().name() == "style"
                        || child_el.value().name() == "noscript"
                        || child_el.value().name() == "iframe"
                        || child_el.value().name() == "canvas"
                        || child_el.value().name() == "svg"
                })
                .unwrap_or(false)
            {
                continue;
            }
            collect_text_nodes(child_el, skip_sel, out);
        } else if let Some(text) = child.value().as_text() {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                out.push(trimmed.to_string());
            }
        }
    }
}
