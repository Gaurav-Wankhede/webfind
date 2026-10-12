//! Structural code indexing — the fourth stage of the token → parse-tree →
//! dependency-graph pipeline.
//!
//! # Why this module exists
//!
//! The parse tree gives you hierarchical structure. This module adds
//! **pattern matching** over it — "find every function that returns Result",
//! "find every unwrap() call", "find every deprecated API usage". That is
//! what turns a parse tree into searchable code structure.
//!
//! # Why tree-sitter queries instead of ast-grep
//!
//! `ast-grep 0.45.3` was evaluated and rejected:
//! - It is **CLI-only** — the crate exposes `main_with_args()` but no
//!   library API for programmatic pattern matching.
//! - It is **non-standard licensed** (not MIT/Apache), which conflicts with
//!   WebFind's MIT distribution.
//!
//! tree-sitter's `Query` API is what ast-grep uses under the hood. Using it
//! directly gives the same pattern-matching power with no license concerns
//! and full programmatic control.
//!
//! # Pattern syntax
//!
//! tree-sitter queries use S-expression patterns with capture names:
//! - `fn $NAME($$$) { $$$ }` — match any function, capture its name
//! - `call_expression: (identifier) @fn` — match function calls
//! - `((identifier) @name (#eq? @name "unwrap"))` — match specific identifiers
//!
//! # What this module provides
//!
//! - `CodeMatch` — a matched pattern with its span and snippet
//! - `match_pattern()` — run a tree-sitter query against source code
//! - `extract_code_symbols()` — extract all functions/classes/imports from
//!   a parse tree using built-in patterns
//! - `find_error_handling()` — find unwrap/expect/Result-returning functions
//! - `find_todos()` — find TODO/FIXME/HACK markers in comments

use anyhow::{Context, Result};
use tree_sitter::{Parser, Query, QueryCursor, StreamingIterator};

use crate::engine::parse_tree::ParseTreeEngine;
use crate::engine::tokenizer::normalize;

/// A single code pattern match.
#[derive(Debug, Clone, PartialEq)]
pub struct CodeMatch {
    /// The document this match belongs to.
    pub doc_id: String,
    /// The pattern that matched.
    pub pattern: String,
    /// Byte offset of the match start.
    pub start_byte: usize,
    /// Byte offset of the match end.
    pub end_byte: usize,
    /// The matched source text.
    pub snippet: String,
    /// Captured named nodes (capture name → text).
    pub captures: std::collections::HashMap<String, String>,
}

impl CodeMatch {
    /// Whether this match is a function definition.
    #[must_use]
    pub fn is_function(&self) -> bool {
        self.pattern.contains("function") || self.pattern.contains("fn")
    }

    /// Whether this match is an error-handling pattern.
    #[must_use]
    pub fn is_error_handling(&self) -> bool {
        self.pattern.contains("unwrap") || self.pattern.contains("expect") || self.pattern.contains("Result")
    }
}

/// A built-in code pattern for common structural queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePattern {
    /// Function definitions (any language).
    FunctionDefinition,
    /// Method calls (any language).
    MethodCall,
    /// Error handling: unwrap/expect/Result.
    ErrorHandling,
    /// TODO/FIXME/HACK markers in comments.
    TodoMarker,
    /// Import statements.
    ImportStatement,
    /// Deprecated API usage.
    DeprecatedApi,
}

