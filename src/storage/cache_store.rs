use std::collections::hash_map::DefaultHasher;
use std::fs::{File, OpenOptions};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Re-crawl policy for a cached URL.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ReCrawlPolicy {
    /// Never re-crawl once cached.
    Never,
    /// Fixed interval in days.
    FixedDays(u32),
    /// Adaptive: interval scales with observed change frequency.
    Adaptive,
}

impl Default for ReCrawlPolicy {
    fn default() -> Self {
        ReCrawlPolicy::FixedDays(7)
    }
}

/// Metadata kept for every URL seen by the crawler.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    /// Normalized URL used as the cache key.
    pub url: String,
    /// 64-bit hash of extracted content text (for change detection).
    pub content_hash: u64,
    /// When this URL was first seen.
    pub first_seen: DateTime<Utc>,
    /// When this URL was last successfully fetched.
    pub last_fetched: DateTime<Utc>,
    /// Number of successful fetches.
    pub fetch_count: u32,
    /// Last HTTP status code.
    pub last_status: u16,
    /// Last observed page title.
    pub title: Option<String>,
    /// Last observed content fingerprint for near-duplicate detection.
    pub simhash: Option<u64>,
}

impl CacheEntry {
    /// Compute content hash from extracted text.
    pub fn hash_content(text: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        hasher.finish()
    }
}

/// On-disk cache of seen URLs with re-crawl policy support.
pub struct CacheStore {
    path: PathBuf,
    entries: std::sync::Mutex<std::collections::HashMap<String, CacheEntry>>,
    policy: ReCrawlPolicy,
}

impl CacheStore {
    /// Open or create a cache store at `base/cache/urls.jsonl`.
    pub fn open(base: impl AsRef<Path>) -> Result<Self> {
        let path = base.as_ref().join("cache").join("urls.jsonl");
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut store = Self {
            path,
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
            policy: ReCrawlPolicy::default(),
        };
        store.load()?;
        Ok(store)
    }

    /// Set the re-crawl policy.
    pub fn with_policy(mut self, policy: ReCrawlPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// Normalize a URL for cache lookup.
    ///
    /// Strips fragments, default ports, and trailing slashes. Does not
    /// resolve relative URLs.
    pub fn normalize_url(url: &str) -> String {
        let mut u = url.trim().to_lowercase();
        if let Some(pos) = u.find('#') {
            u.truncate(pos);
        }
        u = u.trim_end_matches('/').to_string();
        // Strip default ports for http/https.
        for scheme in &["http://", "https://"] {
            if let Some(rest) = u.strip_prefix(scheme) {
                let parts: Vec<&str> = rest.splitn(2, '/').collect();
                let host = parts[0];
                if let Some(colon) = host.find(':') {
                    let port = &host[colon + 1..];
                    let default_port = if scheme == &"http://" { "80" } else { "443" };
                    if port == default_port {
                        let new_host = &host[..colon];
                        let new_rest = parts.get(1).map(|s| format!("/{}", s)).unwrap_or_default();
                        return format!("{}{}{}", scheme, new_host, new_rest);
                    }
                }
            }
        }
        u
    }

    /// Return the cached entry for a URL, if any.
    pub fn get(&self, url: &str) -> Option<CacheEntry> {
        let key = Self::normalize_url(url);
        self.entries.lock().unwrap().get(&key).cloned()
    }

    /// Insert or replace a cache entry and persist it.
    pub fn insert(&self, entry: CacheEntry) -> Result<()> {
        let key = Self::normalize_url(&entry.url);
        self.entries.lock().unwrap().insert(key, entry);
        self.persist()
    }

    /// Returns true if the URL should be fetched now according to policy.
    pub fn should_fetch(&self, url: &str) -> bool {
        match self.get(url) {
            None => true,
            Some(entry) => match self.policy {
                ReCrawlPolicy::Never => false,
                ReCrawlPolicy::FixedDays(days) => {
                    let age = Utc::now() - entry.last_fetched;
                    age > chrono::Duration::days(days as i64)
                }
                ReCrawlPolicy::Adaptive => Self::adaptive_should_fetch(&entry),
            },
        }
    }

    /// Adaptive policy: estimate change frequency and re-crawl before
    /// expected staleness. For simplicity, interval doubles each successful
    /// re-crawl up to 30 days; resets on first fetch to 1 day.
    fn adaptive_should_fetch(entry: &CacheEntry) -> bool {
        let age = Utc::now() - entry.last_fetched;
        let interval_days = if entry.fetch_count <= 1 {
            1
        } else {
            (2_i64).pow((entry.fetch_count - 1).min(4) as u32).min(30)
        };
        age > chrono::Duration::days(interval_days)
    }

    /// Total cached URLs.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn load(&mut self) -> Result<()> {
        if !self.path.exists() {
            return Ok(());
        }
        let file = File::open(&self.path)?;
        let reader = BufReader::new(file);
        let mut map = self.entries.lock().unwrap();
        for line in reader.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let entry: CacheEntry = serde_json::from_str(&line)?;
            map.insert(Self::normalize_url(&entry.url), entry);
        }
        Ok(())
    }

    fn persist(&self) -> Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)?;
        let mut writer = std::io::LineWriter::new(file);
        let map = self.entries.lock().unwrap();
        for entry in map.values() {
            let line = serde_json::to_string(entry)?;
            writeln!(writer, "{}", line)?;
        }
        writer.flush()?;
        Ok(())
    }
}

