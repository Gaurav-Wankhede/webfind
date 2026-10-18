//! Dynamic Query Router: 4-layer cascade
//!
//! Routes queries to appropriate search strategies based on intent, domain, and confidence.
//! Layers: Deterministic → Embedding → Language/Intent → Fallback

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Query routing decision
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRoute {
    pub intent: QueryIntent,
    pub domain: String,
    pub language: String,
    pub sources: Vec<String>,
    pub confidence: f32,
    pub strategy: SearchStrategy,
}

/// Query intent classification
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum QueryIntent {
    /// CVE/GHSA/OSV vulnerability lookup
    Vulnerability,
    /// Rust compiler error (E0xxx)
    RustError,
    /// Code explanation / how-to
    CodeExplanation,
    /// API / library documentation lookup
    ApiDocs,
    /// Technical specification / standard
    Specification,
    /// General knowledge / fact lookup
    FactLookup,
    /// Comparison / vs query
    Comparison,
    /// Tutorial / guide
    Tutorial,
    /// Unknown / general
    General,
}

/// Search strategy to use
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SearchStrategy {
    /// Deterministic routing to specific engines
    Deterministic,
    /// Embedding-based semantic search
    Embedding,
    /// Language/keyword based
    Language,
    /// Fallback to all sources
    Fallback,
}

use regex::Regex;
use std::sync::LazyLock;

static RE_CVE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bcve-\d{4}-\d{4,}\b").expect("valid cve regex")
});
static RE_GHSA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bghsa-[a-z0-9]{4}-[a-z0-9]{4}-[a-z0-9]{4}\b").expect("valid ghsa regex")
});
static RE_RUST_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\be0\d{3}\b").expect("valid rust error regex")
});