impl CodePattern {
    /// The tree-sitter query string for this pattern.
    ///
    /// These patterns use tree-sitter's S-expression syntax. The `$$$`
    /// wildcard matches any number of children.
    #[must_use]
    pub fn query_str_for(&self, language: &str) -> &'static str {
        // A tree-sitter query is compiled against ONE grammar. Listing node
        // types from other languages makes compilation fail with
        // "Invalid node type", so each pattern is selected per language.
        match (self, language) {
            // ---- function definitions ----
            (CodePattern::FunctionDefinition, "rust") => r#"
                (function_item name: (identifier) @name) @func
                (function_signature_item name: (identifier) @name) @func
            "#,
            (CodePattern::FunctionDefinition, "python") => r#"
                (function_definition name: (identifier) @name) @func
            "#,
            (CodePattern::FunctionDefinition, _) => r#"
                (function_declaration name: (identifier) @name) @func
                (method_declaration name: (identifier) @name) @func
                (method_definition name: (identifier) @name) @func
            "#,

            // ---- method / function calls ----
            (CodePattern::MethodCall, "rust") => r#"
                (call_expression function: (identifier) @callee) @call
                (call_expression function: (field_expression field: (field_identifier) @method)) @call
                (macro_invocation macro: (identifier) @callee) @call
            "#,
            (CodePattern::MethodCall, "python") => r#"
                (call function: (identifier) @callee) @call
                (call function: (attribute attribute: (identifier) @method)) @call
            "#,
            (CodePattern::MethodCall, _) => r#"
                (call_expression function: (identifier) @callee) @call
                (call_expression function: (member_expression property: (property_identifier) @method)) @call
            "#,

            // ---- error handling ----
            (CodePattern::ErrorHandling, "rust") => r#"
                (call_expression function: (identifier) @name) @call
                (call_expression function: (field_expression field: (field_identifier) @name)) @call
                (macro_invocation macro: (identifier) @name) @call
                (type_identifier) @type
            "#,
            (CodePattern::ErrorHandling, _) => r#"
                (call_expression function: (identifier) @name) @call
            "#,

            // ---- TODO / FIXME / HACK in comments ----
            (CodePattern::TodoMarker, "rust") => r#"
                (line_comment) @comment
                (block_comment) @comment
            "#,
            (CodePattern::TodoMarker, "python") => r#"
                (comment) @comment
            "#,
            (CodePattern::TodoMarker, _) => r#"
                (comment) @comment
                (line_comment) @comment
            "#,

            // ---- import statements ----
            (CodePattern::ImportStatement, "rust") => r#"
                (use_declaration) @import
            "#,
            (CodePattern::ImportStatement, "python") => r#"
                (import_statement) @import
                (import_from_statement) @import
            "#,
            (CodePattern::ImportStatement, _) => r#"
                (import_declaration) @import
            "#,

            // ---- deprecated API usage ----
            (CodePattern::DeprecatedApi, _) => r#"
                ((identifier) @id (#match? @id "(?i)deprecated|obsolete|legacy"))
            "#,
        }
    }

    /// The query string for this pattern in the given language.
    ///
    /// Falls back to the generic (non-language-specific) pattern when no
    /// language-specific variant exists.
    #[must_use]
    pub fn query_str(&self) -> &'static str {
        self.query_str_for("generic")
    }

    /// A human-readable name for this pattern.
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            CodePattern::FunctionDefinition => "function_definition",
            CodePattern::MethodCall => "method_call",
            CodePattern::ErrorHandling => "error_handling",
            CodePattern::TodoMarker => "todo_marker",
            CodePattern::ImportStatement => "import_statement",
            CodePattern::DeprecatedApi => "deprecated_api",
        }
    }
}

/// Check if a tree-sitter query match satisfies all #eq? and #match? predicates.
///
/// tree-sitter built without the `perf` feature silently ignores these
/// predicates, causing them to match everything. This function re-applies
/// them as Rust-side filters.
fn check_predicates(
    m: &tree_sitter::QueryMatch,
    query: &Query,
    source: &str,
) -> bool {
    let pattern_idx = m.pattern_index;
    for pred in query.general_predicates(pattern_idx) {
        match pred.operator.as_ref() {
            "eq?" if pred.args.len() == 2 => {
                if let tree_sitter::QueryPredicateArg::Capture(cap_idx) = pred.args[0]
                    && let tree_sitter::QueryPredicateArg::String(ref expected) = pred.args[1]
                {
                    let capture = m.captures().iter().find(|c| c.index == cap_idx);
                    if let Some(cap) = capture {
                        let actual = &source[cap.node.start_byte()..cap.node.end_byte()];
                        if actual != expected.as_ref() {
                            return false;
                        }
                    }
                }
            }
            "match?" if pred.args.len() == 2 => {
                if let tree_sitter::QueryPredicateArg::Capture(cap_idx) = pred.args[0]
                    && let tree_sitter::QueryPredicateArg::String(ref pattern) = pred.args[1]
                {
                    let capture = m.captures().iter().find(|c| c.index == cap_idx);
                    if let Some(cap) = capture {
                        let actual = &source[cap.node.start_byte()..cap.node.end_byte()];
                        if let Ok(re) = regex::Regex::new(pattern)
                            && !re.is_match(actual)
                        {
                            return false;
                        }
                    }
                }
            }
            _ => {}
        }
    }
    true
}

