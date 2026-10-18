use scraper::{Html, Selector};
use crate::schema::content::{CalloutEntity, CodeBlockEntity, DiagramEntity, ExampleEntity, FaqEntity, ImageInfo, StepEntity};

const DIAGRAM_FP_DENY: &[&str] = &[
    "photograph", "photography", "photographic", "telegraph", "choreograph",
    "logo", "icon", "avatar", "badge", "button", "nav", "social",
    "favicon", "banner", "spinner", "loader", "ad-", "pixel",
];

const DIAGRAM_HINTS: &[&str] = &[
    "diagram", "architecture", "flowchart", "workflow", "schema",
    "uml", "sequence", "pipeline", "topology", "wireframe", "graph",
];

const DIAGRAM_LANGS: &[&str] = &[
    "mermaid", "dot", "graphviz", "plantuml", "d2", "c4", "nomnoml",
];

fn block_language(elem: scraper::ElementRef<'_>, fallback: scraper::ElementRef<'_>) -> Option<String> {
    elem.value()
        .attr("class")
        .or_else(|| fallback.value().attr("class"))
        .and_then(|classes| {
            classes.split_whitespace().find_map(|c| {
                c.strip_prefix("language-")
                    .or_else(|| c.strip_prefix("lang-"))
                    .map(|s| s.to_lowercase())
            })
        })
}

fn is_diagram_image(alt: &str, url: &str) -> bool {
    let hay = format!("{alt} {url}").to_lowercase();
    if DIAGRAM_FP_DENY.iter().any(|d| hay.contains(d)) {
        return false;
    }
    DIAGRAM_HINTS.iter().any(|h| hay.contains(h))
}

/// Reclassify diagram code blocks and diagram images into diagrams.
/// Pure post-process over already-extracted vectors: no DOM walk, no char budget.
/// Per-vector caps are DoS guards, not content budgets.
pub(crate) fn classify_diagrams(
    code_blocks: &[CodeBlockEntity],
    images: &[ImageInfo],
) -> Vec<DiagramEntity> {
    let mut out = Vec::new();
    for cb in code_blocks {
        let is_diagram = cb
            .language
            .as_deref()
            .map(|l| l.to_lowercase())
            .is_some_and(|l| DIAGRAM_LANGS.contains(&l.as_str()));
        if is_diagram {
            out.push(DiagramEntity {
                kind: cb.language.clone().unwrap_or_default(),
                caption: None,
                source: cb.code.clone(),
            });
        }
        if out.len() >= 50 {
            break;
        }
    }
    for img in images {
        if is_diagram_image(img.alt.as_deref().unwrap_or(""), &img.url) {
            out.push(DiagramEntity {
                kind: "image".to_string(),
                caption: img.alt.clone(),
                source: img.url.clone(),
            });
        }
        if out.len() >= 50 {
            break;
        }
    }
    out
}

/// Headings that mark procedural content for step extraction.
fn is_howto_heading(text: &str) -> bool {
    let lowered = text.to_lowercase();
    lowered.contains("how") || lowered.contains("step") || lowered.contains("guide") || lowered.contains("tutorial") || lowered.contains("instruction")
}

/// Callout kind from a blockquote's leading word; defaults to `quote`.
/// Handles GitHub-style admonitions (`[!WARNING] ...`) as well as plain
/// leading words (`Warning: ...`). Unknown shapes stay `quote` so pull-quotes
/// never inflate alert counts (false-positive guard).
fn callout_kind(text: &str) -> &str {
    const KINDS: &[&str] = &["note", "warning", "tip", "important", "caution", "danger"];
    let trimmed = text.trim_start();
    let first = if let Some(bracketed) = trimmed
        .strip_prefix("[!")
        .and_then(|rest| rest.split(']').next())
    {
        bracketed.to_lowercase()
    } else {
        trimmed
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_matches([':', '!', '[', ']', '*'])
            .to_lowercase()
    };
    KINDS
        .iter()
        .find(|k| first == **k)
        .copied()
        .unwrap_or("quote")
}

/// Extract heading-contextual quality entities in a single document-order pass:
/// examples (code + nearest preceding heading), FAQs (question heading + first
/// following paragraph), procedural steps (ordered lists under how-to headings),
/// callouts (blockquotes). Caps are DoS guards, not content budgets.
pub(crate) fn extract_quality_entities(
    doc: &Html,
) -> (
    Vec<ExampleEntity>,
    Vec<FaqEntity>,
    Vec<StepEntity>,
    Vec<CalloutEntity>,
) {
    let mut examples = Vec::new();
    let mut faqs = Vec::new();
    let mut steps = Vec::new();
    let mut callouts = Vec::new();

    let Ok(sel) = Selector::parse("h1, h2, h3, h4, h5, h6, pre, blockquote, ol, p") else {
        return (examples, faqs, steps, callouts);
    };
    let code_sel = Selector::parse("code").ok();

    let mut last_heading: Option<String> = None;
    let mut pending_faq: Option<String> = None;

    for elem in doc.select(&sel) {
        match elem.value().name() {
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                let text = elem.text().collect::<Vec<_>>().join(" ").trim().to_string();
                if text.is_empty() {
                    continue;
                }
                if text.ends_with('?') {
                    pending_faq = Some(text.clone());
                } else {
                    pending_faq = None;
                }
                last_heading = Some(text);
            }
            "pre" => {
                let code_child = code_sel
                    .as_ref()
                    .and_then(|sel| elem.select(sel).next());
                let target = code_child.unwrap_or(elem);
                let code = target.text().collect::<Vec<_>>().join("");
                let trimmed = code.trim();
                if trimmed.is_empty() || trimmed.len() < 10 {
                    continue;
                }
                if examples.len() >= 50 {
                    continue;
                }
                examples.push(ExampleEntity {
                    context_heading: last_heading.clone(),
                    language: block_language(target, elem),
                    code: trimmed.to_string(),
                });
            }
            "blockquote" => {
                let text = elem.text().collect::<Vec<_>>().join(" ").trim().to_string();
                if text.is_empty() || callouts.len() >= 50 {
                    continue;
                }
                callouts.push(CalloutEntity {
                    kind: callout_kind(&text).to_string(),
                    text,
                });
            }
            "ol" => {
                if steps.len() >= 100 {
                    continue;
                }
                let howto = last_heading.as_deref().is_some_and(is_howto_heading);
                if !howto {
                    continue;
                }
                if let Ok(li_sel) = Selector::parse("li") {
                    for li in elem.select(&li_sel) {
                        let title =
                            li.text().collect::<Vec<_>>().join(" ").trim().to_string();
                        if title.is_empty() || steps.len() >= 100 {
                            continue;
                        }
                        steps.push(StepEntity {
                            position: (steps.len() + 1) as u32,
                            context_heading: last_heading.clone(),
                            title,
                        });
                    }
                }
            }
            "p" => {
                if let Some(question) = pending_faq.take() {
                    // Length floors filter rhetorical fragments and empty
                    // answers (false-positive guard); multi-paragraph answers
                    // keep their first paragraph by design (recall trade-off).
                    if question.len() < 8 {
                        continue;
                    }
                    let answer =
                        elem.text().collect::<Vec<_>>().join(" ").trim().to_string();
                    if answer.len() >= 10 && faqs.len() < 50 {
                        faqs.push(FaqEntity { question, answer });
                    }
                }
            }
            _ => {}
        }
    }

    (examples, faqs, steps, callouts)
}
