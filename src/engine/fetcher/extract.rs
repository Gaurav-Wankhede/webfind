use chrono::Utc;
use super::*;
use anyhow::Result;
use scraper::Html;
use readability_rust::Readability;
use crate::schema::content::*;

impl Fetcher {
#[allow(clippy::too_many_arguments)]
pub fn extract_from_html(
        &self,
        html: &str,
        original_url: &str,
        final_url: &str,
        status_code: u16,
        ssl_valid: bool,
        fetch_duration_ms: u64,
        last_modified: Option<&str>,
    ) -> Result<StructuredContent> {
        let doc = Html::parse_document(html);

        // 1. Readability extraction
        let article = Readability::new(html, None)
            .ok()
            .and_then(|mut r| r.parse());

        // 1b. Documentation & official site semantic container extraction
        let doc_container = extract_doc_container(&doc);

        // Fallback body text when readability and doc container cannot isolate the article.
        let fallback_text = doc_container
            .as_ref()
            .map(|(_, text)| text.clone())
            .unwrap_or_else(|| fallback_body_text(&doc));

        // 2. Metadata from <head>
        let meta = extract_meta(&doc);

        // 3. OpenGraph
        let open_graph = extract_open_graph(&doc);

        // 4. Twitter Card
        let twitter_card = extract_twitter_card(&doc);

        // 5. JSON-LD
        let json_ld = extract_json_ld(&doc);

        // 6. Links
        let (internal_links, external_links) = extract_links(&doc, final_url);

        // 7. Images
        let images = extract_images(&doc);

        // 8. Title: readability > meta > og > h1 > URL path
        let h1_title = extract_h1(&doc);
        let title = article
            .as_ref()
            .and_then(|a| a.title.clone())
            .or_else(|| meta.title.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.title.clone()))
            .or(h1_title)
            .unwrap_or_else(|| title_from_url(final_url))
            .trim()
            .to_string();

        // 9. Content text: Prefer doc container if it has substantial content, else
        // readability article, else the script/style-stripped fallback.
        //
        // SCRIPT GUARD: readability's `text_content` and several doc-container
        // heuristics include inline <script>/<style> bodies. Left unfiltered,
        // raw JS (`var scripts = [['scripts-QK6UYGYG.js', ...]]`) leaked into
        // content_text, which then poisoned keywords, excerpt, and every
        // readability score. Strip code blocks unconditionally before use.
        let candidate_text = if let Some((_, ref dt)) = doc_container
            && dt.len() >= 100
        {
            dt.clone()
        } else {
            article
                .as_ref()
                .and_then(|a| a.text_content.clone())
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(fallback_text)
        };
        let content_text = strip_code_blocks(&candidate_text);

        // 10. Content HTML — cleaned for AI agent consumption.
        // Density-gated container selection: prefer the HTML candidate with the
        // higher content_score (text_density * ln(text_chars)). Avoids taking
        // the broadest container when readability has extracted a tighter, denser
        // article body — ported from fastcrw readability.rs heuristic.
        let raw_html = {
            let doc_html_owned: Option<String> = if let Some((ref dh, _)) = doc_container
                && !dh.trim().is_empty()
            {
                Some(dh.clone())
            } else {
                None
            };
            let art_html_owned: Option<String> = article
                .as_ref()
                .and_then(|a| a.content.clone());

            match (doc_html_owned, art_html_owned) {
                (Some(d), Some(a)) => {
                    // Pick the denser candidate.
                    if content_score(&a) > content_score(&d) { a } else { d }
                }
                (Some(d), None) => d,
                (None, Some(a)) => a,
                (None, None) => format!("<div>{}</div>", html_escape(&content_text)),
            }
        };
        let mut content_html = clean_content_html(&raw_html);
        if is_empty_shell(&content_html) {
            content_html = text_to_html(&content_text);
        }

        // 11. Excerpt
        // Prefer the page's own description, but ALWAYS fall back to the extracted
        // body text. Without this, any page lacking a meta description (Wikipedia,
        // GitHub, most docs) produced an empty excerpt even though 1,000+ words of
        // real content had been extracted — the report then showed a blank box.
        let excerpt = article
            .as_ref()
            .and_then(|a| a.excerpt.clone())
            .filter(|e| !e.trim().is_empty())
            .or_else(|| meta.description.clone().filter(|e| !e.trim().is_empty()))
            .or_else(|| {
                open_graph
                    .as_ref()
                    .and_then(|og| og.description.clone())
                    .filter(|e| !e.trim().is_empty())
            })
            .unwrap_or_else(|| derive_excerpt(&content_text, 240));