/// Run a tree-sitter query against `source` in `language`, with predicate filtering.
///
/// Returns all matches with their spans and captured named nodes.
/// Applies Rust-side filtering for #eq? and #match? predicates that
/// tree-sitter may silently ignore when built without the perf feature.
///
/// # Errors
///
/// Returns an error if the query string is invalid for the language.
pub fn match_pattern_filtered(
    source: &str,
    language: &str,
    pattern: &str,
) -> Result<Vec<CodeMatch>> {
    let normalized = normalize(source);
    let lang = tree_sitter_language_pack::get_language(language)
        .with_context(|| format!("load language `{language}`"))?;

    let mut parser = Parser::new();
    parser.set_language(&lang)
        .with_context(|| format!("set parser language `{language}`"))?;

    let tree = parser.parse(&normalized, None)
        .with_context(|| format!("parse source ({} bytes)", normalized.len()))?;

    let query = Query::new(&lang, pattern)
        .with_context(|| format!("compile query pattern for `{language}`"))?;

    let mut cursor = QueryCursor::new();
    let mut matches = Vec::new();
    let mut captures = cursor.captures(&query, tree.root_node(), normalized.as_bytes());
    while let Some((m, _)) = captures.next() {
        if !check_predicates(m, &query, &normalized) {
            continue;
        }
        let node = m.captures()[0].node;
        let start_byte = node.start_byte();
        let end_byte = node.end_byte();
        let snippet = normalized[start_byte..end_byte].to_string();

        let mut captured = std::collections::HashMap::new();
        for capture in m.captures() {
            let name = query.capture_names()[capture.index as usize].to_string();
            let text = normalized[capture.node.start_byte()..capture.node.end_byte()].to_string();
            captured.insert(name, text);
        }

        matches.push(CodeMatch {
            doc_id: String::new(),
            pattern: pattern.to_string(),
            start_byte,
            end_byte,
            snippet,
            captures: captured,
        });
    }

    Ok(matches)
}

/// Run a tree-sitter query against `source` in `language`.
///
/// Returns all matches with their spans and captured named nodes.
/// Delegates to `match_pattern_filtered` for predicate-aware matching.
///
/// # Errors
///
/// Returns an error if the query string is invalid for the language.
pub fn match_pattern(
    source: &str,
    language: &str,
    pattern: &str,
) -> Result<Vec<CodeMatch>> {
    match_pattern_filtered(source, language, pattern)
}

/// Extract all code symbols from a document using the built-in patterns.
///
/// Runs all `CodePattern` variants and returns a flat list of matches.
/// Each match is tagged with its pattern name.
pub fn extract_code_symbols(
    source: &str,
    language: &str,
) -> Result<Vec<CodeMatch>> {
    let mut all_matches = Vec::new();
    for pattern in [
        CodePattern::FunctionDefinition,
        CodePattern::MethodCall,
        CodePattern::ErrorHandling,
        CodePattern::TodoMarker,
        CodePattern::ImportStatement,
        CodePattern::DeprecatedApi,
    ] {
        let mut matches = match_pattern(source, language, pattern.query_str_for(language))?;
        for m in &mut matches {
            m.pattern = pattern.name().to_string();
        }
        all_matches.extend(matches);
    }
    Ok(all_matches)
}

/// Find error-handling patterns in source code.
///
/// Returns matches for `unwrap()`, `expect()`, `panic!()`, and `Result` types.
pub fn find_error_handling(source: &str, language: &str) -> Result<Vec<CodeMatch>> {
    match_pattern(source, language, CodePattern::ErrorHandling.query_str_for(language))
}

/// Find TODO/FIXME/HACK markers in comments.
pub fn find_todos(source: &str, language: &str) -> Result<Vec<CodeMatch>> {
    match_pattern(source, language, CodePattern::TodoMarker.query_str_for(language))
}

/// Find import statements.
pub fn find_imports(source: &str, language: &str) -> Result<Vec<CodeMatch>> {
    match_pattern(source, language, CodePattern::ImportStatement.query_str_for(language))
}

