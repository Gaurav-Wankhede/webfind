import * as fs from "fs";

let tp = fs.readFileSync("src/engine/fetcher/text_process.rs", "utf8");
const fnStart = tp.indexOf("pub(crate) fn clean_html_streaming");
const fnEnd = tp.indexOf("pub(crate) fn content_score");

const cleanStreamingCode = `pub(crate) fn clean_html_streaming(html: &str) -> String {
    let handlers: Vec<lol_html::ElementContentHandlers> = vec![
        (
            std::borrow::Cow::Borrowed("script, style, noscript, svg, nav, footer, header"),
            lol_html::ElementContentHandler::default()
                .element(|el| {
                    el.remove();
                    Ok(())
                }),
        ),
    ];

    let settings = lol_html::RewriteStrSettings {
        element_content_handlers: handlers,
        ..Default::default()
    };

    lol_html::rewrite_str(html, settings).unwrap_or_else(|_| html.to_owned())
}

`;

tp = tp.slice(0, fnStart) + cleanStreamingCode + tp.slice(fnEnd);
fs.writeFileSync("src/engine/fetcher/text_process.rs", tp, "utf8");
console.log("Replaced clean_html_streaming with exact struct literal");
