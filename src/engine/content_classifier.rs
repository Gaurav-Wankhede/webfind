use crate::schema::content::StructuredContent;

/// Classify a page's content type into a human-readable label.
///
/// Returns one of: "text", "images", "videos", "news", "documentation"
pub fn classify_content_type(content: &StructuredContent) -> &str {
    // 1. Check raw HTTP Content-Type header
    let header = content.content_type_header.to_lowercase();

    if header.starts_with("image/") {
        return "images";
    }
    if header.starts_with("video/") {
        return "videos";
    }
    if header.starts_with("audio/") {
        return "videos";
    }

    // 2. Check URL extension patterns
    let url_lower = content.url.to_lowercase();
    if url_lower.ends_with(".jpg")
        || url_lower.ends_with(".jpeg")
        || url_lower.ends_with(".png")
        || url_lower.ends_with(".gif")
        || url_lower.ends_with(".webp")
        || url_lower.ends_with(".svg")
        || url_lower.ends_with(".ico")
    {
        return "images";
    }
    if url_lower.ends_with(".mp4")
        || url_lower.ends_with(".webm")
        || url_lower.ends_with(".avi")
        || url_lower.ends_with(".mov")
        || url_lower.ends_with(".mkv")
    {
        return "videos";
    }
    if url_lower.ends_with(".mp3")
        || url_lower.ends_with(".wav")
        || url_lower.ends_with(".ogg")
        || url_lower.ends_with(".flac")
    {
        return "videos";
    }

    // 3. Check OpenGraph type
    if let Some(ref og) = content.open_graph {
        if let Some(ref og_type) = og.r#type {
            let t = og_type.to_lowercase();
            if t == "article" && is_news_domain(&content.url) {
                return "news";
            }
            if t == "video" || t == "video.movie" || t == "video.episode" {
                return "videos";
            }
            if t == "image" {
                return "images";
            }
        }
    }

    // 4. Check schema type
    if let Some(ref st) = content.schema_type {
        let s = st.to_lowercase();
        if s.contains("news") || s.contains("article") {
            if is_news_domain(&content.url) {
                return "news";
            }
        }
        if s.contains("techarticle") || s.contains("scholarlyarticle") || s.contains("document") {
            return "documentation";
        }
    }

    // 5. Check content text for news markers
    if content.word_count > 100 {
        let text_lower = content.content_text.to_lowercase();
        let news_signals = [
            "reported",
            "according to",
            "sources say",
            "announced",
            "today",
            "yesterday",
            "breaking news",
            "exclusive",
            "报道",
            "记者",
        ];
        let signal_count = news_signals
            .iter()
            .filter(|s| text_lower.contains(*s))
            .count();
        if signal_count >= 3 {
            return "news";
        }
    }

    // 6. Check if it's documentation-like
    if let Some(ref st) = content.schema_type {
        let s = st.to_lowercase();
        if s.contains("tutorial") || s.contains("howto") || s.contains("guide") {
            return "documentation";
        }
    }

    "text"
}

/// Guess whether a URL belongs to a known news publisher.
fn is_news_domain(url: &str) -> bool {
    let url_lower = url.to_lowercase();
    let news_domains = [
        "news",
        "cnn",
        "bbc",
        "reuters",
        "apnews",
        "nytimes",
        "wsj",
        "bloomberg",
        "theguardian",
        "washingtonpost",
        "economist",
        "forbes",
        "cnbc",
        "npr",
        "abcnews",
        "nbcnews",
        "cbsnews",
        "aljazeera",
        "huffpost",
        "buzzfeednews",
        "vox",
        "axios",
        "politico",
        "thehill",
        "usatoday",
        "latimes",
        "chicagotribune",
    ];
    news_domains.iter().any(|d| url_lower.contains(d))
}
