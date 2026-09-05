use clap::{Parser, Subcommand, ValueEnum};

use crate::schema::request::{OutputFormat, SearchDepth};
use crate::storage::cache_store::ReCrawlPolicy;

#[derive(Parser)]
#[command(
    name = "webfind",
    about = "Self-hosted native search engine for AI agents — pure CLI, zero cost",
    version,
    next_display_order = None,
    subcommand_required = false
)]
pub struct Cli {
    /// Print the AI agent skills reference (progressive disclosure, ~800 tokens) and exit
    #[arg(long, hide = true)]
    pub print_skills: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
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

        /// Merge live search-engine results (DuckDuckGo, Bing) with the index
        /// via RRF. Live-only results carry title/snippet but no stored content.
        #[arg(long, default_value = "false")]
        live: bool,

        /// Graph store to load PageRank from for ranking.
        #[arg(long, value_enum, env = "WEBFIND_GRAPH_STORE")]
        graph_store: Option<GraphStoreArg>,

        /// Path to the Turso/libSQL database file (when --graph-store turso).
        #[arg(long)]
        turso_path: Option<String>,
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
        /// Seed URL to start crawling from. Optional when --daemon is used
        /// (the curated seed catalog supplies the sources instead).
        #[arg(long)]
        seed: Option<String>,

        /// Run as a background curation daemon over the curated seed catalog.
        #[arg(long, default_value = "false")]
        daemon: bool,

        /// Domains to curate (comma-separated slugs). Default: all curated.
        #[arg(long)]
        domains: Option<String>,

        /// Max pages to fetch per source per daemon sweep.
        #[arg(long, default_value = "50")]
        daemon_pages: u32,

        /// Seconds to pause between daemon catalog sweeps.
        #[arg(long, default_value = "3600")]
        daemon_interval: u64,

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

        /// Path to the Turso/libSQL database file (when --graph-store turso).
        #[arg(long)]
        turso_path: Option<String>,

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
        #[arg(long, default_value = "0")]
        min_depth: u32,

        /// Maximum hop depth from seed before stopping discovery.
        #[arg(long, default_value = "5")]
        max_depth: u32,

        /// Auto-map crawl depth from the site's own map (sitemap/llms.txt)
        /// and keep link-following inside the site's declared content surface.
        #[arg(long, default_value = "true")]
        auto_depth: bool,

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

        /// Graph store backend to traverse.
        #[arg(long, value_enum)]
        graph_store: Option<GraphStoreArg>,

        /// Path to the Turso/libSQL database file (when --graph-store turso).
        #[arg(long)]
        turso_path: Option<String>,
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
        /// Seed URL to start crawling from. If omitted, WebFind auto-discovers seeds.
        #[arg(long)]
        seed: Option<String>,

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

        /// Include full page content in each result. Enabled by default: this
        /// is the primary web-research path for agents, and they need the
        /// scraped body text (the MCP/CLI previously defaulted this off, so
        /// research returned content: null and agents fell back to other
        /// engines). Pass --include-content=false for tiny responses.
        #[arg(long, default_value = "true")]
        include_content: bool,

        /// Follow external (cross-domain) links during crawl.
        #[arg(long, default_value = "false")]
        follow_external: bool,

        /// Minimum hop depth to force-fetch (bypasses budget).
        #[arg(long, default_value = "0")]
        min_depth: u32,

        /// Maximum hop depth from seed before stopping discovery.
        #[arg(long, default_value = "5")]
        max_depth: u32,

        /// Auto-map crawl depth from the site's own map (sitemap/llms.txt)
        /// and keep link-following inside the site's declared content surface.
        #[arg(long, default_value = "true")]
        auto_depth: bool,

        /// Query topics for content-aware link prioritization (comma-separated).
        #[arg(long)]
        topics: Option<String>,

        /// Additional seed URLs for multi-seed crawling (comma-separated).
        #[arg(long)]
        seeds: Option<String>,

        /// Render JS-heavy / bot-protected pages in headless Chromium (CDP)
        /// when a plain HTTP fetch yields no meaningful content. Enables
        /// stealth mode so bot-protected sites serve real content.
        #[arg(long, default_value = "false")]
        dynamic: bool,

