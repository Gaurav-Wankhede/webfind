use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;

static CONFIG: OnceLock<WebfindConfig> = OnceLock::new();

/// Initialize and return the loaded configuration.
pub fn init() -> &'static WebfindConfig {
    CONFIG.get_or_init(|| load().unwrap_or_default())
}

/// Resolve and return the data directory.
pub fn data_dir() -> PathBuf {
    resolve_data_dir(init())
}

/// Project-level configuration for webfind.
///
/// Loaded from `webfind.toml` in the current directory, or from the path
/// specified by `WEBFIND_CONFIG`. CLI flags and environment variables take
/// precedence over config values.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct WebfindConfig {
    pub data_dir: Option<PathBuf>,
    pub graph_store: Option<String>,
    pub turso: Option<TursoConfig>,
    pub rate_limit: Option<u32>,
    /// Maximum request body size in bytes (default: 1MB)
    pub body_limit: Option<usize>,
    /// Comma-separated list of allowed CORS origins (default: none — must be explicitly configured)
    pub cors_origins: Option<String>,
    /// Storage budget / retention tuning for limited-disk deployments.
    pub storage: Option<StorageConfig>,
}

/// Storage budget & retention policy for the embedded database.
///
/// WebFind keeps a small, searchable core per page (embedding + excerpt +
/// entities + link edges) and, when a budget is set, evicts the bulky full
/// content (text/markdown/html) for the oldest / lowest-value pages. This
/// bounds disk usage while preserving search recall — the core requirement for
/// running a background crawl daemon on a machine with limited storage.
#[derive(Debug, Default, Deserialize, Clone)]
pub struct StorageConfig {
    /// Hard cap on total database file size in bytes. When the on-disk size
    /// exceeds this, the daemon/prune path evicts full content for the oldest
    /// pages until the DB fits under the budget. `None` = unbounded.
    pub max_bytes: Option<u64>,
    /// Number of pages whose full content is kept before eviction kicks in.
    /// `None` = unlimited full-content retention.
    pub max_full_content_pages: Option<u64>,
}

/// Resolve the storage budget (bytes) from env → config → default (None = unbounded).
pub fn resolve_storage_max_bytes(config: &WebfindConfig) -> Option<u64> {
    std::env::var("WEBFIND_STORAGE_MAX_BYTES")
        .ok()
        .and_then(|s| parse_bytes(&s))
        .or_else(|| config.storage.as_ref().and_then(|s| s.max_bytes))
}

/// Resolve the full-content page cap from env → config → default (None = unlimited).
pub fn resolve_max_full_content_pages(config: &WebfindConfig) -> Option<u64> {
    std::env::var("WEBFIND_STORAGE_MAX_FULL_CONTENT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .or_else(|| {
            config
                .storage
                .as_ref()
                .and_then(|s| s.max_full_content_pages)
        })
}

/// Parse a human-friendly byte size (e.g. "500MB", "2GB", "1500KB", "1048576").
fn parse_bytes(s: &str) -> Option<u64> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = s.strip_suffix("gb") {
        (n, 1024u64.pow(3))
    } else if let Some(n) = s.strip_suffix("mb") {
        (n, 1024u64.pow(2))
    } else if let Some(n) = s.strip_suffix("kb") {
        (n, 1024)
    } else if let Some(n) = s.strip_suffix("b") {
        (n, 1)
    } else {
        (s.as_str(), 1)
    };
    num.trim()
        .parse::<f64>()
        .ok()
        .map(|v| (v * mult as f64) as u64)
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct TursoConfig {
    pub path: Option<String>,
    /// Encryption key for the Turso/libSQL database at rest (SQLCipher AES-256-CBC).
    /// When set, every Turso database opened by WebFind is encrypted with this key.
    /// Expected as a raw UTF-8 passphrase (derived to a 256-bit key by SQLCipher).
    pub encryption_key: Option<String>,
}

/// Load configuration from disk, returning defaults if no file exists.
pub fn load() -> anyhow::Result<WebfindConfig> {
    let path = std::env::var("WEBFIND_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("webfind.toml"));

    if !path.exists() {
        return Ok(WebfindConfig::default());
    }

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("failed to read config {}: {}", path.display(), e))?;

    let config: WebfindConfig = toml::from_str(&contents)
        .map_err(|e| anyhow::anyhow!("failed to parse config {}: {}", path.display(), e))?;

    Ok(config)
}

/// Resolve a single value from CLI → env → config → default.
fn resolve(cli: Option<&str>, env_key: &str, config: Option<&str>, default: &str) -> String {
    cli.map(std::string::ToString::to_string)
        .or_else(|| std::env::var(env_key).ok())
        .or_else(|| config.map(std::string::ToString::to_string))
        .unwrap_or_else(|| default.to_string())
}

