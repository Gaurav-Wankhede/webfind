use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;
use webfind::cli::{Cli, Commands};

// Global allocator (P4.5): mimalloc improves p99 latency for the concurrent
// fetch/index workload vs. the system allocator. Must be declared before main.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod commands;
use webfind::engine::bulk_crawler::BulkDomainCrawler;
use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph, TraversalDirection};
use webfind::engine::crawler::Crawler;
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::fetcher::Fetcher;
use webfind::engine::graph_summary::build_graph_summary;
use webfind::engine::indexer::attach_content;
use webfind::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};
use webfind::report::format_response;
use webfind::schema::content::StructuredContent;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::SearchResponse;

// Embedded skills reference for AI agents (progressive disclosure)
const SKILLS_MD: &str = include_str!("../webfind/SKILL.md");

fn index_path() -> std::path::PathBuf {
    webfind::config::data_dir().join("index_data")
}

fn print_fetch_report(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    let status = if content.is_valid_content {
        "OK"
    } else {
        "EXTRACTION_FAILED"
    };
    println!("┌─────────────────────────────────────────────────────────────┐");
    println!("│                    WEBFIND FETCH REPORT                    │");
    println!("├─────────────────────────────────────────────────────────────┤");
    println!("│ Status:       {:<44} │", status);
    println!("│ URL:          {:<44} │", truncate(&content.url, 44));
    println!("│ Title:        {:<44} │", truncate(&content.title, 44));
    println!(
        "│ Language:     {:<44} │",
        format!(
            "{} ({:.0}%)",
            content.language,
            content.language_confidence * 100.0
        )
    );
    println!("│ Words:        {:<44} │", content.word_count);
    println!(
        "│ Reading time: {:<44} │",
        format!("{}s", content.reading_time_seconds)
    );
    println!(
        "│ Reading ease: {:<44} │",
        format!("{:.1}", content.reading_ease)
    );
    println!(
        "│ Grade level:  {:<44} │",
        format!("{:.1}", content.grade_level)
    );
    println!(
        "│ SSL:          {:<44} │",
        if content.ssl_valid {
            "valid"
        } else {
            "invalid"
        }
    );
    println!(
        "│ Paywalled:    {:<44} │",
        if content.is_paywalled { "yes" } else { "no" }
    );
    println!(
        "│ Fetched in:   {:<44} │",
        format!("{}ms", content.fetch_duration_ms)
    );

    if let Some(ref author) = content.author {
        println!("│ Author:       {:<44} │", truncate(author, 44));
    }
    if let Some(ref published) = content.published_at {
        println!(
            "│ Published:    {:<44} │",
            published.format("%Y-%m-%d %H:%M UTC")
        );
    }

    println!("├─────────────────────────────────────────────────────────────┤");
    println!("│ EXCERPT                                                   │");
    println!("├─────────────────────────────────────────────────────────────┤");
    for line in word_wrap(&content.excerpt, 59) {
        println!("│ {:<59} │", line);
    }

    if show_keywords && !content.keywords.is_empty() {
        println!("├─────────────────────────────────────────────────────────────┤");
        println!("│ KEYWORDS                                                  │");
        println!("├─────────────────────────────────────────────────────────────┤");
        let kw_str: Vec<String> = content
            .keywords
            .iter()
            .take(10)
            .map(|k| format!("{} ({:.1})", k.text, k.tfidf_score))
            .collect();
        for line in word_wrap(&kw_str.join(", "), 59) {
            println!("│ {:<59} │", line);
        }
    }

    if show_links {
        if !content.internal_links.is_empty() {
            println!("├─────────────────────────────────────────────────────────────┤");
            println!("│ INTERNAL LINKS ({:<44}) │", content.internal_links.len());
            println!("├─────────────────────────────────────────────────────────────┤");
            for link in content.internal_links.iter().take(10) {
                println!("│  {:<58} │", truncate(link, 58));
            }
        }
        if !content.external_links.is_empty() {
            println!("├─────────────────────────────────────────────────────────────┤");
            println!("│ EXTERNAL LINKS ({:<44}) │", content.external_links.len());
            println!("├─────────────────────────────────────────────────────────────┤");
            for link in content.external_links.iter().take(10) {
                println!("│  {:<58} │", truncate(link, 58));
            }
        }
    }

    println!("└─────────────────────────────────────────────────────────────┘");
}

