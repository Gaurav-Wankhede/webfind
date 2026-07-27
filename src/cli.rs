use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "webfind",
    about = "Self-hosted native search engine for AI agents — CLI + MCP, zero cost",
    version,
    next_display_order = None
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Search the web index
    Search {
        /// The search query
        query: String,

        /// Search depth level
        #[arg(short, long, value_enum, default_value_t = DepthArg::Standard)]
        depth: DepthArg,

        /// Maximum results to return
        #[arg(short, long, default_value = "10")]
        limit: u32,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputArg::Report)]
        output: OutputArg,

        /// Language filter (ISO 639-1)
        #[arg(long)]
        language: Option<String>,

        /// Domain filter (comma-separated)
        #[arg(long)]
        domains: Option<String>,

        /// Include full content in results
        #[arg(long, default_value = "true")]
        include_content: bool,

        /// Include graph relationships
        #[arg(long, default_value = "false")]
        include_graph: bool,

        /// Include keywords
        #[arg(long, default_value = "true")]
        include_keywords: bool,

        /// Include readability metrics
        #[arg(long, default_value = "false")]
        include_metrics: bool,

        /// Enable BM25 + vector hybrid re-ranking.
        #[arg(long, default_value = "false")]
        hybrid: bool,

        /// Graph store to load PageRank from for ranking.
        #[arg(long, value_enum, env = "WEBFIND_GRAPH_STORE")]
        graph_store: Option<GraphStoreArg>,

        /// SurrealDB connection URL (e.g. memory, ws://localhost:8000).
        #[arg(long)]
        surreal_url: Option<String>,

        /// SurrealDB username.
        #[arg(long)]
        surreal_user: Option<String>,

        /// SurrealDB password.
        #[arg(long)]
        surreal_pass: Option<String>,

        /// SurrealDB namespace.
        #[arg(long)]
        surreal_ns: Option<String>,

        /// SurrealDB database.
        #[arg(long)]
        surreal_db: Option<String>,
    },

    /// Fetch and extract content from a URL
    Fetch {
        /// URL to fetch (or first URL if --urls is used)
        url: String,

        /// Additional URLs to fetch in parallel.
        #[arg(long, value_delimiter = ',')]
        urls: Vec<String>,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputArg::Report)]
        output: OutputArg,

        /// Extract links
        #[arg(long, default_value = "false")]
        extract_links: bool,

        /// Extract keywords
        #[arg(long, default_value = "true")]
        extract_keywords: bool,

        /// Use Chromium headless browser for JS-rendered pages.
        #[arg(long, default_value = "false")]
        dynamic: bool,

        /// Milliseconds to wait for JS execution in dynamic mode.
        #[arg(long, default_value = "2000")]
        dynamic_wait_ms: u64,

        /// Proxy URLs to route all requests through.
        #[arg(long)]
        proxies: Option<String>,
    },

    /// Crawl and index web pages
    Crawl {
        /// Seed URL to start crawling from
        #[arg(long)]
        seed: String,

        /// Crawl depth
        #[arg(short, long, default_value = "2")]
        depth: u32,

        /// Delay between requests (ms)
        #[arg(long, default_value = "1000")]
        delay: u32,

        /// Maximum pages to crawl
        #[arg(long, default_value = "1000")]
        max_pages: u32,

        /// Directory for the URL cache
        #[arg(long, default_value = ".")]
        cache_dir: String,

        /// When to re-crawl a cached URL
        #[arg(long, value_enum, default_value_t = ReCrawlPolicyArg::Fixed)]
        recrawl_policy: ReCrawlPolicyArg,

        /// Days before a cached URL may be re-crawled (fixed policy)
        #[arg(long, default_value = "7")]
        recrawl_days: u32,

        /// Skip URLs that are still fresh in the cache
        #[arg(long, default_value = "false")]
        skip_cached: bool,

        /// Rotate through these proxy URLs (comma-separated, supports socks5://)
        #[arg(long)]
        proxies: Option<String>,

        /// Generate random proxies inside this CIDR block (e.g. 10.0.0.0/24).
        /// Requires your own proxy fleet listening on that subnet.
        #[arg(long)]
        proxy_cidr: Option<String>,

        /// Protocol for proxies generated from --proxy-cidr.
        #[arg(long, value_enum, default_value_t = ProxyProtocolArg::Http)]
        proxy_protocol: ProxyProtocolArg,

        /// Rotate User-Agent strings
        #[arg(long, default_value = "true")]
        rotate_ua: bool,

        /// Keep the same UA+proxy for a given domain across requests.
        #[arg(long, default_value = "true")]
        sticky_sessions: bool,

        /// Respect robots.txt Disallow/Crawl-delay directives.
        #[arg(long, default_value = "true")]
        respect_robots: bool,

        /// Per-domain requests per second when using human mode.
        #[arg(long, default_value = "1")]
        rps: u32,

        /// Use Chromium headless browser fallback for JS-heavy pages.
        #[arg(long, default_value = "false")]
        dynamic: bool,

        /// Milliseconds to wait for JS execution in dynamic mode.
        #[arg(long, default_value = "2000")]
        dynamic_wait_ms: u64,

        /// Use the new bulk-domain crawler with session consistency.
        #[arg(long, default_value = "false")]
        bulk: bool,

        /// Graph store backend for bulk crawls.
        #[arg(long, value_enum)]
        graph_store: Option<GraphStoreArg>,

        /// SurrealDB connection URL (e.g. memory, ws://localhost:8000).
        #[arg(long)]
        surreal_url: Option<String>,

        /// SurrealDB username.
        #[arg(long)]
        surreal_user: Option<String>,

        /// SurrealDB password.
        #[arg(long)]
        surreal_pass: Option<String>,

        /// SurrealDB namespace.
        #[arg(long)]
        surreal_ns: Option<String>,

        /// SurrealDB database.
        #[arg(long)]
        surreal_db: Option<String>,

        /// Pages per session before rotating identity (bulk mode).
        #[arg(long, default_value = "100")]
        pages_per_session: u32,

        /// Session max age in minutes before rotating identity (bulk mode).
        #[arg(long, default_value = "30")]
        session_max_age_minutes: u32,

        /// Enable dense vector indexing for hybrid search.
        #[arg(long, default_value = "false")]
        hybrid: bool,

        /// Follow external (cross-domain) links during crawl.
        #[arg(long, default_value = "false")]
        follow_external: bool,

        /// Minimum hop depth to force-fetch (bypasses budget).
        #[arg(long, default_value = "3")]
        min_depth: u32,

        /// Maximum hop depth from seed before stopping discovery.
        #[arg(long, default_value = "5")]
        max_depth: u32,

        /// Query topics for content-aware link prioritization (comma-separated).
        #[arg(long)]
        topics: Option<String>,
    },

    /// Manage the search index
    Index {
        #[command(subcommand)]
        action: IndexAction,
    },

    /// Explore the link graph
    Graph {
        /// Starting URL
        url: String,

        /// Traversal depth
        #[arg(short, long, default_value = "1")]
        depth: u32,

        /// Direction: inbound, outbound, or both
        #[arg(long, value_enum, default_value_t = DirectionArg::Both)]
        direction: DirectionArg,

        /// SurrealDB connection URL (e.g. memory, ws://localhost:8000).
        #[arg(long)]
        surreal_url: Option<String>,

        /// SurrealDB username.
        #[arg(long)]
        surreal_user: Option<String>,

        /// SurrealDB password.
        #[arg(long)]
        surreal_pass: Option<String>,

        /// SurrealDB namespace.
        #[arg(long)]
        surreal_ns: Option<String>,

        /// SurrealDB database.
        #[arg(long)]
        surreal_db: Option<String>,
    },

    /// Start local forward proxy server with rotating egress IPs
    ProxyPool {
        /// Listen address
        #[arg(long, default_value = "0.0.0.0:4749")]
        listen: String,

        /// CIDR block to generate random egress IPs from (e.g. 10.0.0.0/24).
        /// The host must own/route these IPs.
        #[arg(long)]
        cidr: Option<String>,

        /// Comma-separated list of explicit egress source IPs.
        #[arg(long)]
        source_ips: Option<String>,
    },

    /// Crawl a seed URL and immediately search freshly indexed content.
    Research {
        /// Seed URL to start crawling from.
        seed: String,

        /// Query to run against the freshly indexed pages.
        query: String,

        /// Crawl depth.
        #[arg(short, long, default_value = "2")]
        depth: u32,

        /// Maximum pages to crawl.
        #[arg(long, default_value = "100")]
        max_pages: u32,

        /// Delay between requests (ms).
        #[arg(long, default_value = "1000")]
        delay: u32,

        /// Respect robots.txt Disallow/Crawl-delay directives.
        #[arg(long, default_value = "true")]
        respect_robots: bool,

        /// Proxy URLs to route requests through (comma-separated).
        #[arg(long)]
        proxies: Option<String>,

        /// Enable dense vector indexing + hybrid search for the query.
        #[arg(long, default_value = "false")]
        hybrid: bool,

        /// Maximum results to return.
        #[arg(long, default_value = "10")]
        limit: u32,

        /// Include graph relationships in the response.
        #[arg(long, default_value = "false")]
        include_graph: bool,

        /// Include full page content in each result.
        #[arg(long, default_value = "false")]
        include_content: bool,

        /// Follow external (cross-domain) links during crawl.
        #[arg(long, default_value = "false")]
        follow_external: bool,

        /// Minimum hop depth to force-fetch (bypasses budget).
        #[arg(long, default_value = "3")]
        min_depth: u32,

        /// Maximum hop depth from seed before stopping discovery.
        #[arg(long, default_value = "5")]
        max_depth: u32,

        /// Query topics for content-aware link prioritization (comma-separated).
        #[arg(long)]
        topics: Option<String>,

        /// Additional seed URLs for multi-seed crawling (comma-separated).
        #[arg(long)]
        seeds: Option<String>,
    },

    /// Start MCP server mode
    Serve {
        /// Transport mode
        #[arg(long, value_enum, default_value_t = ModeArg::Mcp)]
        mode: ModeArg,

        /// Port for HTTP transport (default: 4747)
        #[arg(long, default_value = "4747")]
        port: u16,

        /// Transport: stdio or http
        #[arg(long, value_enum, default_value_t = TransportArg::Stdio)]
        transport: TransportArg,

        /// Graph store to load PageRank from for ranking.
        #[arg(long, value_enum, env = "WEBFIND_GRAPH_STORE")]
        graph_store: Option<GraphStoreArg>,

        /// SurrealDB connection URL (e.g. memory, ws://localhost:8000).
        #[arg(long)]
        surreal_url: Option<String>,

        /// SurrealDB username.
        #[arg(long)]
        surreal_user: Option<String>,

        /// SurrealDB password.
        #[arg(long)]
        surreal_pass: Option<String>,

        /// SurrealDB namespace.
        #[arg(long)]
        surreal_ns: Option<String>,

        /// SurrealDB database.
        #[arg(long)]
        surreal_db: Option<String>,

        /// Enable BM25 + vector hybrid re-ranking by default.
        #[arg(long, default_value = "false")]
        hybrid: bool,

        /// Optional per-IP rate limit in requests per second (0 = disabled).
        #[arg(long, env = "WEBFIND_RATE_LIMIT")]
        rate_limit: Option<u32>,
    },

    /// Show engine status
    Status,
}

#[derive(Subcommand)]
pub enum IndexAction {
    /// Import from Common Crawl
    Import {
        /// Common Crawl dataset (e.g., CC-MAIN-2026-01)
        dataset: String,

        /// Maximum pages to import
        #[arg(long, default_value = "100000")]
        limit: u32,
    },

    /// Show index statistics
    Stats,

    /// Optimize the index
    Optimize,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum GraphStoreArg {
    Memory,
    Surrealdb,
}

impl GraphStoreArg {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphStoreArg::Memory => "memory",
            GraphStoreArg::Surrealdb => "surrealdb",
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ProxyProtocolArg {
    Http,
    Https,
    Socks5,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ReCrawlPolicyArg {
    Never,
    Fixed,
    Adaptive,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DepthArg {
    Shallow,
    Standard,
    Deep,
    Comprehensive,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputArg {
    Json,
    Report,
    Markdown,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DirectionArg {
    Inbound,
    Outbound,
    Both,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum ModeArg {
    Mcp,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum TransportArg {
    Stdio,
    Http,
}