/// Layer 1: Deterministic routing based on patterns
fn route_deterministic(query: &str) -> Option<QueryRoute> {
    let q = query.trim();
    let q_lower = q.to_lowercase();
    
    // CVE pattern: CVE-YYYY-NNNNN
    if RE_CVE.is_match(q) {
        return Some(QueryRoute {
            intent: QueryIntent::Vulnerability,
            domain: "security".to_string(),
            language: "en".to_string(),
            sources: vec![
                "duckduckgo".to_string(),
                "bing".to_string(),
                "wikipedia".to_string(),
                "github-code".to_string(),
            ],
            confidence: 0.99,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    // GHSA pattern: GHSA-xxxx-xxxx-xxxx
    if RE_GHSA.is_match(q) {
        return Some(QueryRoute {
            intent: QueryIntent::Vulnerability,
            domain: "security".to_string(),
            language: "en".to_string(),
            sources: vec![
                "duckduckgo".to_string(),
                "bing".to_string(),
                "wikipedia".to_string(),
                "github-code".to_string(),
            ],
            confidence: 0.99,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    // Rust error code: E0xxx (may appear anywhere in the query, e.g.
    // "E0308 mismatched types" or "how to fix E0502"). Anchoring to the whole
    // query meant these fell through to the general branch and returned
    // irrelevant results.
    if RE_RUST_ERROR.is_match(q) {
        return Some(QueryRoute {
            intent: QueryIntent::RustError,
            domain: "rust".to_string(),
            language: "en".to_string(),
            sources: vec![
                "devdocs".to_string(),
                "crates-io".to_string(),
                "github-code".to_string(),
                "stackoverflow".to_string(),
            ],
            confidence: 0.95,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    // Code verbs: "how to", "implement", "async fn", "fn ", "struct ", "impl "
    let code_verbs = ["how to", "implement", "async fn", "fn ", "struct ", "impl ", "trait ", "macro "];
    if code_verbs.iter().any(|v| q_lower.contains(v)) {
        // Detect language from keywords
        let language = detect_language(query);
        return Some(QueryRoute {
            intent: QueryIntent::CodeExplanation,
            domain: "programming".to_string(),
            language,
            sources: vec!["github-code".to_string(), "crates-io".to_string(), "stackoverflow".to_string(), "raw-github".to_string()],
            confidence: 0.85,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    // API docs: "api reference", "method ", "function ", "class "
    let api_patterns = ["api reference", "method ", "function ", "class ", "interface "];
    if api_patterns.iter().any(|v| q_lower.contains(v)) {
        return Some(QueryRoute {
            intent: QueryIntent::ApiDocs,
            domain: "programming".to_string(),
            language: "en".to_string(),
            sources: vec!["crates-io".to_string(), "devdocs".to_string(), "mdn".to_string(), "github-code".to_string()],
            confidence: 0.80,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    // Specification: "rfc", "spec", "standard"
    let spec_patterns = ["rfc ", "specification", "standard "];
    if spec_patterns.iter().any(|v| q_lower.contains(v)) {
        return Some(QueryRoute {
            intent: QueryIntent::Specification,
            domain: "technical".to_string(),
            language: "en".to_string(),
            sources: vec![
                "devdocs".to_string(),
                "mdn".to_string(),
                "duckduckgo".to_string(),
                "github-code".to_string(),
            ],
            confidence: 0.80,
            strategy: SearchStrategy::Deterministic,
        });
    }
    
    None
}

/// Layer 2: Embedding-based routing (semantic similarity to domain centroids)
fn route_embedding(_query: &str, _query_embedding: &[f32]) -> Option<QueryRoute> {
    // In production: compute cosine similarity to pre-computed domain centroids
    // For now, return None to fall through to Layer 3
    None
}

/// Layer 3: Language/Intent detection
fn route_language_intent(query: &str) -> QueryRoute {
    let language = detect_language(query);
    let intent = detect_intent(query);
    let domain = detect_domain(query);
    
    // Select sources based on domain
    let sources = select_sources_for_domain(&domain, &intent);
    
    QueryRoute {
        intent,
        domain,
        language,
        sources,
        confidence: 0.70,
        strategy: SearchStrategy::Language,
    }
}

/// Layer 4: Fallback - use all high-quality sources
fn route_fallback() -> QueryRoute {
    QueryRoute {
        intent: QueryIntent::General,
        domain: "general".to_string(),
        language: "en".to_string(),
        sources: vec![
            "arxiv".to_string(),
            "semantic-scholar".to_string(),
            "github-code".to_string(),
            "crates-io".to_string(),
            "wikipedia".to_string(),
            "stackoverflow".to_string(),
        ],
        confidence: 0.50,
        strategy: SearchStrategy::Fallback,
    }
}

/// Main routing function
pub fn route_query(query: &str, query_embedding: Option<&[f32]>) -> QueryRoute {
    // Layer 1: Deterministic
    if let Some(route) = route_deterministic(query) {
        return route;
    }
    
    // Layer 2: Embedding
    if let Some(emb) = query_embedding {
        if let Some(route) = route_embedding(query, emb) {
            return route;
        }
    }
    
    // Layer 3: Language/Intent
    let route = route_language_intent(query);
    if route.confidence > 0.60 {
        return route;
    }
    
    // Layer 4: Fallback
    route_fallback()
}

/// Detect programming language from query
fn detect_language(query: &str) -> String {
    let q_lower = query.to_lowercase();
    
    let lang_keywords: HashMap<&str, &str> = [
        ("rust", "rust"),
        ("cargo", "rust"),
        ("tokio", "rust"),
        ("async", "rust"),
        ("await", "rust"),
        ("python", "python"),
        ("pip", "python"),
        ("django", "python"),
        ("fastapi", "python"),
        ("javascript", "javascript"),
        ("typescript", "typescript"),
        ("node.js", "javascript"),
        ("npm", "javascript"),
        ("react", "javascript"),
        ("vue", "javascript"),
        ("go", "go"),
        ("golang", "go"),
        ("java", "java"),
        ("spring", "java"),
        ("c++", "cpp"),
        ("cpp", "cpp"),
        ("c#", "csharp"),
        ("swift", "swift"),
        ("kotlin", "kotlin"),
    ].into_iter().collect();
    
    // Prefer the LONGEST matching keyword. A HashMap iteration order is random,
    // so iterating it directly made this function non-deterministic: for
    // "javascript react", whichever of "java" / "javascript" happened to come
    // first won -- even though "javascript" is the better match. Sorting by
    // descending keyword length makes the result stable AND correct.
    let mut keywords: Vec<(&str, &str)> = lang_keywords.into_iter().collect();
    keywords.sort_by(|a, b| b.0.len().cmp(&a.0.len()));

    for (kw, lang) in keywords {
        if q_lower.contains(kw) {
            return lang.to_string();
        }
    }

    "en".to_string()
}

/// Detect intent from query
fn detect_intent(query: &str) -> QueryIntent {
    let q_lower = query.to_lowercase();
    
    if q_lower.contains("how to") || q_lower.contains("tutorial") || q_lower.contains("guide") {
        return QueryIntent::Tutorial;
    }
    if q_lower.contains("vs ") || q_lower.contains("versus") || q_lower.contains("compare") || q_lower.contains("difference") {
        return QueryIntent::Comparison;
    }
    if q_lower.contains("what is") || q_lower.contains("define") || q_lower.contains("meaning of") {
        return QueryIntent::FactLookup;
    }
    if q_lower.contains("error") || q_lower.contains("exception") || q_lower.contains("fail") {
        return QueryIntent::CodeExplanation;
    }
    
    QueryIntent::General
}

/// Detect domain from query
fn detect_domain(query: &str) -> String {
    let q_lower = query.to_lowercase();
    
    if q_lower.contains("rust") || q_lower.contains("cargo") || q_lower.contains("tokio") {
        return "rust".to_string();
    }
    if q_lower.contains("security") || q_lower.contains("vulnerability") || q_lower.contains("cve") {
        return "security".to_string();
    }
    if q_lower.contains("machine learning") || q_lower.contains("ml ") || q_lower.contains("ai ") {
        return "ai_ml".to_string();
    }
    if q_lower.contains("web") || q_lower.contains("http") || q_lower.contains("api") {
        return "web".to_string();
    }
    if q_lower.contains("database") || q_lower.contains("sql") || q_lower.contains("postgres") {
        return "database".to_string();
    }
    
    "general".to_string()
}

/// Select sources for domain/intent combination
fn select_sources_for_domain(domain: &str, intent: &QueryIntent) -> Vec<String> {
    match (domain, intent) {
        ("rust", QueryIntent::CodeExplanation) => vec!["devdocs".to_string(), "crates-io".to_string(), "github-code".to_string(), "raw-github".to_string()],
        ("rust", QueryIntent::RustError) => vec!["devdocs".to_string(), "crates-io".to_string(), "github-code".to_string(), "stackoverflow".to_string()],
        ("security", QueryIntent::Vulnerability) => vec!["wikipedia".to_string(), "stackoverflow".to_string(), "github-code".to_string(), "devdocs".to_string()],
        ("ai_ml", _) => vec!["arxiv".to_string(), "semantic-scholar".to_string(), "github-code".to_string()],
        ("web", QueryIntent::ApiDocs) => vec!["mdn".to_string(), "devdocs".to_string(), "github-code".to_string()],
        (_, QueryIntent::CodeExplanation) => vec!["github-code".to_string(), "crates-io".to_string(), "stackoverflow".to_string()],
        (_, QueryIntent::ApiDocs) => vec!["devdocs".to_string(), "mdn".to_string(), "crates-io".to_string()],
        (_, QueryIntent::Tutorial) => vec!["github-code".to_string(), "stackoverflow".to_string(), "devdocs".to_string()],
        // General / unmatched queries need broad web coverage, not research-paper
        // engines. Routing "serde json deserialize" to arxiv + semantic-scholar
        // returned off-topic papers (and semantic-scholar rate-limits), leaving
        // the precision gate to reject the single surviving hit.
        _ => vec![
            "duckduckgo".to_string(),
            "bing".to_string(),
            "wikipedia".to_string(),
            "devdocs".to_string(),
        ],
    }
}


/// Query router for routing queries to appropriate search strategies
#[derive(Debug, Clone, Default)]
pub struct QueryRouter {
    // Configuration options can be added here
}

impl QueryRouter {
    /// Create a new query router
    pub fn new() -> Self {
        Self::default()
    }
    
    /// Route a query to determine the optimal search strategy
    pub fn route(&self, query: &str) -> QueryRoute {
        route_query(query, None)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_cve_routing() {
        let route = route_query("CVE-2024-3094", None);
        assert_eq!(route.intent, QueryIntent::Vulnerability);
        assert_eq!(route.strategy, SearchStrategy::Deterministic);
        assert!(route.confidence > 0.95);
    }
    
    #[test]
    fn test_rust_error_routing() {
        let route = route_query("E0308", None);
        assert_eq!(route.intent, QueryIntent::RustError);
        assert!(route.sources.contains(&"devdocs".to_string()));
    }
    
    #[test]
    fn test_code_explanation_routing() {
        let route = route_query("how to implement async fn in rust", None);
        assert_eq!(route.intent, QueryIntent::CodeExplanation);
        assert_eq!(route.language, "rust");
    }
    
    #[test]
    fn test_language_detection() {
        assert_eq!(detect_language("rust async tokio"), "rust");
        assert_eq!(detect_language("python fastapi"), "python");
        assert_eq!(detect_language("javascript react"), "javascript");
    // Regression: "javascript" contains "java", so a HashMap-iteration-order
    // implementation returned whichever came first -- flaky between runs.
    // Longest-keyword-first makes it deterministic.
    assert_eq!(detect_language("java spring boot"), "java");
    assert_eq!(detect_language("typescript react hooks"), "typescript");
    assert_eq!(detect_language("golang channels"), "go");
    assert_eq!(detect_language("plain english prose"), "en");
    }
}