        /// Deep research mode: no crawl deadline and no backoff caps, and use
        /// the full CDP browser (stealth + infinite-scroll) to capture
        /// progressively rendered pages. Intended for long-running, thorough
        /// investigations; may run for many minutes.
        #[arg(long, default_value = "false")]
        deep: bool,

        /// Write the JSON result to this file instead of stdout. Agents read
        /// the file with their file-read tool — no shell/Python parsing needed.
        #[arg(long)]
        output: Option<std::path::PathBuf>,

        /// Graph store backend to persist crawled records into (default: turso).
        #[arg(long, value_enum, env = "WEBFIND_GRAPH_STORE")]
        graph_store: Option<GraphStoreArg>,

        /// Path to the Turso/libSQL database file (when --graph-store turso).
        /// Every crawled record is persisted here for graph-memory awareness.
        #[arg(long)]
        turso_path: Option<String>,
    },

    /// Start the HTTP API + Web UI server (pure CLI / HTTP, no MCP)
    Serve {
        /// Port for HTTP transport (default: 4747)
        #[arg(long, default_value = "4747")]
        port: u16,

        /// Graph store to load PageRank from for ranking.
        #[arg(long, value_enum, env = "WEBFIND_GRAPH_STORE")]
        graph_store: Option<GraphStoreArg>,

        /// Path to the Turso/libSQL database file (when --graph-store turso).
        #[arg(long)]
        turso_path: Option<String>,

        /// Enable BM25 + vector hybrid re-ranking by default.
        #[arg(long, default_value = "false")]
        hybrid: bool,

        /// Optional per-IP rate limit in requests per second (0 = disabled).
        #[arg(long, env = "WEBFIND_RATE_LIMIT")]
        rate_limit: Option<u32>,

        /// Port for the HTML GUI server (default: 4749).
        #[arg(long, default_value = "4749", env = "WEBFIND_GUI_PORT")]
        gui_port: u16,
    },

    /// Migrate a legacy SurrealDB JSON export into a fresh Turso database
    Migrate {
        /// Path to the JSON export file (url_nodes + link_edges).
        #[arg(long, value_name = "EXPORT_JSON")]
        from: std::path::PathBuf,

        /// Destination Turso/libSQL database file (created or overwritten).
        #[arg(long, value_name = "TURSO_DB")]
        to: std::path::PathBuf,
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

    /// List the curated seed catalog (non-Wikipedia, official sources per domain)
    Domains,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum GraphStoreArg {
    Memory,
    Turso,
}

impl GraphStoreArg {
    pub fn as_str(&self) -> &'static str {
        match self {
            GraphStoreArg::Memory => "memory",
            GraphStoreArg::Turso => "turso",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "turso" => Some(GraphStoreArg::Turso),
            "memory" => Some(GraphStoreArg::Memory),
            _ => None,
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

impl ReCrawlPolicyArg {
    pub fn to_policy(&self, days: u32) -> ReCrawlPolicy {
        match self {
            ReCrawlPolicyArg::Never => ReCrawlPolicy::Never,
            ReCrawlPolicyArg::Fixed => ReCrawlPolicy::FixedDays(days),
            ReCrawlPolicyArg::Adaptive => ReCrawlPolicy::Adaptive,
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DepthArg {
    Shallow,
    Standard,
    Deep,
    Comprehensive,
}

impl From<DepthArg> for SearchDepth {
    fn from(d: DepthArg) -> Self {
        match d {
            DepthArg::Shallow => SearchDepth::Shallow,
            DepthArg::Standard => SearchDepth::Standard,
            DepthArg::Deep => SearchDepth::Deep,
            DepthArg::Comprehensive => SearchDepth::Comprehensive,
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum OutputArg {
    Json,
    Report,
    Markdown,
}

impl From<OutputArg> for OutputFormat {
    fn from(o: OutputArg) -> Self {
        match o {
            OutputArg::Json => OutputFormat::Json,
            OutputArg::Report => OutputFormat::Report,
            OutputArg::Markdown => OutputFormat::Markdown,
        }
    }
}

#[derive(ValueEnum, Clone, Debug)]
pub enum DirectionArg {
    Inbound,
    Outbound,
    Both,
}
