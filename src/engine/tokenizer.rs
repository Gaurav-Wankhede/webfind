//! Tokenization layer — the first stage of the token → parse-tree →
//! dependency-graph pipeline.
//!
//! # Why this module exists
//!
//! There was no tokenizer in the codebase. `keyword_extraction` produced a flat
//! `Vec<String>` of keywords per page, and those keywords were never linked to
//! anything — no token stream, no positions, no relations. A dependency graph
//! needs a token layer to build on.
//!
//! # Two tiers
//!
//! **Tier A — linguistic tokens** (feeds the parse tree and the graph):
//! [`unicode_segmentation`] splits text into grapheme/word/sentence units per
//! Unicode Standard Annex #29. Each token carries its kind, its position, and
//! its byte span, so a downstream stage can map a token back to the exact
//! source bytes.
//!
//! **Tier B — LLM-aligned tokens** (feeds context budgeting):
//! [`tiktoken`] produces BPE token counts matching OpenAI's tokenizer, so a
//! caller can decide whether a page fits a context window without guessing.
//!
//! Text is NFKC-normalized before Tier A, so full-width characters, ligatures,
//! and compatibility forms fold to their canonical equivalents. Without this,
//! `Ｒｕｓｔ` and `Rust` would be different tokens.

use std::sync::OnceLock;

use anyhow::{Context, Result};
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// The default tiktoken encoding used for LLM token counts.
///
/// `cl100k_base` is the encoding for the GPT-4 / GPT-3.5 family and is the
/// most widely applicable default for context budgeting.
pub const DEFAULT_ENCODING: &str = "cl100k_base";

/// What kind of unit a token is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    /// A word: letters, possibly with internal digits or apostrophes.
    Word,
    /// A run of digits, possibly with a decimal point.
    Number,
    /// A single punctuation character.
    Punctuation,
    /// A symbol or operator that is not punctuation (e.g. `+`, `$`, `→`).
    Symbol,
    /// Whitespace run.
    Whitespace,
}

impl TokenKind {
    /// The stable string form stored in the `tokens.kind` column.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            TokenKind::Word => "word",
            TokenKind::Number => "number",
            TokenKind::Punctuation => "punctuation",
            TokenKind::Symbol => "symbol",
            TokenKind::Whitespace => "whitespace",
        }
    }

    /// Parse the stored string form, defaulting to [`TokenKind::Word`].
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "number" => TokenKind::Number,
            "punctuation" => TokenKind::Punctuation,
            "symbol" => TokenKind::Symbol,
            "whitespace" => TokenKind::Whitespace,
            _ => TokenKind::Word,
        }
    }

    /// Classify a single unicode word segment.
    fn classify(segment: &str) -> Self {
        let mut has_alphanumeric = false;
        let mut all_numeric = true;
        for ch in segment.chars() {
            if ch.is_alphanumeric() {
                has_alphanumeric = true;
                if !ch.is_numeric() && ch != '.' {
                    all_numeric = false;
                }
            } else if ch.is_whitespace() {
                // Whitespace inside a word segment is unusual but possible.
            } else {
                all_numeric = false;
            }
        }

        if !has_alphanumeric {
            // No letters or digits: punctuation or symbol.
            let all_punct = segment
                .chars()
                .all(|c| c.is_ascii_punctuation() && !c.is_ascii_alphanumeric());
            return if all_punct {
                TokenKind::Punctuation
            } else {
                TokenKind::Symbol
            };
        }

        if all_numeric {
            TokenKind::Number
        } else {
            TokenKind::Word
        }
    }
}

/// A single linguistic token with its position and source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// The normalized token text.
    pub text: String,
    /// What kind of unit this is.
    pub kind: TokenKind,
    /// Zero-based ordinal position in the token stream.
    pub pos: usize,
    /// Byte offset of the token's start in the **normalized** text.
    pub byte_start: usize,
    /// Byte offset one past the token's end in the **normalized** text.
    pub byte_end: usize,
}

impl Token {
    /// Whether this token carries lexical content (word or number).
    ///
    /// Whitespace and punctuation are structurally useful but are not terms —
    /// a keyword index should skip them.
    #[must_use]
    pub fn is_lexical(&self) -> bool {
        matches!(self.kind, TokenKind::Word | TokenKind::Number)
    }

    /// The token text lowercased, for case-insensitive term matching.
    #[must_use]
    pub fn normalized_term(&self) -> String {
        self.text.to_lowercase()
    }
}

