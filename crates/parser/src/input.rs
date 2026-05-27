//! Abstract token input for the parser.
//!
//! of pre-classified `SyntaxKind` tokens, decoupled from the concrete
//! lexer representation (`Token`, `Span`, `FullToken`).
//!
//! Sail-specific: each token also carries its byte length so the tree
//! builder can reconstruct source text spans. RA doesn't need this
//! because `rustc_lexer` gives `n_raw_tokens` which the tree builder
//! resolves against the original `LexedStr`.

use crate::syntax_kind::SyntaxKind;

/// Pre-classified token stream for the parser.
///
/// Stores only non-trivia tokens. Trivia tokens (whitespace, comments)
/// are tracked separately in `trivia` for the tree builder to
/// intersperse during `GreenNode` construction.
pub struct Input {
    /// Non-trivia token kinds, in source order.
    kind: Vec<SyntaxKind>,
    /// Byte length of each non-trivia token.
    token_len: Vec<u32>,
    /// Token text for each non-trivia token.
    /// Sail-specific: needed for dynamic fixity lookup where the parser
    /// must inspect the text of IDENT tokens to resolve operator precedence.
    token_text: Vec<String>,
    // No `joint` bitfield — Sail has no `->` vs `- >` ambiguity
    // (operators are lexed as single tokens).
    // No `contextual_kind` — Sail has no contextual keywords.
    // No `edition` — Sail has no edition system.
}

impl Input {
    /// Create an empty `Input` with the given capacity hint.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            kind: Vec::with_capacity(capacity),
            token_len: Vec::with_capacity(capacity),
            token_text: Vec::with_capacity(capacity),
        }
    }

    /// Push a non-trivia token.
    pub fn push(&mut self, kind: SyntaxKind, len: u32) {
        self.kind.push(kind);
        self.token_len.push(len);
        self.token_text.push(String::new());
    }

    /// Push a non-trivia token with its text.
    pub fn push_with_text(&mut self, kind: SyntaxKind, len: u32, text: String) {
        self.kind.push(kind);
        self.token_len.push(len);
        self.token_text.push(text);
    }

    /// Number of non-trivia tokens.
    pub fn len(&self) -> usize {
        self.kind.len()
    }

    /// Whether the input is empty.
    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    /// Token kind at position `idx`. Returns `EOF` if out of bounds.
    pub(crate) fn kind(&self, idx: usize) -> SyntaxKind {
        self.kind.get(idx).copied().unwrap_or(SyntaxKind::EOF)
    }

    /// Byte length of the token at position `idx`.
    /// Panics if out of bounds (only called for valid positions).
    #[allow(dead_code)] // TODO: used in tests, will be needed by incremental reparsing
    pub(crate) fn token_len(&self, idx: usize) -> u32 {
        self.token_len[idx]
    }

    /// Text of the token at position `idx`. Returns empty string if out of bounds.
    ///
    /// Sail-specific: used for dynamic fixity lookup where the parser
    /// must inspect IDENT text to resolve operator precedence.
    pub(crate) fn token_text(&self, idx: usize) -> &str {
        self.token_text.get(idx).map(|s| s.as_str()).unwrap_or("")
    }
}

/// Build an `Input` from a full-fidelity token stream.
///
/// Filters out trivia tokens, converting each non-trivia `FullToken`
/// into a `(SyntaxKind, byte_len)` pair.
impl Input {
    pub fn from_full_tokens(tokens: &[(crate::lex_full::FullToken, crate::lexer::Span)]) -> Self {
        let mut input = Self::with_capacity(tokens.len());
        for (tok, span) in tokens {
            if !tok.is_trivia() {
                input.push(tok.syntax_kind(), (span.end - span.start) as u32);
            }
        }
        input
    }

    /// Build an `Input` from a full-fidelity token stream with text.
    ///
    /// Like `from_full_tokens` but also stores token text for fixity lookup.
    pub fn from_full_tokens_with_text(
        tokens: &[(crate::lex_full::FullToken, crate::lexer::Span)],
        source: &str,
    ) -> Self {
        let mut input = Self::with_capacity(tokens.len());
        for (tok, span) in tokens {
            if !tok.is_trivia() {
                let text = source[span.start..span.end].to_string();
                input.push_with_text(tok.syntax_kind(), (span.end - span.start) as u32, text);
            }
        }
        input
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input() {
        let input = Input::with_capacity(0);
        assert!(input.is_empty());
        assert_eq!(input.len(), 0);
        assert_eq!(input.kind(0), SyntaxKind::EOF);
        assert_eq!(input.kind(100), SyntaxKind::EOF);
    }

    #[test]
    fn push_and_access() {
        let mut input = Input::with_capacity(2);
        input.push(SyntaxKind::IDENT, 3);
        input.push(SyntaxKind::PLUS, 1);
        assert_eq!(input.len(), 2);
        assert_eq!(input.kind(0), SyntaxKind::IDENT);
        assert_eq!(input.kind(1), SyntaxKind::PLUS);
        assert_eq!(input.kind(2), SyntaxKind::EOF);
        assert_eq!(input.token_len(0), 3);
        assert_eq!(input.token_len(1), 1);
    }

    #[test]
    fn from_full_tokens_filters_trivia() {
        use crate::lex_full::lex_full_fidelity;
        let tokens = lex_full_fidelity("val x : int\n");
        let input = Input::from_full_tokens(&tokens);
        // Should have only non-trivia tokens: "val", "x", ":", "int"
        assert_eq!(input.len(), 4);
        assert_eq!(input.kind(0), SyntaxKind::KW_VAL);
        assert_eq!(input.kind(1), SyntaxKind::IDENT);
        assert_eq!(input.kind(2), SyntaxKind::COLON);
        assert_eq!(input.kind(3), SyntaxKind::IDENT); // "int" is an IDENT
    }
}
