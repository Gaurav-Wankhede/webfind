use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use clap::Parser;
use tracing_subscriber::EnvFilter;
use webfind::cli::{Cli, Commands, GraphStoreArg};
use webfind::engine::bulk_crawler::BulkDomainCrawler;
use webfind::engine::crawl_graph::{CrawlGraphStore, InMemoryCrawlGraph, TraversalDirection};
use webfind::engine::crawler::Crawler;
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::fetcher::Fetcher;
use webfind::engine::graph_summary::build_graph_summary;
use webfind::engine::indexer::Indexer;
use webfind::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use webfind::engine::ranker::Ranker;
use webfind::engine::search_engine::InMemorySearchEngine;
use webfind::engine::surreal_engine::SurrealSearchEngine;
use webfind::report::format_response;
use webfind::schema::content::StructuredContent;
use webfind::schema::request::{OutputFormat, SearchDepth, SearchRequest};
use webfind::schema::response::SearchResponse;
use webfind::storage::surreal_store::SurrealStore;

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
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cfg = webfind::config::load().unwrap_or_default();

    let cli = Cli::parse();

    match cli.command {
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
            surreal_url,
            surreal_user,
            surreal_pass,
            surreal_ns,
            surreal_db,
            hybrid,
        } => {
            let _ = (&language, &domains);
            let graph_store_name = webfind::config::resolve_graph_store(
                &cfg,
                graph_store.as_ref().map(|g| g.as_str()),
            );
            let graph_store = if graph_store_name == "surrealdb" {
                Some(GraphStoreArg::Surrealdb)
            } else {
                None
            };
            let surreal = webfind::config::resolve_surreal(
                &cfg,
                surreal_url.as_deref(),
                surreal_user.as_deref(),
                surreal_pass.as_deref(),
                surreal_ns.as_deref(),
                surreal_db.as_deref(),
            );
            let path = index_path();

            if !path.exists() {
                eprintln!(
                    "No index found at {}. Run 'webfind crawl' or 'webfind index import' first.",
                    path.display()
                );
                std::process::exit(1);
            }

            let embedder: Option<Arc<dyn webfind::engine::embedder::Embedder>> = if hybrid {
                Some(Arc::new(
                    webfind::engine::embedder::FastembedEmbedder::new()
                        .context("load embedding model for hybrid search")?,
                ))
            } else {
                None
            };
            let indexer = Indexer::new(Arc::new(
                embedder
                    .map(|e| InMemorySearchEngine::with_embedder(e))
                    .unwrap_or_else(InMemorySearchEngine::new),
            ));
            let ranker = Ranker::new();

            let depth_enum = match depth {
                webfind::cli::DepthArg::Shallow => SearchDepth::Shallow,
                webfind::cli::DepthArg::Standard => SearchDepth::Standard,
                webfind::cli::DepthArg::Deep => SearchDepth::Deep,
                webfind::cli::DepthArg::Comprehensive => SearchDepth::Comprehensive,
            };

            let start = Instant::now();
            let mut results = indexer.search_bm25(&query, limit as usize).await?;
            let search_ms = start.elapsed().as_millis() as u64;

            let request = SearchRequest {
                query: query.clone(),
                depth: depth_enum.clone(),
                limit,
                output: OutputFormat::Json,
                language: None,
                date_range: None,
                domains: None,
                content_type: None,
                include_content,
                include_graph,
                include_keywords,
                include_metrics,
                hybrid,
            };

            let vector_scores: Option<HashMap<String, f64>> = if hybrid {
                Some(indexer.search_vector(&query, limit as usize).await?)
            } else {
                None
            };

            let data_dir = webfind::config::data_dir();
            let cache = webfind::engine::pagerank_cache::PageRankCache::new(&data_dir);
            let graph_store_arc: Option<Arc<dyn CrawlGraphStore + Send + Sync>> = match graph_store
            {
                Some(GraphStoreArg::Surrealdb) => {
                    let store = SurrealStore::new(
                        &surreal.url,
                        &surreal.user,
                        &surreal.pass,
                        &surreal.ns,
                        &surreal.db,
                    )
                    .await
                    .context("connect to graph store for ranking")?;
                    Some(Arc::new(store))
                }
                Some(GraphStoreArg::Memory) => Some(Arc::new(InMemoryCrawlGraph::new())),
                None => None,
            };
            let graph_scores: Option<HashMap<String, f64>> = match &graph_store_arc {
                Some(store) => Some(cache.get_or_compute(store.as_ref(), 20, 0.85).await?),
                None => None,
            };
            results = ranker.rank(
                results,
                &request,
                graph_scores.as_ref(),
                vector_scores.as_ref(),
            );

            let graph_summary = if include_graph {
                match &graph_store_arc {
                    Some(store) => build_graph_summary(store.as_ref(), &results).await,
                    None => None,
                }
            } else {
                None
            };

            let mut signals = vec!["bm25".to_string()];
            if hybrid {
                signals.push("vector".to_string());
            }
            if graph_scores.is_some() {
                signals.push("graph".to_string());
            }
            let total = results.len() as u64;
            let meta = indexer.metadata(signals).await?;

            let response = SearchResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                query: query.clone(),
                depth: depth_enum,
                total_results: total,
                returned: results.len() as u32,
                latency_ms: search_ms,
                results,
                suggestions: vec![],
                related: vec![],
                graph: graph_summary,
                metadata: meta,
            };

            let output_enum = match output {
                webfind::cli::OutputArg::Json => OutputFormat::Json,
                webfind::cli::OutputArg::Report => OutputFormat::Report,
                webfind::cli::OutputArg::Markdown => OutputFormat::Markdown,
            };

            let formatted = format_response(&response, &output_enum);
            print!("{}", formatted);

            Ok(())
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
                Some(webfind::engine::device_profile::SessionManager::new(true))
            } else {
                None
            };
            let fetcher = if proxies.is_some() {
                Fetcher::new_human(proxy_pool, session_manager, true, 1)?
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
            surreal_url,
            surreal_user,
            surreal_pass,
            surreal_ns,
            surreal_db,
            pages_per_session,
            session_max_age_minutes,
            hybrid,
            follow_external,
            min_depth,
            max_depth,
            topics,
        } => {
            let graph_store_name = webfind::config::resolve_graph_store(
                &cfg,
                graph_store.as_ref().map(|g| g.as_str()),
            );
            let graph_store = if graph_store_name == "surrealdb" {
                GraphStoreArg::Surrealdb
            } else {
                GraphStoreArg::Memory
            };
            let surreal = webfind::config::resolve_surreal(
                &cfg,
                surreal_url.as_deref(),
                surreal_user.as_deref(),
                surreal_pass.as_deref(),
                surreal_ns.as_deref(),
                surreal_db.as_deref(),
            );
            let path = index_path();
            let embedder: Option<Arc<dyn webfind::engine::embedder::Embedder>> = if hybrid {
                Some(Arc::new(
                    webfind::engine::embedder::FastembedEmbedder::new()
                        .context("load embedding model for hybrid indexing")?,
                ))
            } else {
                None
            };
            let policy = match recrawl_policy {
                webfind::cli::ReCrawlPolicyArg::Never => {
                    webfind::storage::cache_store::ReCrawlPolicy::Never
                }
                webfind::cli::ReCrawlPolicyArg::Fixed => {
                    webfind::storage::cache_store::ReCrawlPolicy::FixedDays(recrawl_days)
                }
                webfind::cli::ReCrawlPolicyArg::Adaptive => {
                    webfind::storage::cache_store::ReCrawlPolicy::Adaptive
                }
            };
            let proxy_list: Vec<String> = proxies
                .as_ref()
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let protocol = match proxy_protocol {
                webfind::cli::ProxyProtocolArg::Http => {
                    webfind::engine::proxy_pool::ProxyProtocol::Http
                }
                webfind::cli::ProxyProtocolArg::Https => {
                    webfind::engine::proxy_pool::ProxyProtocol::Https
                }
                webfind::cli::ProxyProtocolArg::Socks5 => {
                    webfind::engine::proxy_pool::ProxyProtocol::Socks5
                }
            };

            if bulk {
                let proxy_pool = ProxyPool::new();
                if !proxy_list.is_empty() {
                    for url in &proxy_list {
                        proxy_pool.add(webfind::engine::proxy_pool::ProxyEndpoint::from_url(url)?);
                    }
                }
                if let Some(ref cidr) = proxy_cidr {
                    let generated = ProxyPool::from_cidr(cidr, 8080, protocol, 10)?;
                    for ep in generated.endpoints() {
                        proxy_pool.add(ep);
                    }
                }

                let graph: Arc<dyn webfind::engine::crawl_graph::CrawlGraphStore> =
                    match graph_store {
                        webfind::cli::GraphStoreArg::Memory => {
                            Arc::new(webfind::engine::crawl_graph::InMemoryCrawlGraph::new())
                        }
                        webfind::cli::GraphStoreArg::Surrealdb => {
                            let store = SurrealStore::new(
                                &surreal.url,
                                &surreal.user,
                                &surreal.pass,
                                &surreal.ns,
                                &surreal.db,
                            )
                            .await
                            .context("connect to SurrealDB graph store")?;
                            Arc::new(store)
                        }
                    };

                let crawler = BulkDomainCrawler::new(
                    proxy_pool,
                    pages_per_session as usize,
                    session_max_age_minutes as u64,
                    delay as u64,
                    rps,
                    max_pages as usize,
                    follow_external,
                    min_depth,
                    max_depth,
                )
                .with_respect_robots(respect_robots)
                .with_graph_store(graph)
                .with_topics(
                    topics
                        .map(|t| t.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
                        .unwrap_or_default(),
                );

                if dynamic {
                    println!("Dynamic fallback is not yet supported in bulk mode; ignoring.");
                }

                println!("Crawling (bulk): {}", seed);
                println!(
                    "Max pages: {} | Delay: {}ms | RPS: {}",
                    max_pages, delay, rps
                );
                println!("Index: {}", path.display());
                println!("Cache: {}/cache/urls.jsonl", cache_dir);
                println!("Re-crawl: {:?}", policy);
                println!("Graph store: {:?}", graph_store);
                println!();

                let start = Instant::now();
                let contents = crawler.crawl(&seed).await?;
                let elapsed = start.elapsed();

                // Index fetched content.
                let mut stats = webfind::engine::crawler::CrawlStats {
                    pages_discovered: contents.len(),
                    pages_fetched: contents.iter().filter(|c| c.is_valid_content).count(),
                    ..Default::default()
                };

                if !contents.is_empty() {
                    let indexer = Indexer::new(Arc::new(
                        embedder
                            .as_ref()
                            .map(|e| InMemorySearchEngine::with_embedder(e.clone()))
                            .unwrap_or_else(InMemorySearchEngine::new),
                    ));
                    match indexer.index_batch(&contents).await {
                        Ok(_) => {
                            stats.pages_indexed =
                                contents.iter().filter(|c| c.is_valid_content).count();
                        }
                        Err(e) => {
                            tracing::error!("failed to index batch: {}", e);
                            stats.index_errors += 1;
                        }
                    }
                }

                println!();
                println!("Bulk crawl complete in {:.2}s:", elapsed.as_secs_f64());
                println!("  Discovered: {} pages", stats.pages_discovered);
                println!("  Fetched:    {} pages", stats.pages_fetched);
                println!("  Indexed:    {} pages", stats.pages_indexed);
                if stats.index_errors > 0 {
                    println!("  Index errors: {}", stats.index_errors);
                }
                return Ok(());
            }

            let crawler = Crawler::new(&seed, max_pages, delay)
                .with_cache(&cache_dir, policy, skip_cached)?
                .with_human_mode(
                    proxy_list,
                    proxy_cidr.clone(),
                    protocol,
                    rotate_ua,
                    sticky_sessions,
                    rps,
                );
            let crawler = if let Some(ref e) = embedder {
                crawler.with_embedder(Some(e.clone()))
            } else {
                crawler
            };
            let crawler = if dynamic {
                crawler.with_dynamic_fallback(dynamic_wait_ms)
            } else {
                crawler
            };

            if rotate_ua {
                println!("User-Agent rotation: enabled");
            }
            if !proxies.as_ref().unwrap_or(&String::new()).is_empty() {
                println!("Proxies: {}", proxies.as_ref().unwrap());
            }

            println!("Crawling: {}", seed);
            println!("Max pages: {} | Delay: {}ms", max_pages, delay);
            println!("Index: {}", path.display());
            println!("Cache: {}/cache/urls.jsonl", cache_dir);
            println!("Re-crawl: {:?}", policy);
            println!();

            match crawler.crawl_and_index(&path).await {
                Ok(stats) => {
                    println!();
                    println!("Crawl complete:");
                    println!("  Discovered: {} pages", stats.pages_discovered);
                    println!("  Fetched:    {} pages", stats.pages_fetched);
                    println!(
                        "  Skipped:    {} (cache fresh) | {} (near-duplicate)",
                        stats.pages_skipped_cache, stats.pages_skipped_dup
                    );
                    println!("  Indexed:    {} pages", stats.pages_indexed);
                    if stats.fetch_errors > 0 {
                        println!("  Fetch errors:    {}", stats.fetch_errors);
                    }
                    if stats.index_errors > 0 {
                        println!("  Index errors:    {}", stats.index_errors);
                    }
                    Ok(())
                }
                Err(e) => {
                    eprintln!("Crawl failed: {}", e);
                    Err(e)
                }
            }
        }

        Commands::Index { action } => match action {
            webfind::cli::IndexAction::Import { dataset, limit } => {
                println!("Importing from Common Crawl: {} (limit={})", dataset, limit);
                anyhow::bail!("Import not yet implemented — coming in Phase 10");
            }
            webfind::cli::IndexAction::Stats => {
                let path = index_path();
                if !path.exists() {
                    println!("No index found at {}", path.display());
                    println!("Run 'webfind crawl' or 'webfind index import' to create one.");
                    return Ok(());
                }
                let indexer = Indexer::new(Arc::new(InMemorySearchEngine::new()));
                let count = indexer.doc_count().await?;
                println!("Index: {}", path.display());
                println!("Documents: {}", count);
                println!("Engine: v{}", env!("CARGO_PKG_VERSION"));
                Ok(())
            }
            webfind::cli::IndexAction::Optimize => {
                let path = index_path();
                if !path.exists() {
                    println!("No index found at {}", path.display());
                    return Ok(());
                }
                let indexer = Indexer::new(Arc::new(InMemorySearchEngine::new()));
                indexer.optimize().await?;
                println!("Index optimized.");
                Ok(())
            }
        },

        Commands::Graph {
            url,
            depth,
            direction,
            surreal_url,
            surreal_user,
            surreal_pass,
            surreal_ns,
            surreal_db,
        } => {
            let surreal = webfind::config::resolve_surreal(
                &cfg,
                surreal_url.as_deref(),
                surreal_user.as_deref(),
                surreal_pass.as_deref(),
                surreal_ns.as_deref(),
                surreal_db.as_deref(),
            );
            let store = SurrealStore::new(
                &surreal.url,
                &surreal.user,
                &surreal.pass,
                &surreal.ns,
                &surreal.db,
            )
            .await
            .context("connect to SurrealDB graph store")?;

            let direction = match direction {
                webfind::cli::DirectionArg::Inbound => TraversalDirection::Inbound,
                webfind::cli::DirectionArg::Outbound => TraversalDirection::Outbound,
                webfind::cli::DirectionArg::Both => TraversalDirection::Both,
            };
            let visited = webfind::engine::crawl_graph::traverse_graph(
                Arc::new(store),
                &url,
                depth,
                direction,
            )
            .await;

            println!(
                "Graph traversal: {} (depth={}, direction={:?})",
                url, depth, direction
            );
            println!("Discovered {} URLs", visited.len());
            for u in &visited {
                println!("  - {}", u);
            }
            Ok(())
        }

        Commands::ProxyPool {
            listen,
            cidr,
            source_ips,
        } => {
            let listen_addr = listen.parse()?;
            let ips: Vec<std::net::IpAddr> = if let Some(c) = cidr {
                webfind::engine::proxy_server::generate_ips_from_cidr(&c)?
            } else if let Some(list) = source_ips {
                list.split(',')
                    .map(|s| s.trim().parse())
                    .collect::<Result<Vec<_>, _>>()?
            } else {
                anyhow::bail!("provide either --cidr or --source-ips");
            };
            let server = webfind::engine::proxy_server::ProxyServer::new(listen_addr, ips);
            server.run().await
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
            let proxy_list: Vec<String> = proxies
                .as_ref()
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            let proxy_pool = ProxyPool::new();
            for url in &proxy_list {
                proxy_pool.add(ProxyEndpoint::from_url(url)?);
            }

            let embedder: Option<Arc<dyn Embedder>> = if hybrid {
                Some(Arc::new(
                    FastembedEmbedder::new().context("load embedding model for hybrid research")?,
                ))
            } else {
                None
            };

            let graph: Arc<dyn CrawlGraphStore + Send + Sync> = Arc::new(InMemoryCrawlGraph::new());
            let topics: Vec<String> = topics
                .as_deref()
                .map(|s| {
                    s.split(',')
                        .map(|t| t.trim().to_lowercase())
                        .filter(|t| !t.is_empty())
                        .collect()
                })
                .unwrap_or_default();

            // Determine seeds: explicit seed, additional seeds, or auto-discovery.
            let auto_discover = seed.is_none();
            let mut all_seeds: Vec<String> = Vec::new();
            if let Some(ref seed) = seed {
                all_seeds.push(seed.clone());
            }
            if let Some(ref seeds_str) = seeds {
                all_seeds.extend(
                    seeds_str
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty()),
                );
            }

            if all_seeds.is_empty() {
                // Auto-discover seeds from WebFind's own index and graph.
                // In the CLI, the crawl starts with a fresh in-memory index, so
                // discovery falls through to query-driven candidate generation.
                let temp_engine = Arc::new(InMemorySearchEngine::new());
                let temp_indexer = Indexer::new(temp_engine);
                let discovered = webfind::engine::discovery::discover_seeds(
                    &temp_indexer,
                    Some(graph.as_ref()),
                    &query,
                )
                .await?;
                if discovered.is_empty() {
                    anyhow::bail!("No seeds discovered for the query. Provide a seed URL or rephrase the query.");
                }
                all_seeds = discovered;
            }

            let follow_external = if auto_discover { true } else { follow_external };

            let crawler =
                BulkDomainCrawler::new(proxy_pool, 100, 30, delay as u64, 1, max_pages as usize, follow_external, min_depth, max_depth)
                    .with_respect_robots(respect_robots)
                    .with_graph_store(graph.clone())
                    .with_topics(topics);

            // Multi-seed: crawl all seeds, merge and deduplicate.
            println!("Researching: {}", all_seeds.join(", "));
            println!("Query: {}", query);
            let crawl_start = Instant::now();
            let mut contents: Vec<StructuredContent> = Vec::new();
            let mut seen_urls: std::collections::HashSet<String> = std::collections::HashSet::new();
            for seed_url in &all_seeds {
                let crawled = crawler.crawl(seed_url).await?;
                for c in crawled {
                    if seen_urls.insert(c.url.clone()) {
                        contents.push(c);
                    }
                }
            }
            println!(
                "Crawl + index complete in {:.2}s ({} pages)",
                crawl_start.elapsed().as_secs_f64(),
                contents.len()
            );

            let indexer = Indexer::new(Arc::new(
                embedder
                    .as_ref()
                    .map(|e| InMemorySearchEngine::with_embedder(e.clone()))
                    .unwrap_or_else(InMemorySearchEngine::new),
            ));
            indexer.index_batch(&contents).await?;

            let mut results = indexer.search_bm25(&query, limit as usize).await?;

            let request = SearchRequest {
                query: query.clone(),
                depth: SearchDepth::Standard,
                limit,
                output: OutputFormat::Json,
                language: None,
                date_range: None,
                domains: None,
                content_type: None,
                include_content,
                include_graph,
                include_keywords: false,
                include_metrics: false,
                hybrid,
            };

            let vector_scores: Option<HashMap<String, f64>> = if hybrid {
                Some(indexer.search_vector(&query, limit as usize).await?)
            } else {
                None
            };

            let ranker = Ranker::new();
            results = ranker.rank(results, &request, None, vector_scores.as_ref());

            if include_content {
                Indexer::attach_content(&mut results, &contents);
            }

            let graph_summary = if include_graph {
                build_graph_summary(graph.as_ref(), &results).await
            } else {
                None
            };

            let mut signals = vec!["bm25".to_string()];
            if hybrid {
                signals.push("vector".to_string());
            }
            let meta = indexer.metadata(signals).await?;

            let response = SearchResponse {
                request_id: uuid::Uuid::new_v4().to_string(),
                query,
                depth: SearchDepth::Standard,
                total_results: results.len() as u64,
                returned: results.len() as u32,
                latency_ms: crawl_start.elapsed().as_millis() as u64,
                results,
                suggestions: vec![],
                related: vec![],
                graph: graph_summary,
                metadata: meta,
            };

            let body = format_response(&response, &OutputFormat::Json);
            print!("{}", body);
            Ok(())
        }

        Commands::Serve {
            mode: _,
            port,
            transport,
            graph_store,
            surreal_url,
            surreal_user,
            surreal_pass,
            surreal_ns,
            surreal_db,
            hybrid,
            rate_limit,
            gui_port,
        } => {
            let graph_store_name = webfind::config::resolve_graph_store(
                &cfg,
                graph_store.as_ref().map(|g| g.as_str()),
            );
            let graph_store = if graph_store_name == "surrealdb" {
                Some(GraphStoreArg::Surrealdb)
            } else if graph_store_name == "memory" {
                Some(GraphStoreArg::Memory)
            } else {
                None
            };
            let surreal = webfind::config::resolve_surreal(
                &cfg,
                surreal_url.as_deref(),
                surreal_user.as_deref(),
                surreal_pass.as_deref(),
                surreal_ns.as_deref(),
                surreal_db.as_deref(),
            );

            // /search is OFFLINE: it queries the existing SurrealDB index populated
            // by /research (or other indexing paths). It never crawls the internet.
            // /research is ONLINE: it crawls the internet and writes into SurrealDB.
            let embedder: Option<Arc<dyn webfind::engine::embedder::Embedder>> = if hybrid {
                Some(Arc::new(
                    webfind::engine::embedder::FastembedEmbedder::new()
                        .context("load embedding model for hybrid search")?,
                ))
            } else {
                None
            };

            // Build the graph store (SurrealDB) which is the single source of truth
            // for both offline search and research persistence.
            let (surreal_store, graph_store): (
                Option<Arc<SurrealStore>>,
                Option<Arc<dyn CrawlGraphStore + Send + Sync>>,
            ) = match graph_store {
                Some(GraphStoreArg::Surrealdb) => {
                    let store = SurrealStore::new(
                        &surreal.url,
                        &surreal.user,
                        &surreal.pass,
                        &surreal.ns,
                        &surreal.db,
                    )
                    .await
                    .context("connect to graph store for API ranking")?;
                    let arc = Arc::new(store);
                    (
                        Some(arc.clone()),
                        Some(arc as Arc<dyn CrawlGraphStore + Send + Sync>),
                    )
                }
                Some(GraphStoreArg::Memory) => {
                    (None, Some(Arc::new(InMemoryCrawlGraph::new())))
                }
                None => (None, None),
            };

            // Use SurrealDB as the search backend in serve mode so both /search
            // (offline query) and /research (online crawl) operate on the same
            // knowledge graph and vector DB. /search never triggers a crawl.
            let indexer: Indexer = if let Some(ref store) = surreal_store {
                let engine = SurrealSearchEngine::new(store.db());
                let engine = if let Some(e) = embedder.clone() {
                    engine.with_embedder(e)
                } else {
                    engine
                };
                if let Err(e) = engine.apply_schema().await {
                    tracing::warn!("failed to apply SurrealSearchEngine schema: {}", e);
                }
                Indexer::new(Arc::new(engine))
            } else {
                Indexer::new(Arc::new(
                    embedder
                        .map(|e| InMemorySearchEngine::with_embedder(e))
                        .unwrap_or_else(InMemorySearchEngine::new),
                ))
            };

            let audit_store: Option<
                Arc<dyn webfind::engine::fingerprint::FingerprintAuditLog + Send + Sync>,
            > = surreal_store
                .as_ref()
                .map(|s| s.clone() as Arc<dyn webfind::engine::fingerprint::FingerprintAuditLog + Send + Sync>);

            let rate_limit = webfind::config::resolve_rate_limit(&cfg, rate_limit);

            // Initialize query log service for autocomplete if SurrealDB is available.
            let query_log = if let Some(ref store) = surreal_store {
                let log_service = webfind::engine::query_log::QueryLogService::new(
                    store.db(),
                    format!("{}_{}", surreal.ns, surreal.db),
                );
                Some(Arc::new(log_service))
            } else {
                None
            };

            // Initialize category service if SurrealDB is available.
            let categories = if let Some(ref store) = surreal_store {
                let cat_service = webfind::engine::categories::CategoryService::new(store.db());
                Some(Arc::new(cat_service))
            } else {
                None
            };

            match transport {
                webfind::cli::TransportArg::Http => {
                    println!("Starting HTTP search API on port {}", port);
                    if rate_limit.is_some() {
                        println!("Rate limiting enabled");
                    }
                    println!("Starting WebFind GUI on port {}", gui_port);
                    if query_log.is_some() {
                        println!("Query log & autocomplete enabled");
                    }

                    let state = Arc::new(
                        webfind::api::ApiState::new(
                            indexer,
                            graph_store,
                            audit_store,
                            webfind::config::data_dir(),
                        )
                        .with_query_log(query_log)
                        .with_categories(categories),
                    );

                    let api_handle = tokio::spawn(webfind::api::run_server(
                        state.clone(),
                        rate_limit,
                        port,
                    ));
                    let gui_handle = tokio::spawn(webfind::gui::run_server(
                        state.clone(),
                        gui_port,
                    ));

                    let (api_res, gui_res) = tokio::try_join!(api_handle, gui_handle)?;
                    api_res?;
                    gui_res?;
                    Ok(())
                }
                webfind::cli::TransportArg::Stdio => {
                    println!("Starting MCP server on stdio");
                    webfind::mcp::WebfindMcpServer::run_stdio(
                        indexer,
                        graph_store,
                        audit_store,
                        webfind::config::data_dir(),
                    )
                    .await?;
                    Ok(())
                }
            }
        }

        Commands::Status => {
            let path = index_path();
            println!("webfind v{}", env!("CARGO_PKG_VERSION"));

            if path.exists() {
                let indexer = Indexer::new(Arc::new(InMemorySearchEngine::new()));
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
