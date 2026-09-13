//! Reddit-aware fetch support.
//!
//! Handles URL detection, `.json` endpoint rewriting, and listing/post parsing
//! for Reddit domains.  Reddit blocks anonymous `.json` requests as of 2025,
//! so we fall back to old.reddit.com HTML extraction when JSON fails.
//!
//! Design principle: **handle Reddit as a genuine person.**  Descriptive
//! User-Agent, respectful rate limits, no spam.

use std::time::Duration;

use anyhow::{Context, Result};

/// Descriptive User-Agent for Reddit requests.
///
/// Reddit's API etiquette requires a recognizable UA identifying the bot
/// operator.  We use a descriptive, non-deceptive string that identifies
/// WebFind as a research tool.
pub const REDDIT_USER_AGENT: &str =
    "WebFind/0.1 (research crawler; +https://github.com/Gaurav-Wankhede/webfind)";

/// Minimum delay between Reddit requests (respect rate limits).
///
/// Reddit allows ~60 requests/minute for authenticated OAuth, but anonymous
/// access is stricter — 10 requests/minute is the safe floor.
pub const REDDIT_RATE_LIMIT: Duration = Duration::from_secs(6);

/// Known Reddit host patterns (without `www.` prefix).
const REDDIT_HOSTS: &[&str] = &["reddit.com", "old.reddit.com", "new.reddit.com"];

/// Check whether a URL belongs to Reddit.
pub fn is_reddit_url(url: &str) -> bool {
    REDDIT_HOSTS
        .iter()
        .any(|host| url.contains(&format!("://{}", host)) || url.contains(&format!(".{}", host)))
}

/// Rewrite a Reddit URL to its `.json` endpoint for structured extraction.
///
/// - `https://www.reddit.com/r/rust` → `https://www.reddit.com/r/rust.json`
/// - `https://old.reddit.com/r/rust/comments/abc123` → `https://old.reddit.com/r/rust/comments/abc123.json`
/// - Already has `.json` → returned as-is
/// - URLs with query params → params stripped, `.json` appended before `?`
///
/// Returns `None` if the URL is not a Reddit URL.
pub fn to_reddit_json_url(url: &str) -> Option<String> {
    if !is_reddit_url(url) {
        return None;
    }

    // Already a JSON endpoint
    if url.ends_with(".json") || url.contains(".json?") {
        return Some(url.to_string());
    }

    // Strip trailing slash
    let base = url.trim_end_matches('/');

    // Strip query params — Reddit JSON doesn't honor them well
    let base_no_query = match base.find('?') {
        Some(pos) => &base[..pos],
        None => base,
    };

    Some(format!("{}.json", base_no_query))
}

/// Rewrite a Reddit URL to old.reddit.com for HTML fallback extraction.
///
/// Old Reddit returns cleaner HTML that's easier to parse without JS.
pub fn to_old_reddit_url(url: &str) -> String {
    // Replace any reddit.com host with old.reddit.com
    url.replace("://www.reddit.com", "://old.reddit.com")
        .replace("://new.reddit.com", "://old.reddit.com")
        .replace("://reddit.com", "://old.reddit.com")
}

/// A single item extracted from a Reddit listing JSON.
#[derive(Debug, Clone)]
pub struct RedditListingItem {
    /// Post or subreddit title.
    pub title: String,
    /// Full URL to the post or subreddit.
    pub url: String,
    /// Post permalink (relative).
    pub permalink: String,
    /// Subreddit name (e.g. "rust").
    pub subreddit: String,
    /// Author username.
    pub author: String,
    /// Self-text for self posts.
    pub selftext: String,
    /// External link URL for link posts.
    pub domain: String,
    /// Score (upvotes - downvotes).
    pub score: i64,
    /// Number of comments.
    pub num_comments: i64,
    /// Created UTC timestamp.
    pub created_utc: Option<f64>,
}

