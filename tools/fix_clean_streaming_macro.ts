import * as fs from "fs";

let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
const fnStart = tp.indexOf("pub(crate) fn clean_html_streaming");
const fnEnd = tp.indexOf("pub(crate) fn content_score");

const cleanStreamingCode = `pub(crate) fn clean_html_streaming(html: &str) -> String {
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

`;

tp = tp.slice(0, fnStart) + cleanStreamingCode + tp.slice(fnEnd);
fs.writeFileSync("src/engine/fetcher/text_process.rs", tp, "utf8");
console.log("Rewrote clean_html_streaming with element! macro");