/// Resolve the Turso/libSQL database file path (CLI → env → config → default).
pub fn resolve_turso(config: &WebfindConfig, cli_path: Option<&str>) -> String {
    resolve(
        cli_path,
        "WEBFIND_TURSO_PATH",
        config.turso.as_ref().and_then(|t| t.path.as_deref()),
        "webfind.db",
    )
}

use crate::cli::GraphStoreArg;

/// Resolve the graph-store backend name. Defaults to Turso (embedded file).
pub fn resolve_graph_store(config: &WebfindConfig, cli: Option<GraphStoreArg>) -> GraphStoreArg {
    cli.or_else(|| {
        std::env::var("WEBFIND_GRAPH_STORE")
            .ok()
            .and_then(|s| s.parse().ok())
    })
    .or_else(|| {
        config
            .graph_store
            .as_ref()
            .and_then(|s| s.parse().ok())
    })
    .unwrap_or(GraphStoreArg::Turso)
}

/// Resolve the data directory path. Defaults to the current working directory.
pub fn resolve_data_dir(config: &WebfindConfig) -> PathBuf {
    config
        .data_dir
        .clone()
        .or_else(|| std::env::var("WEBFIND_DATA_DIR").map(PathBuf::from).ok())
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Resolve requests-per-second rate limit from CLI → env → config.
pub fn resolve_rate_limit(config: &WebfindConfig, cli: Option<u32>) -> Option<NonZeroU32> {
    cli.or(config.rate_limit)
        .or_else(|| {
            std::env::var("WEBFIND_RATE_LIMIT")
                .ok()
                .and_then(|s| s.parse().ok())
        })
        .and_then(NonZeroU32::new)
}

/// Resolve request body limit from env → config → default (1MB for API).
pub fn resolve_body_limit(config: &WebfindConfig) -> usize {
    std::env::var("WEBFIND_BODY_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .or(config.body_limit)
        .unwrap_or(1024 * 1024)
}

/// Resolve CORS origins from env → config → default (empty = restrictive).
pub fn resolve_cors_origins(config: &WebfindConfig) -> Vec<String> {
    std::env::var("WEBFIND_CORS_ORIGINS")
        .ok()
        .or_else(|| config.cors_origins.clone())
        .map(|s| {
            s.split(',')
                .map(|o| o.trim().to_string())
                .filter(|o| !o.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let text = r#"
            data_dir = "/var/webfind"
            graph_store = "turso"

            [turso]
            path = "/var/webfind/webfind.db"
        "#;
        let cfg: WebfindConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.data_dir, Some(PathBuf::from("/var/webfind")));
        assert_eq!(cfg.graph_store, Some("turso".to_string()));
        let t = cfg.turso.unwrap();
        assert_eq!(t.path, Some("/var/webfind/webfind.db".to_string()));
    }

    #[test]
    fn test_resolve_turso_priority_cli_over_config() {
        let cfg = WebfindConfig {
            turso: Some(TursoConfig {
                path: Some("/config/db".to_string()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(resolve_turso(&cfg, Some("/cli/db")), "/cli/db");
        assert_eq!(resolve_turso(&cfg, None), "/config/db");
    }

    #[test]
    fn test_resolve_graph_store_defaults() {
        let cfg = WebfindConfig::default();
        assert_eq!(resolve_graph_store(&cfg, None), GraphStoreArg::Turso);

        // CLI arg overrides env var and config
        assert_eq!(
            resolve_graph_store(&cfg, Some(GraphStoreArg::Memory)),
            GraphStoreArg::Memory
        );
    }

    #[test]
    fn test_resolve_data_dir_from_config() {
        let cfg = WebfindConfig {
            data_dir: Some(PathBuf::from("/tmp/webfind")),
            ..Default::default()
        };
        assert_eq!(resolve_data_dir(&cfg), PathBuf::from("/tmp/webfind"));
    }

    #[test]
    fn test_parse_bytes() {
        assert_eq!(parse_bytes("500MB"), Some(500 * 1024 * 1024));
        assert_eq!(parse_bytes("2GB"), Some(2 * 1024 * 1024 * 1024));
        assert_eq!(parse_bytes("1500KB"), Some(1500 * 1024));
        assert_eq!(parse_bytes("1048576"), Some(1_048_576));
        assert_eq!(parse_bytes("512B"), Some(512));
        assert_eq!(parse_bytes("garbage"), None);
    }

    #[test]
    fn test_resolve_storage_from_config() {
        let cfg = WebfindConfig {
            storage: Some(StorageConfig {
                max_bytes: Some(500 * 1024 * 1024),
                max_full_content_pages: Some(1000),
            }),
            ..Default::default()
        };
        assert_eq!(resolve_storage_max_bytes(&cfg), Some(500 * 1024 * 1024));
        assert_eq!(resolve_max_full_content_pages(&cfg), Some(1000));
    }
}
