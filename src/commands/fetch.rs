use webfind::cli::FetchMode;
use webfind::engine::device_profile::{SessionManager, StickySessions};
use webfind::engine::fetcher::{Fetcher, RotateUserAgent};
use webfind::engine::proxy_pool::ProxyPool;
use webfind::engine::util::split_comma;
use webfind::render::{BoxFormat, MarkdownFormat, RenderContext, RenderFormat};
use webfind::schema::content::StructuredContent;
use webfind::schema::request::OutputFormat;

fn print_fetch_report(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    let ctx = RenderContext::from_content(content, content.excerpt.clone());
    let renderer = BoxFormat;
    let output = renderer.render(&ctx, show_links, show_keywords);
    print!("{}", output);
}

fn print_fetch_markdown(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    let ctx = RenderContext::from_content(content, content.excerpt.clone());
    let renderer = MarkdownFormat;
    let output = renderer.render(&ctx, show_links, show_keywords);
    print!("{}", output);
}

/// Rough token estimate (chars / 4). Informational only, never a cut.
fn token_estimate(chars: usize) -> usize {
    chars / 4
}

/// Compact body: the extractor-produced excerpt, no hardcoded budget.
/// Falls back to the full markdown when no excerpt was extracted.
fn excerpt_first(content: &StructuredContent) -> StructuredContent {
    let mut shaped = content.clone();
    if !shaped.excerpt.is_empty() {
        shaped.content_markdown = shaped.excerpt.clone();
    }
    shaped
}

/// Shared relevance vocabulary: page keywords weighted by TF-IDF, headings
/// at 1.0, plus query words at 1.0 when a question is present.
fn build_terms(content: &StructuredContent, query: Option<&str>) -> Vec<(String, f64)> {
    let mut terms: Vec<(String, f64)> = content
        .keywords
        .iter()
        .map(|k| (k.text.to_lowercase(), k.tfidf_score.max(0.0)))
        .filter(|(t, _)| t.len() >= 3)
        .collect();
    terms.extend(
        content
            .entities
            .headings
            .iter()
            .map(|h| (h.text.to_lowercase(), 1.0))
            .filter(|(t, _)| t.len() >= 3),
    );
    if let Some(q) = query {
        terms.extend(
            q.split_whitespace()
                .map(|w| (w.to_lowercase(), 1.0))
                .filter(|(t, _)| t.len() >= 3),
        );
    }
    terms
}

/// Weighted overlap of a text against the shared vocabulary.
fn overlap_score(text: &str, terms: &[(String, f64)]) -> f64 {
    let lowered = text.to_lowercase();
    terms
        .iter()
        .filter(|(t, _)| lowered.contains(t))
        .map(|(_, w)| w)
        .sum()
}