impl Default for CacheStore {
    fn default() -> Self {
        CacheStore::open(".").expect("default cache in current dir")
    }
}

/// Fast SimHash implementation for near-duplicate detection.
pub struct SimHash;

impl SimHash {
    /// Compute a 64-bit SimHash from 3-character n-grams.
    ///
    /// N-grams make the fingerprint stable even for short texts and small edits.
    pub fn compute(text: &str) -> u64 {
        let normalized: String = text
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .flat_map(|c| c.to_lowercase())
            .collect();
        let chars: Vec<char> = normalized.chars().collect();
        let mut vec = [0i64; 64];
        if chars.len() < 3 {
            return 0;
        }
        for window in chars.windows(3) {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            let h = hasher.finish();
            for i in 0..64 {
                let bit = (h >> i) & 1;
                if bit == 1 {
                    vec[i] += 1;
                } else {
                    vec[i] -= 1;
                }
            }
        }
        let mut hash = 0u64;
        for i in 0..64 {
            if vec[i] > 0 {
                hash |= 1 << i;
            }
        }
        hash
    }

    /// Count differing bits between two hashes.
    pub fn hamming_distance(a: u64, b: u64) -> u32 {
        (a ^ b).count_ones()
    }

    /// True if the two texts are near-duplicates (≤15 bit difference).
    ///
    /// The threshold is intentionally loose for short crawled excerpts; the
    /// main goal is to avoid indexing obviously identical/syndicated pages.
    pub fn is_near_duplicate(a: &str, b: &str) -> bool {
        Self::hamming_distance(Self::compute(a), Self::compute(b)) <= 15
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_normalize_url() {
        assert_eq!(
            CacheStore::normalize_url("https://example.com/page#section"),
            "https://example.com/page"
        );
        assert_eq!(
            CacheStore::normalize_url("https://example.com:443/page/"),
            "https://example.com/page"
        );
        assert_eq!(
            CacheStore::normalize_url("http://example.com:80/page"),
            "http://example.com/page"
        );
    }

    #[test]
    fn test_cache_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cache = CacheStore::open(tmp.path()).unwrap();

        assert!(cache.should_fetch("https://example.com/a"));

        let entry = CacheEntry {
            url: "https://example.com/a".to_string(),
            content_hash: 123,
            first_seen: Utc::now(),
            last_fetched: Utc::now(),
            fetch_count: 1,
            last_status: 200,
            title: Some("Hello".to_string()),
            simhash: Some(0),
        };
        cache.insert(entry.clone()).unwrap();

        assert_eq!(cache.len(), 1);
        assert!(!cache.should_fetch("https://example.com/a"));

        // Re-open should persist.
        let cache2 = CacheStore::open(tmp.path()).unwrap();
        assert_eq!(cache2.len(), 1);
        assert_eq!(
            cache2.get("https://example.com/a").unwrap().content_hash,
            123
        );
    }

    #[test]
    fn test_fixed_policy_blocks_recent() {
        let tmp = TempDir::new().unwrap();
        let cache = CacheStore::open(tmp.path())
            .unwrap()
            .with_policy(ReCrawlPolicy::FixedDays(7));

        let entry = CacheEntry {
            url: "https://example.com/a".to_string(),
            content_hash: 0,
            first_seen: Utc::now(),
            last_fetched: Utc::now(),
            fetch_count: 1,
            last_status: 200,
            title: None,
            simhash: None,
        };
        cache.insert(entry).unwrap();
        assert!(!cache.should_fetch("https://example.com/a"));
    }

    #[test]
    fn test_never_policy() {
        let tmp = TempDir::new().unwrap();
        let cache = CacheStore::open(tmp.path())
            .unwrap()
            .with_policy(ReCrawlPolicy::Never);

        let entry = CacheEntry {
            url: "https://example.com/old".to_string(),
            content_hash: 0,
            first_seen: Utc::now() - chrono::Duration::days(400),
            last_fetched: Utc::now() - chrono::Duration::days(365),
            fetch_count: 1,
            last_status: 200,
            title: None,
            simhash: None,
        };
        cache.insert(entry).unwrap();
        assert!(!cache.should_fetch("https://example.com/old"));
    }

    #[test]
    fn test_simhash_near_duplicate() {
        let a = "Rust is a systems programming language focused on safety";
        let b = "Rust is a systems programming language focused on safety and speed";
        let c = "Python is a high level scripting language for web development";
        assert!(SimHash::is_near_duplicate(a, b));
        assert!(!SimHash::is_near_duplicate(a, c));
    }
}