        // 12. Author
        let author = article
            .as_ref()
            .and_then(|a| a.byline.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.article_author.clone()))
            .or_else(|| meta.author.clone());

        // 13. Site name
        let site_name = article
            .as_ref()
            .and_then(|a| a.site_name.clone())
            .or_else(|| open_graph.as_ref().and_then(|og| og.site_name.clone()))
            .or_else(|| extract_site_name(final_url));

        // 14. Language detection
        let (language, language_confidence) = detect_language(&content_text, html);

        // 15. Published date (priority: readability > JSON-LD > og > meta)
        let published_at = article
            .as_ref()
            .and_then(|a| a.published_time.clone())
            .or_else(|| {
                open_graph
                    .as_ref()
                    .and_then(|og| og.article_published_time.clone())
            })
            .or_else(|| meta.published_time.clone())
            .as_deref()
            .and_then(parse_date);

        // 16. Modified date
        let modified_at = open_graph
            .as_ref()
            .and_then(|og| og.article_modified_time.clone())
            .or_else(|| meta.modified_time.clone())
            .or_else(|| meta.updated_time.clone())
            .or_else(|| meta.last_modified.clone())
            .or_else(|| last_modified.map(String::from))
            .as_deref()
            .and_then(parse_date);

        // 17. Content metrics
        let word_count = count_words(&content_text);
        let char_count = content_text.len() as u32;
        let sentence_count = count_sentences(&content_text);
        let reading_time_seconds = estimate_reading_time(word_count);
        let reading_ease = textstat::flesch_reading_ease(&content_text);
        let grade_level = textstat::flesch_kincaid_grade(&content_text);

        // 18. Keywords
        let keywords = extract_page_keywords(&content_text, 15);

        // 19. Normalize text
        let normalized_text = normalize_text(&content_text);

        // 19b. Entity extraction — regex + DOM AST (code blocks, tables, headings).
        let mut entities = extract_entities(&content_text);
        let (code_blocks, tables, headings) = extract_dom_entities(&doc);
        entities.code_blocks = code_blocks;
        entities.tables = tables;
        entities.headings = headings;

        // 19c. Quality entities — heading-contextual examples/FAQs/steps/callouts
        // plus diagram reclassification. Relevance-sized, never char-capped.
        let (examples, faqs, steps, callouts) = extract_quality_entities(&doc);
        entities.examples = examples;
        entities.faqs = faqs;
        entities.steps = steps;
        entities.callouts = callouts;
        entities.diagrams = classify_diagrams(&entities.code_blocks, &images);

        // Pre-compute values before moves
        let content_markdown = html_to_markdown(&content_html);
        let is_valid = !content_text.is_empty() && word_count >= MIN_VALID_WORDS;

        // 20. Schema type from JSON-LD
        let schema_type = json_ld
            .iter()
            .find_map(|v| v.get("@type").and_then(|t| t.as_str()).map(String::from));

        // 21. Paywall detection
        let is_paywalled = detect_paywall(html);

        // 22. Favicon
        let favicon = extract_favicon(&doc);

        // 23. RSS
        let rss_url = extract_rss(&doc);

        // 24. Encoding from content type (if we had headers)
        let encoding: Option<String> = None;

        Ok(StructuredContent {
            url: original_url.to_string(),
            final_url: final_url.to_string(),
            status_code,
            title,
            description: meta.description,
            canonical_url: meta.canonical_url,
            language,
            language_confidence,
            published_at,
            modified_at,
            author,
            site_name,
            content_text,
            content_html,
            content_markdown,
            excerpt,
            word_count,
            char_count,
            sentence_count,
            reading_time_seconds,
            reading_ease,
            grade_level,
            keywords,
            open_graph,
            twitter_card,
            json_ld,
            schema_type,
            images,
            internal_links,
            external_links,
            favicon,
            rss_url,
            normalized_text,
            fetched_at: Utc::now(),
            fetch_duration_ms,
            html_size_bytes: html.len() as u64,
            encoding,
            ssl_valid,
            redirect_count: 0,
            content_type: String::new(),
            content_type_header: String::new(),
            is_paywalled,
            is_valid_content: is_valid,
            entities,
        })
    }
}