/// Relevance-ranked extractive body: keep sentences overlapping the query,
/// keyword, heading, and entity terms, preserving document order.
/// Keyword terms carry their TF-IDF weight (best practice from main-content
/// extraction benchmarks); query/heading terms weight 1.0. Sentences scoring
/// above zero are kept, so output size follows relevance, not a char budget.
/// Falls back to the excerpt on zero overlap.
fn relevant_body(content: &StructuredContent, query: Option<&str>) -> String {
    let terms = build_terms(content, query);

    let sentences: Vec<&str> = content
        .content_markdown
        .split_inclusive(['.', '!', '?', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();

    let mut kept = String::new();
    for sentence in sentences {
        if overlap_score(sentence, &terms) > 0.0 {
            if !kept.is_empty() {
                kept.push('\n');
            }
            kept.push_str(sentence);
        }
    }
    if kept.is_empty() {
        content.excerpt.clone()
    } else {
        kept
    }
}

/// Semantic entity projection: which entities travel from raw extraction to
/// LLM input. Headings always pass (the document map). Every other slot keeps
/// items overlapping the shared vocabulary; zero-overlap items are bloat for
/// this question, not signal. Size follows relevance, never a char budget.
fn project_entities(
    content: &StructuredContent,
    query: Option<&str>,
) -> webfind::schema::content::Entities {
    use webfind::schema::content::Entities;
    let terms = build_terms(content, query);
    let keep = |text: &str| overlap_score(text, &terms) > 0.0;
    let e = &content.entities;
    Entities {
        code_blocks: e
            .code_blocks
            .iter()
            .filter(|b| {
                keep(&b.code)
                    || b.language.as_deref().is_some_and(|l| {
                        query
                            .unwrap_or_default()
                            .to_lowercase()
                            .contains(&l.to_lowercase())
                    })
            })
            .cloned()
            .collect(),
        tables: e
            .tables
            .iter()
            .filter(|t| {
                keep(&t.headers.join(" "))
                    || t.rows
                        .iter()
                        .flatten()
                        .any(|c| overlap_score(c, &terms) > 0.0)
                    || t.caption.as_deref().is_some_and(keep)
            })
            .cloned()
            .collect(),
        headings: e.headings.clone(),
        diagrams: e
            .diagrams
            .iter()
            .filter(|d| keep(&d.kind) || d.caption.as_deref().is_some_and(keep))
            .cloned()
            .collect(),
        examples: e
            .examples
            .iter()
            .filter(|x| {
                keep(&x.code) || x.context_heading.as_deref().is_some_and(keep)
            })
            .cloned()
            .collect(),
        faqs: e.faqs.clone(),
        steps: e.steps.clone(),
        maths: e.maths.clone(),
        callouts: e.callouts.clone(),
        package_versions: e.package_versions.clone(),
        licenses: e.licenses.clone(),
        repository_urls: e.repository_urls.clone(),
        emails: e.emails.iter().filter(|s| keep(s)).cloned().collect(),
        phones: e.phones.iter().filter(|s| keep(s)).cloned().collect(),
        addresses: e.addresses.iter().filter(|s| keep(s)).cloned().collect(),
        urls: e.urls.iter().filter(|s| keep(s)).cloned().collect(),
        prices: e.prices.iter().filter(|s| keep(s)).cloned().collect(),
        dates: e.dates.iter().filter(|s| keep(s)).cloned().collect(),
        ip_addresses: e
            .ip_addresses
            .iter()
            .filter(|s| keep(s))
            .cloned()
            .collect(),
        social_handles: e
            .social_handles
            .iter()
            .filter(|s| keep(s))
            .cloned()
            .collect(),
    }
}

/// Intent-based auto decision: a present query means the question needs
/// precise context (Relevant); otherwise map first, details on demand.
/// Decided by invocation intent, never by size thresholds.
fn resolve_mode(mode: FetchMode, query: Option<&str>) -> FetchMode {
    match mode {
        FetchMode::Auto if query.is_some() => FetchMode::Relevant,
        FetchMode::Auto => FetchMode::Compact,
        m => m,
    }
}

/// One-line budget witness printed ahead of shaped (non-full) bodies.
fn print_budget(mode: &FetchMode, body_chars: usize) {
    println!(
        "Budget: mode={:?} ~{} tokens ({} chars)",
        mode,
        token_estimate(body_chars),
        body_chars
    );
}

#[allow(clippy::too_many_arguments)]
pub async fn run(
    url: String,
    urls: Vec<String>,
    output: webfind::cli::OutputArg,
    compact: bool,
    mode: FetchMode,
    query: Option<String>,
    extract_links: bool,
    extract_keywords: bool,
    dynamic: bool,
    dynamic_wait_ms: u64,
    proxies: Option<String>,
) -> anyhow::Result<()> {
    use webfind::engine::pipeline::FetchPipeline;

    let mut all_urls = vec![url];
    let mut urls = urls;
    all_urls.append(&mut urls);
    all_urls.retain(|u| !u.is_empty());

    let proxy_pool = if let Some(ref list) = proxies {
        Some(ProxyPool::from_list(&split_comma(list))?)
    } else {
        None
    };
    let session_manager = if proxies.is_some() {
        Some(SessionManager::new(StickySessions::Sticky))
    } else {
        None
    };
    let mut fetcher = if proxies.is_some() {
        Fetcher::new_human(proxy_pool, session_manager, RotateUserAgent::Rotate, 1)?
    } else {
        Fetcher::new()?
    };
    if dynamic {
        fetcher = fetcher.with_dynamic_fallback(dynamic_wait_ms);
    }

    let output_enum: OutputFormat = output.into();

    // Intent-based auto decision: a present query means the question needs
    // precise context (Relevant); otherwise map first, details on demand.
    // No size thresholds anywhere in this decision.
    let mode = resolve_mode(mode, query.as_deref());

    if all_urls.len() == 1 {
        let content = fetcher.fetch_url(&all_urls[0]).await?;
        match output_enum {
            OutputFormat::Json => {
                let json = if compact || matches!(mode, FetchMode::Compact) {
                    let mut shaped = excerpt_first(&content);
                    shaped.entities =
                        project_entities(&content, query.as_deref());
                    serde_json::to_string_pretty(&shaped.to_compact())?
                } else if matches!(mode, FetchMode::Relevant) {
                    let mut slim = content.to_compact();
                    slim.content_markdown = relevant_body(&content, query.as_deref());
                    slim.entities = project_entities(&content, query.as_deref());
                    serde_json::to_string_pretty(&slim)?
                } else {
                    serde_json::to_string_pretty(&content)?
                };
                println!("{}", json);
            }
            OutputFormat::Report => {
                if matches!(mode, FetchMode::Full) {
                    print_fetch_report(&content, extract_links, extract_keywords);
                } else {
                    let shaped = match mode {
                        FetchMode::Compact => excerpt_first(&content),
                        _ => {
                            let mut shaped = content.clone();
                            shaped.content_markdown =
                                relevant_body(&content, query.as_deref());
                            shaped
                        }
                    };
                    print_budget(&mode, shaped.content_markdown.len());
                    print_fetch_report(&shaped, extract_links, extract_keywords);
                }
            }
            OutputFormat::Markdown => {
                if matches!(mode, FetchMode::Full) {
                    print_fetch_markdown(&content, extract_links, extract_keywords);
                } else {
                    let shaped = match mode {
                        FetchMode::Compact => excerpt_first(&content),
                        _ => {
                            let mut shaped = content.clone();
                            shaped.content_markdown =
                                relevant_body(&content, query.as_deref());
                            shaped
                        }
                    };
                    print_budget(&mode, shaped.content_markdown.len());
                    print_fetch_markdown(&shaped, extract_links, extract_keywords);
                }
            }
        }
    } else {
        let pipeline = FetchPipeline::new(fetcher);
        let results = pipeline.fetch_all(&all_urls).await;
        let valid = FetchPipeline::filter_valid(results.clone());
        let (success, failure, errors) = FetchPipeline::summarize(&results);

        let shaped: Vec<StructuredContent> = match mode {
            FetchMode::Full | FetchMode::Auto => valid.clone(),
            FetchMode::Compact => valid.iter().map(excerpt_first).collect(),
            FetchMode::Relevant => valid
                .iter()
                .map(|c| {
                    let mut item = c.clone();
                    item.content_markdown = relevant_body(c, query.as_deref());
                    item
                })
                .collect(),
        };
        // Semantic projection for shaped modes: headings always travel,
        // overlapping entities travel, zero-overlap noise stays behind.
        let shaped: Vec<StructuredContent> = if matches!(mode, FetchMode::Full) {
            shaped
        } else {
            shaped
                .iter()
                .map(|c| {
                    let mut item = c.clone();
                    item.entities = project_entities(c, query.as_deref());
                    item
                })
                .collect()
        };

        match output_enum {
            OutputFormat::Json => {
                let json = if compact || matches!(mode, FetchMode::Compact) {
                    let compact_valid: Vec<_> =
                        shaped.iter().map(|c| c.to_compact()).collect();
                    serde_json::to_string_pretty(&compact_valid)?
                } else if matches!(mode, FetchMode::Relevant) {
                    let slim_valid: Vec<_> = shaped
                        .iter()
                        .map(|c| {
                            let mut slim = c.to_compact();
                            slim.content_markdown = relevant_body(c, query.as_deref());
                            slim
                        })
                        .collect();
                    serde_json::to_string_pretty(&slim_valid)?
                } else {
                    serde_json::to_string_pretty(&shaped)?
                };
                println!("{}", json);
            }
            OutputFormat::Markdown => {
                for content in &shaped {
                    if !matches!(mode, FetchMode::Full) {
                        print_budget(&mode, content.content_markdown.len());
                    }
                    print_fetch_markdown(content, extract_links, extract_keywords);
                    println!("\n---\n");
                }
            }
            OutputFormat::Report => {
                println!(
                    "Fetched {} URLs: {} success, {} failure",
                    all_urls.len(),
                    success,
                    failure
                );
                for (url, err) in errors {
                    println!("  FAIL {}: {}", url, err);
                }
                for content in &shaped {
                    if !matches!(mode, FetchMode::Full) {
                        print_budget(&mode, content.content_markdown.len());
                    }
                    print_fetch_report(content, extract_links, extract_keywords);
                    println!();
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use webfind::schema::content::Entities;
    use webfind::schema::response::Keyword;

    fn fixture(keyword: &str) -> StructuredContent {
        StructuredContent {
            url: "https://example.com".into(),
            final_url: "https://example.com".into(),
            status_code: 200,
            title: "Fixture".into(),
            description: None,
            canonical_url: None,
            language: "en".into(),
            language_confidence: 1.0,
            published_at: None,
            modified_at: None,
            author: None,
            site_name: None,
            content_text: "full text".into(),
            content_html: "<p>full text</p>".into(),
            content_markdown: format!(
                "Rust ownership moves values.\nUnrelated weather report.\n{} borrow checker rules.",
                keyword
            ),
            excerpt: "short excerpt".into(),
            word_count: 12,
            char_count: 90,
            sentence_count: 3,
            reading_time_seconds: 1,
            reading_ease: 70.0,
            grade_level: 8.0,
            keywords: vec![Keyword {
                text: keyword.into(),
                tfidf_score: 9.0,
                rank: 1,
            }],
            open_graph: None,
            twitter_card: None,
            json_ld: vec![],
            schema_type: None,
            images: vec![],
            internal_links: vec![],
            external_links: vec![],
            favicon: None,
            rss_url: None,
            normalized_text: "full text".into(),
            fetched_at: chrono::Utc::now(),
            fetch_duration_ms: 1,
            html_size_bytes: 64,
            encoding: None,
            ssl_valid: true,
            redirect_count: 0,
            content_type: "text/html".into(),
            content_type_header: "text/html".into(),
            is_paywalled: false,
            is_valid_content: true,
            entities: Entities::default(),
        }
    }

    #[test]
    fn compact_uses_excerpt_without_char_budget() {
        let content = fixture("borrow");
        let shaped = excerpt_first(&content);
        assert_eq!(shaped.content_markdown, "short excerpt");
        assert_eq!(shaped.title, "Fixture");
        let slim = shaped.to_compact();
        assert_eq!(slim.content_markdown, "short excerpt");
    }

    #[test]
    fn relevant_keeps_overlapping_sentences_in_order() {
        let content = fixture("borrow");
        let body = relevant_body(&content, Some("borrow checker"));
        assert!(body.contains("borrow checker rules"));
        assert!(!body.contains("weather report"));
        // Sentence without any term overlap is dropped, order preserved.
        assert!(!body.contains("ownership moves values"));
    }

    #[test]
    fn relevant_falls_back_to_excerpt_on_zero_overlap() {
        let mut content = fixture("borrow");
        content.content_markdown = "Clouds drift slowly.\nRain patters down.".into();
        let body = relevant_body(&content, Some("zzzqqq"));
        assert_eq!(body, "short excerpt");
    }

    #[test]
    fn relevant_weights_keywords_by_tfidf() {
        let mut content = fixture("borrow");
        content.keywords.push(Keyword {
            text: "weather".into(),
            tfidf_score: 0.0,
            rank: 2,
        });
        // "weather report" scores 0.0 (zero-weight term only) and is dropped,
        // while the tfidf-weighted "borrow" sentence is kept.
        let body = relevant_body(&content, None);
        assert!(body.contains("borrow checker rules"));
        assert!(!body.contains("weather report"));
    }

    #[test]
    fn token_estimate_is_chars_over_four() {
        assert_eq!(token_estimate(4000), 1000);
        assert_eq!(token_estimate(28430), 7107);
    }

    #[test]
    fn auto_resolves_by_intent_not_size() {
        assert!(matches!(
            resolve_mode(FetchMode::Auto, Some("how to install")),
            FetchMode::Relevant
        ));
        assert!(matches!(
            resolve_mode(FetchMode::Auto, None),
            FetchMode::Compact
        ));
        assert!(matches!(
            resolve_mode(FetchMode::Full, Some("q")),
            FetchMode::Full
        ));
    }

    #[test]
    fn projection_keeps_map_and_overlap_drops_noise() {
        use webfind::schema::content::HeadingEntity;
        let mut content = fixture("borrow");
        content.entities.headings = vec![HeadingEntity {
            level: 2,
            text: "Borrow checker guide".into(),
            anchor: None,
        }];
        content.entities.emails = vec![
            "borrow@docs.io".into(),
            "spam@telemarketer.example".into(),
        ];
        let projected = project_entities(&content, Some("how to borrow"));
        assert_eq!(projected.headings.len(), 1, "map always travels");
        assert!(
            projected.emails.iter().any(|e| e == "borrow@docs.io"),
            "overlapping contact travels"
        );
        assert!(
            !projected.emails.iter().any(|e| e.contains("telemarketer")),
            "zero-overlap contact stays behind"
        );
    }
}