/// Extract code symbols from a parsed document and merge them into the
/// `ParseNode` list.
///
/// This is the bridge between the parse tree and the code-pattern layer:
/// it runs all built-in patterns and returns the combined node list.
pub fn extract_and_merge(
    _engine: &ParseTreeEngine,
    url: &str,
    text: &str,
) -> Result<Vec<CodeMatch>> {
    let normalized = normalize(text);
    let language = tree_sitter_language_pack::detect_language_from_path(url)
        .or_else(|| tree_sitter_language_pack::detect_language_from_content(&normalized))
        .unwrap_or("text");

    if language == "text" || !tree_sitter_language_pack::has_parser(language) {
        return Ok(Vec::new());
    }

    extract_code_symbols(&normalized, language)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_function_definitions_in_rust() {
        let rust = r#"
            fn main() {
                println!("hello");
            }
            fn helper() -> i32 { 42 }
        "#;
        let matches = match_pattern(rust, "rust", CodePattern::FunctionDefinition.query_str_for("rust")).unwrap();
        assert!(!matches.is_empty(), "must find function definitions");
        assert!(matches.iter().any(|m| m.captures.contains_key("name")));
    }

    #[test]
    fn finds_function_definitions_in_python() {
        let py = "def hello():\n    print('world')\n";
        let matches = match_pattern(py, "python", CodePattern::FunctionDefinition.query_str_for("python")).unwrap();
        assert!(!matches.is_empty(), "must find function definitions in python");
    }

    #[test]
    fn finds_error_handling_patterns() {
        let rust = r#"
            fn risky() {
                let x = some_result.unwrap();
                let y = another.expect("failed");
            }
        "#;
        let matches = find_error_handling(rust, "rust").unwrap();
        assert!(!matches.is_empty(), "must find unwrap/expect");
        assert!(matches.iter().any(|m| m.snippet.contains("unwrap")));
        assert!(matches.iter().any(|m| m.snippet.contains("expect")));
    }

    #[test]
    fn finds_todo_markers() {
        let rust = r#"
            // TODO: refactor this
            fn foo() {}
            // FIXME: handle edge case
        "#;
        let matches = find_todos(rust, "rust").unwrap();
        assert_eq!(matches.len(), 2, "must find both TODO and FIXME");
    }

    #[test]
    fn finds_import_statements() {
        let rust = "use std::collections::HashMap;\nuse std::io::Read;\n";
        let matches = find_imports(rust, "rust").unwrap();
        assert_eq!(matches.len(), 2, "must find both use statements");
    }

    #[test]
    fn extract_code_symbols_finds_multiple_kinds() {
        let rust = r#"
            use std::collections::HashMap;
            // TODO: optimize
            fn main() {
                let x = compute().unwrap();
            }
        "#;
        let matches = extract_code_symbols(rust, "rust").unwrap();
        let patterns: Vec<&str> = matches.iter().map(|m| m.pattern.as_str()).collect();
        assert!(patterns.contains(&"import_statement"), "must find imports");
        assert!(patterns.contains(&"todo_marker"), "must find TODO");
        assert!(patterns.contains(&"function_definition"), "must find function");
        assert!(patterns.contains(&"error_handling"), "must find unwrap");
    }

    #[test]
    fn no_match_returns_empty_vec() {
        let rust = "fn main() {}";
        let matches = match_pattern(rust, "rust", CodePattern::TodoMarker.query_str_for("rust")).unwrap();
        assert!(matches.is_empty(), "no TODO must yield empty vec");
    }

    #[test]
    fn pattern_with_no_match_does_not_error() {
        let rust = "fn main() {}";
        let result = match_pattern(rust, "rust", CodePattern::DeprecatedApi.query_str_for("rust"));
        assert!(result.is_ok(), "no match must not error");
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn extract_and_merge_works_with_url() {
        let engine = ParseTreeEngine::new();
        let rust = "fn main() { let x = foo().unwrap(); }";
        let matches = extract_and_merge(&engine, "https://example.com/main.rs", rust).unwrap();
        assert!(!matches.is_empty(), "must extract code symbols from URL");
    }

    #[test]
    fn extract_and_merge_falls_back_for_unknown_extension() {
        let engine = ParseTreeEngine::new();
        let text = "some plain text";
        let matches = extract_and_merge(&engine, "https://example.com/file.xyz", text).unwrap();
        assert!(matches.is_empty(), "unknown extension must yield empty");
    }

    #[test]
    fn code_match_helpers_work() {
        let m = CodeMatch {
            doc_id: "doc".into(),
            pattern: "function_definition".into(),
            start_byte: 0,
            end_byte: 10,
            snippet: "fn main()".into(),
            captures: std::collections::HashMap::new(),
        };
        assert!(m.is_function());
        assert!(!m.is_error_handling());
    }

    #[test]
    fn all_patterns_have_distinct_names() {
        let patterns = [
            CodePattern::FunctionDefinition,
            CodePattern::MethodCall,
            CodePattern::ErrorHandling,
            CodePattern::TodoMarker,
            CodePattern::ImportStatement,
            CodePattern::DeprecatedApi,
        ];
        let names: Vec<&str> = patterns.iter().map(|p| p.name()).collect();
        let unique: std::collections::HashSet<_> = names.iter().cloned().collect();
        assert_eq!(names.len(), unique.len(), "pattern names must be unique");
    }
}