/// Parse a Reddit listing JSON response into structured items.
///
/// Handles both `/r/{subreddit}` listing pages and `/r/{subreddit}/comments/{id}` post pages.
pub fn parse_reddit_listing(json_str: &str) -> Result<Vec<RedditListingItem>> {
    let value: serde_json::Value =
        serde_json::from_str(json_str).context("failed to parse Reddit JSON")?;

    let mut items = Vec::new();

    match &value {
        // Listing page: an array with `data.children` containing posts
        serde_json::Value::Array(arr) if !arr.is_empty() => {
            // First element is the listing data
            if let Some(data) = arr[0].get("data")
                && let Some(children) = data.get("children")
                    && let Some(children_arr) = children.as_array() {
                        for child in children_arr {
                            if let Some(item) = parse_listing_child(child) {
                                items.push(item);
                            }
                        }
                    }
        }
        // Single post: has `data.children` with one post and optionally `data.children` in [1] for comments
        serde_json::Value::Object(_) => {
            if let Some(data) = value.get("data")
                && let Some(children) = data.get("children")
                    && let Some(children_arr) = children.as_array() {
                        for child in children_arr {
                            if let Some(item) = parse_listing_child(child) {
                                items.push(item);
                            }
                        }
                    }
        }
        _ => {}
    }

    Ok(items)
}

/// Extract text content from a Reddit listing into a single document.
///
/// This combines title, selftext, and metadata into a readable format
/// suitable for StructuredContent extraction.
pub fn listing_to_text(items: &[RedditListingItem]) -> String {
    let mut out = String::new();

    for (i, item) in items.iter().enumerate() {
        if i > 0 {
            out.push_str("\n\n---\n\n");
        }

        // Header: subreddit + title
        out.push_str(&format!("r/{} — {}\n", item.subreddit, item.title));

        // Author
        out.push_str(&format!(
            "u/{} · {} points · {} comments\n",
            item.author, item.score, item.num_comments
        ));

        // Content
        if !item.selftext.is_empty() {
            out.push_str(&item.selftext);
        } else if !item.domain.is_empty() {
            out.push_str(&format!("[Link: {}]", item.domain));
        }
    }

    out
}

