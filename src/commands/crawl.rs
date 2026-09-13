use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;

use webfind::engine::bg_worker::BackgroundWorker;
use webfind::engine::bulk_crawler::{BulkDomainCrawler, FollowExternalLinks, RespectRobots};
use webfind::engine::crawl_graph::CrawlGraphStore;
use webfind::engine::crawler::Crawler;
use webfind::engine::embedder::{Embedder, FastembedEmbedder};
use webfind::engine::proxy_pool::{ProxyEndpoint, ProxyPool};
use webfind::engine::search_engine::{InMemorySearchEngine, SearchEngine};

#[allow(clippy::too_many_arguments)]
pub async fn run(
    cfg: &webfind::config::WebfindConfig,
    seed: Option<String>,
    daemon: bool,
    domains: Option<String>,
    daemon_pages: u32,
    daemon_interval: u64,
    delay: u32,
    max_pages: u32,
    cache_dir: String,
    recrawl_policy: webfind::cli::ReCrawlPolicyArg,
    recrawl_days: u32,
    skip_cached: bool,
    proxies: Option<String>,
    proxy_cidr: Option<String>,
    proxy_protocol: webfind::cli::ProxyProtocolArg,
    rotate_ua: bool,
    sticky_sessions: bool,
    respect_robots: bool,
    rps: u32,
    dynamic: bool,
    dynamic_wait_ms: u64,
    bulk: bool,
    graph_store: Option<webfind::cli::GraphStoreArg>,
    turso_path: Option<String>,
    pages_per_session: u32,
    session_max_age_minutes: u32,
    hybrid: bool,
    follow_external: bool,
    min_depth: u32,
    max_depth: u32,
    auto_depth: bool,
    topics: Option<String>,
) -> anyhow::Result<()> {
    use webfind::engine::util::split_comma;

    // Background curation daemon mode: continuously crawl the curated seed
    // catalog (official, non-Wikipedia sources) into the persistent graph store.
    if daemon {
        return run_daemon(
            cfg,
            graph_store,
            turso_path,
            domains,
            daemon_pages,
            daemon_interval,
        )
        .await;
    }

    // Non-daemon mode requires a seed.
    let seed = seed.context("a --seed URL is required unless running --daemon")?;

    let graph_store = webfind::config::resolve_graph_store(cfg, graph_store);
    let path = crate::commands::index_path();
    let embedder: Option<Arc<dyn Embedder>> = if hybrid {
        Some(Arc::new(
            FastembedEmbedder::new().context("load embedding model for hybrid indexing")?,
        ))
    } else {
        None
    };
    let policy = recrawl_policy.to_policy(recrawl_days);
    let proxy_list: Vec<String> = proxies.as_ref().map(|s| split_comma(s)).unwrap_or_default();
    let protocol = match proxy_protocol {
        webfind::cli::ProxyProtocolArg::Http => webfind::engine::proxy_pool::ProxyProtocol::Http,
        webfind::cli::ProxyProtocolArg::Https => webfind::engine::proxy_pool::ProxyProtocol::Https,
        webfind::cli::ProxyProtocolArg::Socks5 => {
            webfind::engine::proxy_pool::ProxyProtocol::Socks5
        }
    };

    if bulk {
        let proxy_pool = ProxyPool::new();
        if !proxy_list.is_empty() {
            for url in &proxy_list {
                proxy_pool.add(ProxyEndpoint::from_url(url)?)?;
            }
        }
        if let Some(ref cidr) = proxy_cidr {
            let generated = ProxyPool::from_cidr(cidr, 8080, protocol, 10)?;
            for ep in generated.endpoints()?.into_iter() {
                proxy_pool.add(ep.clone())?;
            }
        }

        let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
        let graph: Arc<dyn CrawlGraphStore> = crate::commands::build_graph_store(
            &graph_store,
            &turso_path,
            cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref()),
        )
        .await?;

        let crawler = BulkDomainCrawler::new(
            proxy_pool,
            pages_per_session as usize,
            session_max_age_minutes as u64,
            delay as u64,
            rps,
            max_pages as usize,
            if follow_external {
                FollowExternalLinks::Follow
            } else {
                FollowExternalLinks::Ignore
            },
            min_depth,
            max_depth,
        )
        .with_respect_robots(if respect_robots {
            RespectRobots::Yes
        } else {
            RespectRobots::No
        })
        .with_auto_depth(auto_depth)
        .with_graph_store(graph)
        .with_topics(
            topics
                .map(|t| {
                    t.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
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

        let mut stats = webfind::engine::crawler::CrawlStats {
            pages_discovered: contents.len(),
            pages_fetched: contents.iter().filter(|c| c.is_valid_content).count(),
            ..Default::default()
        };

        if !contents.is_empty() {
            let indexer = embedder
                .as_ref()
                .map(|e| InMemorySearchEngine::with_embedder(e.clone()))
                .unwrap_or_default();
            match indexer.index_batch(&contents).await {
                Ok(_) => {
                    stats.pages_indexed = contents.iter().filter(|c| c.is_valid_content).count();
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
        println!("Proxies: {}", proxies.as_ref().unwrap_or(&String::new()));
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

/// Run the background curation daemon: continuously crawl the curated seed
/// catalog (official, non-Wikipedia sources) into the persistent graph store,
/// honoring each source's recrawl cadence and the configured storage budget.
async fn run_daemon(
    cfg: &webfind::config::WebfindConfig,
    graph_store_arg: Option<webfind::cli::GraphStoreArg>,
    turso_path: Option<String>,
    domains: Option<String>,
    daemon_pages: u32,
    daemon_interval: u64,
) -> anyhow::Result<()> {
    use webfind::engine::curation_daemon::{CurationDaemon, DaemonConfig};

    println!("{}", webfind::engine::seed_catalog::catalog_summary());

    let kind = webfind::config::resolve_graph_store(cfg, graph_store_arg);
    let turso_path = webfind::config::resolve_turso(cfg, turso_path.as_deref());
    let graph: Arc<dyn CrawlGraphStore> = crate::commands::build_graph_store(
        &kind,
        &turso_path,
        cfg.turso.as_ref().and_then(|t| t.encryption_key.as_deref()),
    )
    .await?;

    // Background worker persists crawled content + embeddings into the graph.
    let worker = Arc::new(BackgroundWorker::new(graph.clone(), None, 64));

    let only_domains: Vec<String> = domains
        .as_ref()
        .map(|d| {
            d.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let config = DaemonConfig {
        delay_ms: 1000,
        max_pages_per_source: daemon_pages.max(1) as usize,
        sweep_interval_secs: daemon_interval.max(60),
        only_domains,
    };

    let daemon = CurationDaemon::new(config, graph, worker, None);
    daemon.run_forever().await
}
