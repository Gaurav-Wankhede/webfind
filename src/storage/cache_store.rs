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
        std::fs::create_dir_all(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("cache path has no parent: {:?}", path))?,
        )?;
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
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .cloned()
    }

    /// Insert or replace a cache entry and persist it.
    pub fn insert(&self, entry: CacheEntry) -> Result<()> {
        let key = Self::normalize_url(&entry.url);
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, entry);
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
            (2_i64).pow((entry.fetch_count - 1).min(4)).min(30)
        };
        age > chrono::Duration::days(interval_days)
    }

    /// Total cached URLs.
    pub fn len(&self) -> usize {
        self.entries.lock().unwrap_or_else(|e| e.into_inner()).len()
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
        let mut map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
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
        let map = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        for entry in map.values() {
            let line = serde_json::to_string(entry)?;
            writeln!(writer, "{}", line)?;
        }
        writer.flush()?;
        Ok(())
    }
}

/// Fast SimHash implementation for near-duplicate detection.
pub struct SimHash;

impl SimHash {
    /// Hamming-distance threshold for near-duplicate classification.
    ///
    /// Charikar (2002) introduced SimHash for near-duplicate detection; Manku
    /// et al. (2007) validated a threshold of 3 bits over 64-bit fingerprints
    /// at 8-billion-page scale. Higher values admit genuine duplicates.
    /// Sources: https://hld.handbook.academy/curriculum/case-studies/web-crawler/
    ///          https://ijasrm.com/wp-content/uploads/2019/05/IJASRM_V4S5_1458_122_125.pdf
    pub const NEAR_DUP_THRESHOLD: u32 = 3;

    /// Compute a 64-bit SimHash from 3-word shingles (k=3 words).
    ///
    /// Word-level shingling (Charikar 2002; Manku et al. 2007) — not character
    /// n-grams — is what makes the ≤3-bit near-duplicate threshold valid; char
    /// trigrams are noisy enough that a single edit exceeds 3 bits on short text.
    /// Unweighted (classic Charikar) vote; TF-weighting would shift the bit
    /// distribution and invalidate the calibrated threshold.
    pub fn compute(text: &str) -> u64 {
        const SHINGLE_K: usize = 3;
        let words: Vec<String> = text
            .split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| !w.is_empty())
            .collect();
        let mut vec = [0i64; 64];
        if words.len() < SHINGLE_K {
            return 0;
        }
        for window in words.windows(SHINGLE_K) {
            let mut hasher = DefaultHasher::new();
            window.hash(&mut hasher);
            let h = hasher.finish();
            for (i, v) in vec.iter_mut().enumerate() {
                let bit = (h >> i) & 1;
                if bit == 1 {
                    *v += 1;
                } else {
                    *v -= 1;
                }
            }
        }
        let mut hash = 0u64;
        for (i, &v) in vec.iter().enumerate() {
            if v > 0 {
                hash |= 1 << i;
            }
        }
        hash
    }

    /// Count differing bits between two hashes.
    pub fn hamming_distance(a: u64, b: u64) -> u32 {
        (a ^ b).count_ones()
    }

    /// True if the two texts are near-duplicates (≤ [`Self::NEAR_DUP_THRESHOLD`] bit difference).
    ///
    /// Threshold fixed to the Manku et al. (2007) validated value; see the
    /// const docs for citations. Boilerplate-strip-before-hash mitigates
    /// stop-word false positives (https://sharief007.github.io/dev-docs/system-design/web-crawler/frontier-deep-dive/).
    pub fn is_near_duplicate(a: &str, b: &str) -> bool {
        Self::hamming_distance(Self::compute(a), Self::compute(b)) <= Self::NEAR_DUP_THRESHOLD
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Realistic-length prose (~40 words) for SimHash distance tests. The ≤3-bit
    /// threshold is calibrated for full documents, not short strings — a one-word
    /// edit must be a tiny fraction of total shingles.
    const BASE_PROSE: &str = "the distributed search engine indexes crawled web pages across \
        many hosts and stores their content in a local database so that agents can \
        query it later without hitting the network again every single time which \
        saves bandwidth and improves latency for repeated research tasks over time";

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
        // Realistic-length document (~150 words): a one-word edit is a tiny
        // fraction of total shingles, so it stays within the 3-bit threshold.
        let a = format!("{} jumped", BASE_PROSE);
        let b = format!("{} leapt", BASE_PROSE);
        let c = "Python is a high level scripting language for web development with \
                 dynamic typing automatic memory management and an extensive standard \
                 library covering web data science and automation tasks for beginners".to_string();
        let d = SimHash::hamming_distance(SimHash::compute(&a), SimHash::compute(&b));
        assert!(
            SimHash::is_near_duplicate(&a, &b),
            "one-word edit on a real document must stay within {} bits (dist={d})",
            SimHash::NEAR_DUP_THRESHOLD
        );
        assert!(!SimHash::is_near_duplicate(&a, &c), "distinct doc must not be a near-dup");
        assert!(d <= SimHash::NEAR_DUP_THRESHOLD, "near-dup distance {d} within threshold");
    }
}
