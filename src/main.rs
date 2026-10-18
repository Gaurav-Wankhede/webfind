use clap::Parser;
use tracing_subscriber::EnvFilter;
use webfind::cli::{Cli, Commands};

// Global allocator (P4.5): mimalloc improves p99 latency for the concurrent
// fetch/index workload vs. the system allocator. Must be declared before main.
//
// Only compiled with the `mimalloc` feature. mimalloc builds C sources that
// some cross-compilers reject (`-Werror,-Wdate-time` on its own
// `options.c`), which made the default build fail on Windows. Windows ships a
// capable allocator already, so the default build uses the system allocator
// there and native builders can opt in: `cargo build --release --features mimalloc`.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod commands;

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
            queries,
            depth,
            limit,
            output,
            output_file,
            language,
            domains,
            include_content,
            include_graph,
            include_keywords,
            include_metrics,
            graph_store,
            turso_path,
            hybrid,
            live,
        } => {            return commands::search::run(
                &cfg,
                query,
                queries,
                depth,
                limit,
                output,
                output_file,
                language,
                domains,
                include_content,
                include_graph,
                include_keywords,
                include_metrics,
                graph_store,
                turso_path,
                hybrid,
                live,
            )
            .await;
        }


        Commands::Research {
            query,
            seed,
            seeds,
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
            auto_depth,
            domains,
            dynamic,
            deep,
            output,
            graph_store,
            turso_path,
        } => {
            return commands::research::run(
                &cfg,
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
                auto_depth,
                domains,
                Some(seeds.join(",")),
                dynamic,
                deep,
                output,
                graph_store,
                turso_path,
            )
            .await;
        }

        Commands::Fetch {
            url,
            urls,
            output,
            compact,
            mode,
            query,
            extract_links,
            extract_keywords,
            dynamic,
            dynamic_wait_ms,
            proxies,
        } => {
            return commands::fetch::run(
                url,
                urls,
                output,
                compact,
                mode,
                query,
                extract_links,
                extract_keywords,
                dynamic,
                dynamic_wait_ms,
                proxies,
            )
            .await;
        }

        Commands::Crawl {
            seed,
            daemon,
            domains,
            daemon_pages,
            daemon_interval,
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
            auto_depth,
            topics,
        } => {
            return commands::crawl::run(
                &cfg,
                seed,
                daemon,
                domains,
                daemon_pages,
                daemon_interval,
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
                auto_depth,
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

        Commands::Migrate { from, to } => {
            return commands::migrate::run(&cfg, from, to).await;
        }

        Commands::Status => {
            return commands::status::run().await;
        }
    }
}
