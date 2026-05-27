//! Full-fidelity lexer that preserves whitespace and comments.
//!
//! Unlike the Chumsky-based `lexer()` which strips trivia, this lexer
//! produces a token for every byte in the source. Concatenating all
//! token texts reconstructs the original input exactly.
//!
//! Used by the event-based parser (M-stage+) for building lossless
//! rowan GreenNodes.

use crate::lexer::{Span, Token};

/// Full-fidelity token — wraps `Token` with trivia variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FullToken {
    Token(Token),
    Whitespace,
    LineComment,
    BlockComment,
    /// `///` doc comment (NOT `////`). Preserved as non-trivia so
    /// ItemTree can extract documentation for hover display.
    DocComment,
}

impl FullToken {
    pub fn syntax_kind(&self) -> crate::syntax_kind::SyntaxKind {
        use crate::syntax_kind::SyntaxKind;
        match self {
            FullToken::Token(tok) => SyntaxKind::from_token(tok),
            FullToken::Whitespace => SyntaxKind::WHITESPACE,
            FullToken::LineComment => SyntaxKind::LINE_COMMENT,
            FullToken::BlockComment => SyntaxKind::BLOCK_COMMENT,
            FullToken::DocComment => SyntaxKind::DOC_COMMENT,
        }
    }

    pub fn is_trivia(&self) -> bool {
        matches!(self, FullToken::Whitespace | FullToken::LineComment | FullToken::BlockComment)
    }
}

/// Lex input preserving all whitespace and comments.
/// Every byte of input is covered by exactly one token span.
pub fn lex_full_fidelity(input: &str) -> Vec<(FullToken, Span)> {
    // Strategy: run the Chumsky lexer to get non-trivia tokens with
    // correct spans, then fill in the gaps with trivia tokens.
    use crate::hand_lexer::tokenize;

    let parsed = tokenize(input);

    if input.is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    let mut cursor = 0;

    for (token, span) in &parsed {
        // Fill gap between cursor and this token with trivia
        if span.start > cursor {
            emit_trivia(input, cursor, span.start, &mut result);
        }
        result.push((FullToken::Token(token.clone()), *span));
        cursor = span.end;
    }

    // Fill trailing trivia after last token
    if cursor < input.len() {
        emit_trivia(input, cursor, input.len(), &mut result);
    }

    result
}

/// Classify a gap region (between two tokens) as whitespace and/or
/// comment trivia tokens.
fn emit_trivia(input: &str, start: usize, end: usize, out: &mut Vec<(FullToken, Span)>) {
    let bytes = input.as_bytes();
    let mut pos = start;

    while pos < end {
        let seg_start = pos;

        // Line comment: // ... or doc comment: /// ...
        if pos + 1 < end && bytes[pos] == b'/' && bytes[pos + 1] == b'/' {
            pos += 2;
            while pos < end && bytes[pos] != b'\n' {
                pos += 1;
            }
            // Detect `///` doc comment (but NOT `////` which is a section divider)
            let is_doc = seg_start + 2 < end
                && bytes[seg_start + 2] == b'/'
                && !(seg_start + 3 < end && bytes[seg_start + 3] == b'/');
            let tok = if is_doc { FullToken::DocComment } else { FullToken::LineComment };
            out.push((tok, Span::new(seg_start, pos)));
            continue;
        }

        // Block comment: /* ... */
        if pos + 1 < end && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
            pos += 2;
            while pos + 1 < end && !(bytes[pos] == b'*' && bytes[pos + 1] == b'/') {
                pos += 1;
            }
            if pos + 1 < end {
                pos += 2;
            }
            out.push((FullToken::BlockComment, Span::new(seg_start, pos)));
            continue;
        }

        // ML block comment: (* ... *)
        if pos + 1 < end && bytes[pos] == b'(' && bytes[pos + 1] == b'*' {
            pos += 2;
            while pos + 1 < end && !(bytes[pos] == b'*' && bytes[pos + 1] == b')') {
                pos += 1;
            }
            if pos + 1 < end {
                pos += 2;
            }
            out.push((FullToken::BlockComment, Span::new(seg_start, pos)));
            continue;
        }

        // Whitespace
        if bytes[pos].is_ascii_whitespace() {
            while pos < end && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            out.push((FullToken::Whitespace, Span::new(seg_start, pos)));
            continue;
        }

        // Unknown gap character — emit as whitespace (shouldn't happen)
        pos += 1;
        out.push((FullToken::Whitespace, Span::new(seg_start, pos)));
    }
}