fn print_fetch_markdown(content: &StructuredContent, show_links: bool, show_keywords: bool) {
    println!("# {}", content.title);
    println!();
    println!("**URL:** {}", content.url);
    if let Some(ref author) = content.author {
        println!("**Author:** {}", author);
    }
    if let Some(ref published) = content.published_at {
        println!("**Published:** {}", published.format("%Y-%m-%d %H:%M UTC"));
    }
    println!(
        "**Language:** {} ({:.0}%)",
        content.language,
        content.language_confidence * 100.0
    );
    println!(
        "**Reading time:** {}s | **Words:** {} | **Grade:** {:.1}",
        content.reading_time_seconds, content.word_count, content.grade_level
    );
    println!();

    if !content.excerpt.is_empty() {
        println!("> {}", content.excerpt);
        println!();
    }

    if show_keywords && !content.keywords.is_empty() {
        println!("## Keywords");
        for kw in content.keywords.iter().take(10) {
            println!("- {} (score: {:.1})", kw.text, kw.tfidf_score);
        }
        println!();
    }

    if show_links {
        if !content.internal_links.is_empty() {
            println!("## Internal Links ({})", content.internal_links.len());
            for link in content.internal_links.iter().take(20) {
                println!("- {}", link);
            }
            println!();
        }
        if !content.external_links.is_empty() {
            println!("## External Links ({})", content.external_links.len());
            for link in content.external_links.iter().take(20) {
                println!("- {}", link);
            }
            println!();
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}

fn word_wrap(text: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.len() + word.len() + 1 > width && !current.is_empty() {
            lines.push(current.clone());
            current.clear();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Check for WEBFIND_PRINT_SKILLS=1 early (before CLI parsing)
    if std::env::var("WEBFIND_PRINT_SKILLS").is_ok() {
        print!("{}", SKILLS_MD);
        return Ok(());
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cfg = webfind::config::load().unwrap_or_default();

    let cli = Cli::parse();

    // Handle --print-skills flag (also triggered by WEBFIND_PRINT_SKILLS=1)
    if cli.print_skills {
        print!("{}", SKILLS_MD);
        return Ok(());
    }

    let Some(command) = cli.command else {
        // No subcommand provided and no --print-skills: show help
        use clap::CommandFactory;
        Cli::command().print_help().unwrap();
        println!();
        return Ok(());
    };

    match command {
        Commands::Search {
            query,
            depth,
            limit,
            output,
            language,
            domains,
            include_content,
            include_graph,
            include_keywords,
            include_metrics,
            graph_store,
            turso_path,
            hybrid,
        } => {
            return commands::search::run(
                &cfg,
                query,
                depth,
                limit,
                output,
                language,
                domains,
                include_content,
                include_graph,
                include_keywords,
                include_metrics,
                graph_store,
                turso_path,
                hybrid,
            )
            .await;
        }

        Commands::Fetch {
            url,
            mut urls,
            output,
            extract_links,
            extract_keywords,
            dynamic,
            dynamic_wait_ms,
            proxies,
        } => {
            let mut all_urls = vec![url];
            all_urls.append(&mut urls);
            all_urls.retain(|u| !u.is_empty());

            let proxy_pool = if let Some(ref list) = proxies {
                let parsed: Vec<String> = list
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                Some(webfind::engine::proxy_pool::ProxyPool::from_list(&parsed)?)
            } else {
                None
            };
            let session_manager = if proxies.is_some() {
                Some(webfind::engine::device_profile::SessionManager::new(
                    webfind::engine::device_profile::StickySessions::Sticky,
                ))
            } else {
                None
            };
            let fetcher = if proxies.is_some() {
                Fetcher::new_human(
                    proxy_pool,
                    session_manager,
                    webfind::engine::fetcher::RotateUserAgent::Rotate,
                    1,
                )?
            } else if dynamic {
                Fetcher::new()?.with_dynamic_fallback(dynamic_wait_ms)
            } else {
                Fetcher::new()?
            };

            if all_urls.len() == 1 {
                let content = fetcher.fetch_url(&all_urls[0]).await?;
                let output_enum = match output {
                    webfind::cli::OutputArg::Json => OutputFormat::Json,
                    webfind::cli::OutputArg::Report => OutputFormat::Report,
                    webfind::cli::OutputArg::Markdown => OutputFormat::Markdown,
                };
                match output_enum {
                    OutputFormat::Json => {
                        let json = serde_json::to_string_pretty(&content)?;
                        print!("{}", json);
                    }
                    OutputFormat::Report => {
                        print_fetch_report(&content, extract_links, extract_keywords);
                    }
                    OutputFormat::Markdown => {
                        print_fetch_markdown(&content, extract_links, extract_keywords);
                    }
                }
            } else {
                let pipeline = webfind::engine::pipeline::FetchPipeline::new(fetcher);
                let results = pipeline.fetch_all(&all_urls).await;
                let valid = webfind::engine::pipeline::FetchPipeline::filter_valid(results.clone());
                let (success, failure, errors) =
                    webfind::engine::pipeline::FetchPipeline::summarize(&results);

                println!(
                    "Fetched {} URLs: {} success, {} failure",
                    all_urls.len(),
                    success,
                    failure
                );
                for (url, err) in errors {
                    println!("  FAIL {}: {}", url, err);
                }
                for content in &valid {
                    println!("\n--- {} ---", content.url);
                    println!("Title: {}", content.title);
                    println!(
                        "Words: {} | Grade: {:.1}",
                        content.word_count, content.grade_level
                    );
                }
            }

            Ok(())
        }

        Commands::Crawl {
            seed,
            depth: _,
            delay,
            max_pages,
            cache_dir,
            recrawl_policy,
            recrawl_days,
            skip_cached,
            proxies,
            proxy_cidr,
            proxy_protocol,
            rotate_ua,
            sticky_sessions,
            respect_robots,
            rps,
            dynamic,
            dynamic_wait_ms,
            bulk,
            graph_store,
            turso_path,
            pages_per_session,
            session_max_age_minutes,
            hybrid,
            follow_external,
            min_depth,
            max_depth,
            topics,
        } => {
            return commands::crawl::run(
                &cfg,
                seed,
                delay,
                max_pages,
                cache_dir,
                recrawl_policy,
                recrawl_days,
                skip_cached,
                proxies,
                proxy_cidr,
                proxy_protocol,
                rotate_ua,
                sticky_sessions,
                respect_robots,
                rps,
                dynamic,
                dynamic_wait_ms,
                bulk,
                graph_store,
                turso_path,
                pages_per_session,
                session_max_age_minutes,
                hybrid,
                follow_external,
                min_depth,
                max_depth,
                topics,
            )
            .await;
        }

        Commands::Index { action } => {
            return commands::index::run(action).await;
        }

        Commands::Graph {
            url,
            depth,
            direction,
            graph_store,
            turso_path,
        } => {
            return commands::graph::run(&cfg, url, depth, direction, graph_store, turso_path)
                .await;
        }

        Commands::ProxyPool {
            listen,
            cidr,
            source_ips,
        } => {
            return commands::proxy_pool::run(listen, cidr, source_ips).await;
        }

        Commands::Research {
            seed,
            query,
            depth: _,
            max_pages,
            delay,
            respect_robots,
            proxies,
            hybrid,
            limit,
            include_graph,
            include_content,
            follow_external,
            min_depth,
            max_depth,
            topics,
            seeds,
        } => {
            return commands::research::run(
                seed,
                query,
                max_pages,
                delay,
                respect_robots,
                proxies,
                hybrid,
                limit,
                include_graph,
                include_content,
                follow_external,
                min_depth,
                max_depth,
                topics,
                seeds,
            )
            .await;
        }

        Commands::Serve {
            mode: _,
            port,
            transport,
            graph_store,
            turso_path,
            hybrid,
            rate_limit,
            gui_port,
        } => {
            return commands::serve::run(
                &cfg,
                port,
                transport,
                graph_store,
                turso_path,
                hybrid,
                rate_limit,
                gui_port,
            )
            .await;
        }

        Commands::Migrate { from, to } => {
            return commands::migrate::run(&cfg, from, to).await;
        }

        Commands::Status => {
            let path = index_path();
            println!("webfind v{}", env!("CARGO_PKG_VERSION"));

            if path.exists() {
                let indexer = Arc::new(InMemorySearchEngine::new());
                match indexer.doc_count().await {
                    Ok(count) => {
                        println!("Index:    {}", path.display());
                        println!("Documents: {}", count);
                        println!("Status:   ready");
                    }
                    Err(e) => {
                        println!("Index:    {} (error: {})", path.display(), e);
                        println!("Status:   index corrupt or missing");
                    }
                }
            } else {
                println!("Index:    not found");
                println!("Status:   no index — run 'webfind crawl' to create one");
            }
            Ok(())
        }
    }
}
