use scraper::Selector;
use scraper::Html;
use crate::schema::content::{CodeBlockEntity, HeadingEntity, TableEntity};

fn block_language(elem: scraper::ElementRef<'_>, fallback: scraper::ElementRef<'_>) -> Option<String> {
    elem.value()
        .attr("class")
        .or_else(|| fallback.value().attr("class"))
        .and_then(|classes| {
            classes.split_whitespace().find_map(|c| {
                c.strip_prefix("language-")
                    .or_else(|| c.strip_prefix("lang-"))
                    .map(String::from)
            })
        })
}

/// True when a code span is substantial enough to be a real example rather
/// than inline formatting or a stray fragment.
fn is_code_candidate(text: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.len() < 15 {
        return false;
    }
    let lines = trimmed.lines().count();
    let alpha = trimmed.chars().filter(|c| c.is_alphanumeric()).count();
    let chars = trimmed.chars().count().max(1);
    // Real code: multi-line, or a single line with real structure and letters.
    (lines >= 2 && alpha > 8) || (lines == 1 && alpha as f64 / chars as f64 > 0.55 && alpha >= 7)
}

/// Normalization key used to de-duplicate `<pre><code>` vs bare `<code>`
/// matches for the same source.
fn normalized_code_key(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ").to_ascii_lowercase()
}

/// Extract structured technical entities from the DOM AST (code blocks, tables, headings).
pub(crate) fn extract_dom_entities(doc: &Html) -> (Vec<CodeBlockEntity>, Vec<TableEntity>, Vec<HeadingEntity>) {
    let mut code_blocks = Vec::new();
    let mut tables = Vec::new();
    let mut headings = Vec::new();

    // 1. Extract code blocks with syntax languages.
    //
    // Two shapes must be handled: `<pre><code>` (rustdoc, GitHub) and bare
    // `<code>` (docs.rs renders examples as plain `<code>` elements). Scanning
    // only `<pre>` silently dropped every example on docs.rs — the page reports
    // zero code blocks despite containing dozens.
    let mut seen_code: Vec<String> = Vec::new();

    if let Ok(pre_sel) = Selector::parse("pre") {
        let code_child_sel = Selector::parse("code").ok();
        for elem in doc.select(&pre_sel) {
            let code_child = code_child_sel.as_ref().and_then(|sel| elem.select(sel).next());
            let target_elem = code_child.unwrap_or(elem);
            let code_text = target_elem.text().collect::<Vec<_>>().join("");
            let trimmed = code_text.trim();
            if !is_code_candidate(trimmed) {
                continue;
            }
            let language = block_language(target_elem, elem);
            seen_code.push(normalized_code_key(trimmed));
            let line_count = trimmed.lines().count();
            code_blocks.push(CodeBlockEntity {
                language,
                code: trimmed.to_string(),
                line_count,
            });
            if code_blocks.len() >= 50 {
                return (code_blocks, tables, headings);
            }
        }
    }

    // Bare `<code>` blocks (docs.rs style). Keep only block-like ones: inline
    // code is a single short token, real examples are multi-line.
    if code_blocks.len() < 50
        && let Ok(code_sel) = Selector::parse("code") {
            for elem in doc.select(&code_sel) {
                let code_text = elem.text().collect::<Vec<_>>().join("");
                let trimmed = code_text.trim();
                if !is_code_candidate(trimmed) {
                    continue;
                }
                let key = normalized_code_key(trimmed);
                if seen_code.contains(&key) {
                    continue;
                }
                seen_code.push(key);
                let line_count = trimmed.lines().count();
                code_blocks.push(CodeBlockEntity {
                    language: block_language(elem, elem),
                    code: trimmed.to_string(),
                    line_count,
                });
                if code_blocks.len() >= 50 {
                    break;
                }
            }
        }

    // 2. Extract tables
    if let (Ok(table_sel), Ok(th_sel), Ok(tr_sel), Ok(td_sel), Ok(caption_sel)) = (
        Selector::parse("table"),
        Selector::parse("th"),
        Selector::parse("tr"),
        Selector::parse("td"),
        Selector::parse("caption"),
    ) {
        for table in doc.select(&table_sel) {
            let caption = table
                .select(&caption_sel)
                .next()
                .map(|c| c.text().collect::<Vec<_>>().join(" ").trim().to_string());
            let headers: Vec<String> = table
                .select(&th_sel)
                .map(|th| th.text().collect::<Vec<_>>().join(" ").trim().to_string())
                .filter(|h| !h.is_empty())
                .collect();
            let mut rows = Vec::new();
            for row in table.select(&tr_sel) {
                let cells: Vec<String> = row
                    .select(&td_sel)
                    .map(|td| td.text().collect::<Vec<_>>().join(" ").trim().to_string())
                    .collect();
                if !cells.is_empty() {
                    rows.push(cells);
                }
                if rows.len() >= 100 {
                    break;
                }
            }
            if !headers.is_empty() || !rows.is_empty() {
                tables.push(TableEntity {
                    caption,
                    headers,
                    rows,
                });
            }
            if tables.len() >= 20 {
                break;
            }
        }
    }

    // 3. Extract document outline hierarchy
    if let Ok(heading_sel) = Selector::parse("h1, h2, h3, h4, h5, h6") {
        for h in doc.select(&heading_sel) {
            let tag_name = h.value().name();
            let level = match tag_name {
                "h1" => 1,
                "h2" => 2,
                "h3" => 3,
                "h4" => 4,
                "h5" => 5,
                "h6" => 6,
                _ => 2,
            };
            let text = h.text().collect::<Vec<_>>().join(" ").trim().to_string();
            if !text.is_empty() {
                let anchor = h.value().attr("id").or_else(|| h.value().attr("name")).map(String::from);
                headings.push(HeadingEntity {
                    level,
                    text,
                    anchor,
                });
            }
            if headings.len() >= 100 {
                break;
            }
        }
    }

    (code_blocks, tables, headings)
}
