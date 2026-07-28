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
#[derive(Debug, Default, Deserialize)]
pub struct WebfindConfig {
    pub data_dir: Option<PathBuf>,
    pub graph_store: Option<String>,
    pub surreal: Option<SurrealConfig>,
    pub rate_limit: Option<u32>,
}

#[derive(Debug, Default, Deserialize)]
pub struct SurrealConfig {
    pub url: Option<String>,
    pub user: Option<String>,
    pub pass: Option<String>,
    pub ns: Option<String>,
    pub db: Option<String>,
}

/// Resolved SurrealDB connection settings with all fallbacks applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSurreal {
    pub url: String,
    pub user: String,
    pub pass: String,
    pub ns: String,
    pub db: String,
}

impl Default for ResolvedSurreal {
    fn default() -> Self {
        Self {
            url: "memory".to_string(),
            user: String::new(),
            pass: String::new(),
            ns: "webfind".to_string(),
            db: "webfind".to_string(),
        }
    }
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

/// Resolve SurrealDB connection settings.
pub fn resolve_surreal(
    config: &WebfindConfig,
    cli_url: Option<&str>,
    cli_user: Option<&str>,
    cli_pass: Option<&str>,
    cli_ns: Option<&str>,
    cli_db: Option<&str>,
) -> ResolvedSurreal {
    let surreal = config.surreal.as_ref();
    ResolvedSurreal {
        url: resolve(
            cli_url,
            "WEBFIND_SURREAL_URL",
            surreal.and_then(|s| s.url.as_deref()),
            "memory",
        ),
        user: resolve(
            cli_user,
            "WEBFIND_SURREAL_USER",
            surreal.and_then(|s| s.user.as_deref()),
            "",
        ),
        pass: resolve(
            cli_pass,
            "WEBFIND_SURREAL_PASS",
            surreal.and_then(|s| s.pass.as_deref()),
            "",
        ),
        ns: resolve(
            cli_ns,
            "WEBFIND_SURREAL_NS",
            surreal.and_then(|s| s.ns.as_deref()),
            "webfind",
        ),
        db: resolve(
            cli_db,
            "WEBFIND_SURREAL_DB",
            surreal.and_then(|s| s.db.as_deref()),
            "webfind",
        ),
    }
}

/// Resolve the graph-store backend name. Defaults to `"memory"`.
pub fn resolve_graph_store(config: &WebfindConfig, cli: Option<&str>) -> String {
    cli.map(std::string::ToString::to_string)
        .or_else(|| std::env::var("WEBFIND_GRAPH_STORE").ok())
        .or_else(|| config.graph_store.clone())
        .unwrap_or_else(|| "memory".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_config() {
        let text = r#"
            data_dir = "/var/webfind"
            graph_store = "surrealdb"

            [surreal]
            url = "http://localhost:7790"
            user = "root"
            pass = "root"
            ns = "prod"
            db = "prod"
        "#;
        let cfg: WebfindConfig = toml::from_str(text).unwrap();
        assert_eq!(cfg.data_dir, Some(PathBuf::from("/var/webfind")));
        assert_eq!(cfg.graph_store, Some("surrealdb".to_string()));
        let s = cfg.surreal.unwrap();
        assert_eq!(s.url, Some("http://localhost:7790".to_string()));
        assert_eq!(s.db, Some("prod".to_string()));
    }

    #[test]
    fn test_resolve_surreal_priority_cli_over_config() {
        let cfg = WebfindConfig {
            surreal: Some(SurrealConfig {
                url: Some("ws://config".to_string()),
                user: None,
                pass: None,
                ns: Some("ns".to_string()),
                db: Some("db".to_string()),
            }),
            ..Default::default()
        };
        let resolved = resolve_surreal(&cfg, Some("ws://cli"), None, None, None, None);
        assert_eq!(resolved.url, "ws://cli");
        assert_eq!(resolved.ns, "ns");
    }

    #[test]
    fn test_resolve_surreal_defaults() {
        let cfg = WebfindConfig::default();
        let resolved = resolve_surreal(&cfg, None, None, None, None, None);
        assert_eq!(resolved, ResolvedSurreal::default());
    }

    #[test]
    fn test_resolve_graph_store_defaults() {
        let cfg = WebfindConfig::default();
        assert_eq!(resolve_graph_store(&cfg, None), "memory");
    }

    #[test]
    fn test_resolve_data_dir_from_config() {
        let cfg = WebfindConfig {
            data_dir: Some(PathBuf::from("/tmp/webfind")),
            ..Default::default()
        };
        assert_eq!(resolve_data_dir(&cfg), PathBuf::from("/tmp/webfind"));
    }
}