/// NFKC-normalize text before tokenizing.
///
/// Folds compatibility forms (full-width `Ｒｕｓｔ` → `Rust`), ligatures, and
/// combining sequences to their canonical equivalents so the same concept does
/// not produce different tokens depending on how it was typed.
#[must_use]
pub fn normalize(text: &str) -> String {
    text.nfkc().collect()
}

/// Tokenize `text` into linguistic tokens.
///
/// The input is NFKC-normalized first; byte spans refer to the **normalized**
/// text. Use [`normalize`] yourself if you need to map spans back to the
/// original input.
#[must_use]
pub fn tokenize(text: &str) -> Vec<Token> {
    let normalized = normalize(text);
    let mut tokens = Vec::new();
    let mut pos = 0usize;

    for (byte_start, segment) in normalized.split_word_bound_indices() {
        // Skip empty segments (can occur at boundaries).
        if segment.is_empty() {
            continue;
        }

        let kind = if segment.chars().all(char::is_whitespace) {
            TokenKind::Whitespace
        } else {
            TokenKind::classify(segment)
        };

        tokens.push(Token {
            text: segment.to_string(),
            kind,
            pos,
            byte_start,
            byte_end: byte_start + segment.len(),
        });
        pos += 1;
    }

    tokens
}

/// Tokenize and keep only lexical tokens (words and numbers).
///
/// This is the stream a keyword index or a graph edge builder wants: no
/// whitespace, no punctuation.
#[must_use]
pub fn lexical_tokens(text: &str) -> Vec<Token> {
    tokenize(text)
        .into_iter()
        .filter(Token::is_lexical)
        .collect()
}

/// Split `text` into sentences.
///
/// Uses UAX #29 sentence boundaries, so abbreviations and decimals do not
/// split spuriously.
#[must_use]
pub fn sentences(text: &str) -> Vec<String> {
    normalize(text)
        .unicode_sentences()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

/// The process-wide `cl100k_base` encoder, loaded once.
fn encoder() -> Result<&'static tiktoken::CoreBpe> {
    static ENCODER: OnceLock<Option<&'static tiktoken::CoreBpe>> = OnceLock::new();
    ENCODER
        .get_or_init(|| tiktoken::get_encoding(DEFAULT_ENCODING))
        .as_ref()
        .copied()
        .with_context(|| format!("tiktoken encoding `{DEFAULT_ENCODING}` is unavailable"))
}

/// Count LLM (BPE) tokens in `text`.
///
/// This is the number a context-window budget cares about, not the word count.
///
/// # Errors
///
/// Returns an error only if the encoder failed to load.
pub fn count_llm_tokens(text: &str) -> Result<usize> {
    Ok(encoder()?.count(text))
}

/// Encode `text` into LLM token ids.
///
/// # Errors
///
/// Returns an error only if the encoder failed to load.
pub fn encode_llm_tokens(text: &str) -> Result<Vec<u32>> {
    Ok(encoder()?.encode(text))
}