/// Attempt to lex `text` as a single token.
///
/// Returns `Some((kind, error))` if the entire text forms exactly one
/// token (including trivia). Returns `None` if the text would produce
/// zero or more than one token.
pub fn single_token(text: &str) -> Option<(crate::syntax_kind::SyntaxKind, Option<String>)> {
    if text.is_empty() {
        return None;
    }
    let tokens = lex_full_fidelity(text);
    if tokens.len() == 1 {
        let kind = tokens[0].0.syntax_kind();
        Some((kind, None))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_lossless(input: &str) {
        let tokens = lex_full_fidelity(input);
        let reconstructed: String =
            tokens.iter().map(|(_, span)| &input[span.start..span.end]).collect();
        assert_eq!(reconstructed, input, "lossless round-trip failed");
    }

    #[test]
    fn lossless_simple() {
        assert_lossless("val x : int\n");
    }

    #[test]
    fn lossless_with_comment() {
        assert_lossless("// comment\nval x : int\n");
    }

    #[test]
    fn lossless_with_block_comment() {
        assert_lossless("/* block */ val x : int\n");
    }

    #[test]
    fn lossless_with_ml_comment() {
        assert_lossless("(* ml comment *) val x : int\n");
    }

    #[test]
    fn lossless_function() {
        assert_lossless("function f(x : int) -> int = x + 1\n");
    }

    #[test]
    fn preserves_whitespace_tokens() {
        let tokens = lex_full_fidelity("val   x\n");
        let ws_count = tokens.iter().filter(|(t, _)| matches!(t, FullToken::Whitespace)).count();
        assert!(ws_count >= 1);
    }

    #[test]
    fn preserves_comment_tokens() {
        let tokens = lex_full_fidelity("// hello\nval x : int\n");
        let comment_count =
            tokens.iter().filter(|(t, _)| matches!(t, FullToken::LineComment)).count();
        assert_eq!(comment_count, 1);
    }

    #[test]
    fn identifies_keywords() {
        let tokens = lex_full_fidelity("val foo : int\n");
        let non_trivia: Vec<_> = tokens.iter().filter(|(t, _)| !t.is_trivia()).collect();
        assert!(non_trivia.len() >= 4); // val, foo, :, int
        assert!(matches!(&non_trivia[0].0, FullToken::Token(Token::KwVal)));
    }

    #[test]
    fn empty_input() {
        let tokens = lex_full_fidelity("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn only_whitespace() {
        let input = "  \n\t  \n";
        assert_lossless(input);
        let tokens = lex_full_fidelity(input);
        assert!(tokens.iter().all(|(t, _)| t.is_trivia()));
    }

    #[test]
    fn lossless_multiline() {
        assert_lossless("val x : int\nfunction f() = 42\n// end\n");
    }

    #[test]
    fn doc_comment_detected() {
        let tokens = lex_full_fidelity("/// This is a doc\nval x : int\n");
        let doc_count = tokens.iter().filter(|(t, _)| matches!(t, FullToken::DocComment)).count();
        assert_eq!(doc_count, 1, "should detect one doc comment");
        // Doc comments are NOT trivia
        assert!(!FullToken::DocComment.is_trivia());
    }

    #[test]
    fn regular_comment_not_doc() {
        let tokens = lex_full_fidelity("// regular comment\nval x : int\n");
        let doc_count = tokens.iter().filter(|(t, _)| matches!(t, FullToken::DocComment)).count();
        assert_eq!(doc_count, 0, "regular comment should not be doc");
    }

    #[test]
    fn quadruple_slash_not_doc() {
        let tokens = lex_full_fidelity("//// section divider\nval x : int\n");
        let doc_count = tokens.iter().filter(|(t, _)| matches!(t, FullToken::DocComment)).count();
        assert_eq!(doc_count, 0, "//// should not be doc comment");
    }
}