fn parse_listing_child(child: &serde_json::Value) -> Option<RedditListingItem> {
    let kind = child.get("kind").and_then(|k| k.as_str())?;
    // Only process posts (t3) and subreddit listings (t5)
    if kind != "t3" && kind != "t5" {
        return None;
    }

    let data = child.get("data")?;

    let title = data
        .get("title")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .to_string();
    let permalink = data
        .get("permalink")
        .and_then(|p| p.as_str())
        .unwrap_or("")
        .to_string();
    let url = format!("https://www.reddit.com{}", permalink);
    let subreddit = data
        .get("subreddit")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let author = data
        .get("author")
        .and_then(|a| a.as_str())
        .unwrap_or("[deleted]")
        .to_string();
    let selftext = data
        .get("selftext")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    let domain = data
        .get("domain")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();
    let score = data.get("score").and_then(|s| s.as_i64()).unwrap_or(0);
    let num_comments = data
        .get("num_comments")
        .and_then(|c| c.as_i64())
        .unwrap_or(0);
    let created_utc = data.get("created_utc").and_then(|c| c.as_f64());

    Some(RedditListingItem {
        title,
        url,
        permalink,
        subreddit,
        author,
        selftext,
        domain,
        score,
        num_comments,
        created_utc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_reddit_url() {
        assert!(is_reddit_url("https://www.reddit.com/r/rust"));
        assert!(is_reddit_url("https://old.reddit.com/r/rust"));
        assert!(is_reddit_url("https://new.reddit.com/r/rust"));
        assert!(!is_reddit_url("https://github.com/rust-lang"));
        assert!(!is_reddit_url("https://example.com"));
    }

    #[test]
    fn test_to_reddit_json_url() {
        assert_eq!(
            to_reddit_json_url("https://www.reddit.com/r/rust").unwrap(),
            "https://www.reddit.com/r/rust.json"
        );
        assert_eq!(
            to_reddit_json_url("https://www.reddit.com/r/rust/comments/abc123").unwrap(),
            "https://www.reddit.com/r/rust/comments/abc123.json"
        );
        // Already .json
        assert_eq!(
            to_reddit_json_url("https://www.reddit.com/r/rust.json").unwrap(),
            "https://www.reddit.com/r/rust.json"
        );
        // Trailing slash stripped
        assert_eq!(
            to_reddit_json_url("https://www.reddit.com/r/rust/").unwrap(),
            "https://www.reddit.com/r/rust.json"
        );
        // Query params stripped
        assert_eq!(
            to_reddit_json_url("https://www.reddit.com/r/rust?limit=25").unwrap(),
            "https://www.reddit.com/r/rust.json"
        );
        // Non-Reddit URL
        assert!(to_reddit_json_url("https://github.com/rust-lang").is_none());
    }

    #[test]
    fn test_to_old_reddit_url() {
        assert_eq!(
            to_old_reddit_url("https://www.reddit.com/r/rust"),
            "https://old.reddit.com/r/rust"
        );
        assert_eq!(
            to_old_reddit_url("https://new.reddit.com/r/rust"),
            "https://old.reddit.com/r/rust"
        );
        assert_eq!(
            to_old_reddit_url("https://reddit.com/r/rust"),
            "https://old.reddit.com/r/rust"
        );
    }

    #[test]
    fn test_parse_reddit_listing_empty_array() {
        let json = r#"[]"#;
        let items = parse_reddit_listing(json).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_parse_reddit_listing_with_posts() {
        let json = r#"{
            "data": {
                "children": [
                    {
                        "kind": "t3",
                        "data": {
                            "title": "Hello from WebFind",
                            "permalink": "/r/rust/comments/abc123/hello_from_webfind/",
                            "subreddit": "rust",
                            "author": "testuser",
                            "selftext": "This is a test post body.",
                            "domain": "self.rust",
                            "score": 42,
                            "num_comments": 7,
                            "created_utc": 1700000000.0
                        }
                    }
                ]
            }
        }"#;
        let items = parse_reddit_listing(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Hello from WebFind");
        assert_eq!(items[0].subreddit, "rust");
        assert_eq!(items[0].score, 42);
        assert_eq!(items[0].num_comments, 7);
        assert!(items[0].selftext.contains("test post body"));
    }

    #[test]
    fn test_parse_reddit_listing_array_format() {
        let json = r#"[
            {
                "data": {
                    "children": [
                        {
                            "kind": "t3",
                            "data": {
                                "title": "Listing Post",
                                "permalink": "/r/rust/comments/xyz/listing_post/",
                                "subreddit": "rust",
                                "author": "alice",
                                "selftext": "",
                                "domain": "github.com",
                                "score": 10,
                                "num_comments": 2
                            }
                        }
                    ]
                }
            }
        ]"#;
        let items = parse_reddit_listing(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Listing Post");
    }

    #[test]
    fn test_listing_to_text() {
        let items = vec![
            RedditListingItem {
                title: "Post One".to_string(),
                url: "https://www.reddit.com/r/rust/comments/1/post_one/".to_string(),
                permalink: "/r/rust/comments/1/post_one/".to_string(),
                subreddit: "rust".to_string(),
                author: "alice".to_string(),
                selftext: "Body text here.".to_string(),
                domain: "self.rust".to_string(),
                score: 5,
                num_comments: 1,
                created_utc: None,
            },
            RedditListingItem {
                title: "Post Two".to_string(),
                url: "https://www.reddit.com/r/rust/comments/2/post_two/".to_string(),
                permalink: "/r/rust/comments/2/post_two/".to_string(),
                subreddit: "rust".to_string(),
                author: "bob".to_string(),
                selftext: "".to_string(),
                domain: "github.com".to_string(),
                score: 3,
                num_comments: 0,
                created_utc: None,
            },
        ];
        let text = listing_to_text(&items);
        assert!(text.contains("r/rust — Post One"));
        assert!(text.contains("Body text here."));
        assert!(text.contains("r/rust — Post Two"));
        assert!(text.contains("[Link: github.com]"));
        assert!(text.contains("---"));
    }

    #[test]
    fn test_non_t3_children_are_skipped() {
        let json = r#"{
            "data": {
                "children": [
                    {
                        "kind": "t1",
                        "data": {"title": "comment, not post"}
                    },
                    {
                        "kind": "t3",
                        "data": {
                            "title": "actual post",
                            "permalink": "/r/rust/comments/x/actual_post/",
                            "subreddit": "rust",
                            "author": "bob",
                            "selftext": "",
                            "domain": "self.rust",
                            "score": 1,
                            "num_comments": 0
                        }
                    }
                ]
            }
        }"#;
        let items = parse_reddit_listing(json).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "actual post");
    }
}