/// Whether `text` fits within an LLM context budget of `max_tokens`.
///
/// # Errors
///
/// Returns an error only if the encoder failed to load.
pub fn fits_context_window(text: &str, max_tokens: usize) -> Result<bool> {
    Ok(count_llm_tokens(text)? <= max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_words_with_correct_spans() {
        let tokens = tokenize("Hello, world!");
        let lexical: Vec<&Token> = tokens.iter().filter(|t| t.is_lexical()).collect();
        assert_eq!(lexical.len(), 2);
        assert_eq!(lexical[0].text, "Hello");
        assert_eq!(lexical[0].kind, TokenKind::Word);
        assert_eq!(lexical[1].text, "world");

        // Byte spans must address the normalized text exactly.
        let normalized = normalize("Hello, world!");
        for token in &tokens {
            assert_eq!(
                &normalized[token.byte_start..token.byte_end],
                token.text,
                "span must slice back to the token text"
            );
        }
    }

    #[test]
    fn positions_are_sequential() {
        let tokens = tokenize("a b c");
        for (i, token) in tokens.iter().enumerate() {
            assert_eq!(token.pos, i, "positions must be a dense ordinal sequence");
        }
    }

    #[test]
    fn nfkc_folds_fullwidth_to_ascii() {
        // Full-width Ｒｕｓｔ must tokenize the same as ASCII Rust.
        let fullwidth = tokenize("Ｒｕｓｔ");
        let ascii = tokenize("Rust");
        let fw: Vec<&str> = fullwidth
            .iter()
            .filter(|t| t.is_lexical())
            .map(|t| t.text.as_str())
            .collect();
        let ascii_terms: Vec<&str> = ascii
            .iter()
            .filter(|t| t.is_lexical())
            .map(|t| t.text.as_str())
            .collect();
        assert_eq!(fw, ascii_terms, "NFKC must fold compatibility forms");
        assert_eq!(fw, vec!["Rust"]);
    }

    #[test]
    fn classifies_numbers_and_punctuation() {
        let tokens = tokenize("count 42 items.");
        let kinds: Vec<TokenKind> = tokens.iter().map(|t| t.kind).collect();
        assert!(kinds.contains(&TokenKind::Number), "42 must be a Number");
        assert!(
            kinds.contains(&TokenKind::Punctuation),
            "'.' must be Punctuation"
        );
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        assert!(tokenize("").is_empty());
        assert!(lexical_tokens("").is_empty());
        assert!(sentences("").is_empty());
    }

    #[test]
    fn whitespace_only_input_yields_no_lexical_tokens() {
        let tokens = tokenize("   \n\t  ");
        assert!(
            tokens.iter().all(|t| !t.is_lexical()),
            "whitespace must never be lexical"
        );
        assert!(lexical_tokens("   \n\t  ").is_empty());
    }

    #[test]
    fn lexical_tokens_drops_whitespace_and_punctuation() {
        let all = tokenize("Rust, is fast.");
        let lexical = lexical_tokens("Rust, is fast.");
        assert!(lexical.len() < all.len(), "filtering must drop something");
        assert!(lexical.iter().all(Token::is_lexical));
        assert_eq!(
            lexical.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            vec!["Rust", "is", "fast"]
        );
    }

    #[test]
    fn splits_sentences_on_boundaries() {
        let s = sentences("First sentence. Second sentence! Third?");
        assert_eq!(s.len(), 3, "got {s:?}");
        assert_eq!(s[0], "First sentence.");
    }

    #[test]
    fn token_kind_round_trips_through_its_string_form() {
        for kind in [
            TokenKind::Word,
            TokenKind::Number,
            TokenKind::Punctuation,
            TokenKind::Symbol,
            TokenKind::Whitespace,
        ] {
            assert_eq!(TokenKind::parse(kind.as_str()), kind);
        }
        // Unknown values must not panic.
        assert_eq!(TokenKind::parse("future_kind"), TokenKind::Word);
    }

    #[test]
    fn normalized_term_lowercases() {
        let tokens = lexical_tokens("RUST");
        assert_eq!(tokens[0].normalized_term(), "rust");
    }

    #[test]
    fn counts_llm_tokens() {
        let count = count_llm_tokens("Hello, world!").expect("encoder loads");
        assert!(count > 0, "a non-empty string must have at least one token");
        // BPE tokens are not words: this string is 2 words but more tokens.
        assert!(count >= 2, "got {count}");
    }

    #[test]
    fn llm_token_count_is_zero_for_empty_input() {
        assert_eq!(count_llm_tokens("").expect("encoder loads"), 0);
    }

    #[test]
    fn llm_token_count_grows_with_length() {
        let short = count_llm_tokens("hello").expect("encoder");
        let long = count_llm_tokens("hello world this is a longer sentence").expect("encoder");
        assert!(long > short, "{long} must exceed {short}");
    }

    #[test]
    fn context_window_check_is_monotonic() {
        let text = "the quick brown fox jumps over the lazy dog";
        let count = count_llm_tokens(text).expect("encoder");
        assert!(fits_context_window(text, count).expect("check"));
        assert!(!fits_context_window(text, count - 1).expect("check"));
    }

    #[test]
    fn encodes_to_token_ids() {
        let ids = encode_llm_tokens("hello world").expect("encoder");
        assert!(!ids.is_empty());
        assert!(ids.iter().all(|id| *id < u32::MAX));
    }

    #[test]
    fn handles_unicode_text_without_panicking() {
        let tokens = tokenize("日本語のテキスト and emoji 🦀 mixed");
        assert!(!tokens.is_empty());
        // Every span must still be a valid slice boundary.
        let normalized = normalize("日本語のテキスト and emoji 🦀 mixed");
        for token in &tokens {
            assert!(
                normalized.is_char_boundary(token.byte_start)
                    && normalized.is_char_boundary(token.byte_end),
                "spans must land on char boundaries"
            );
        }
    }
}
