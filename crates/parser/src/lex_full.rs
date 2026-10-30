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

/// Something the lexer can say about a byte range, in upstream's words.
///
/// rust-analyzer's `LexedStr` carries one of these per token (`error:
/// Vec<LexError>`, `lexed_str.rs`) and `build_tree` merges them into the
/// parse errors; this is the same channel. It exists because upstream
/// Sail's lexer raises on seven things this one recovered from in
/// silence — see `lex_errors`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    /// Byte offset the caret goes on.
    pub offset: usize,
    /// Upstream's message, verbatim.
    pub message: String,
}

/// Lex input preserving all whitespace and comments.
/// Every byte of input is covered by exactly one token span.
#[cfg(test)]
pub fn lex_full_fidelity(input: &str) -> Vec<(FullToken, Span)> {
    lex_full_fidelity_with_errors(input).0
}

/// Lex, and say what upstream's lexer would have raised on.
pub fn lex_full_fidelity_with_errors(input: &str) -> (Vec<(FullToken, Span)>, Vec<LexError>) {
    // Strategy: run the Chumsky lexer to get non-trivia tokens with
    // correct spans, then fill in the gaps with trivia tokens.
    use crate::hand_lexer::tokenize;

    let parsed = tokenize(input);

    if input.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut result = Vec::new();
    let mut errors = Vec::new();
    let mut cursor = 0;

    for (token, span) in &parsed {
        // Fill gap between cursor and this token with trivia
        if span.start > cursor {
            emit_trivia(input, cursor, span.start, &mut result, &mut errors);
        }
        result.push((FullToken::Token(token.clone()), *span));
        cursor = span.end;
    }

    // Fill trailing trivia after last token
    if cursor < input.len() {
        emit_trivia(input, cursor, input.len(), &mut result, &mut errors);
    }

    lex_errors(input, &parsed, &mut errors);
    errors.sort_by_key(|e| e.offset);
    (result, errors)
}

/// Classify a gap region (between two tokens) as whitespace and/or
/// comment trivia tokens.
fn emit_trivia(
    input: &str,
    start: usize,
    end: usize,
    out: &mut Vec<(FullToken, Span)>,
    errs: &mut Vec<LexError>,
) {
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
            // `line_comment ... | eof` raises "File ended before newline in
            // comment" (`lexer.mll:356`) and `doc_line_comment ... | eof`
            // its own wording (`:315`), at the `//` that started it. A
            // comment only reaches the end of a *gap* without a newline
            // when the gap is the end of the file.
            if pos == end && pos == input.len() {
                errs.push(LexError {
                    offset: seg_start,
                    message: if is_doc {
                        "File ended before newline in documentation comment".to_owned()
                    } else {
                        "File ended before newline in comment".to_owned()
                    },
                });
            }
            out.push((tok, Span::new(seg_start, pos)));
            continue;
        }

        // Block comment: /* ... */, nested.
        //
        // Two readers decided where a block comment ends and they
        // disagreed: `hand_lexer` counts depth, as upstream's
        // `block_comment` does (it carries one, `lexer.mll:346`), and this
        // stopped at the first `*/`. Nothing noticed for as long as the
        // leftover ` */` was emitted as whitespace in silence —
        // `test/typecheck/pass/scattered_mapping_doc.sail` is
        // `/*! ... /* nested */ */` and the outer closer came out as two
        // stray characters. It became visible the moment a stray character
        // was worth a diagnostic.
        if pos + 1 < end && bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
            pos += 2;
            let mut depth = 1u32;
            while pos + 1 < end && depth > 0 {
                if bytes[pos] == b'/' && bytes[pos + 1] == b'*' {
                    depth += 1;
                    pos += 2;
                } else if bytes[pos] == b'*' && bytes[pos + 1] == b'/' {
                    depth -= 1;
                    pos += 2;
                } else {
                    pos += 1;
                }
            }
            if depth > 0 {
                // `block_comment ... | eof` raises "Unbalanced comment"
                // (`lexer.mll:356`) and `doc_comment ... | eof` raises
                // "Unbalanced documentation comment" (`:343`) — a `/*!` is
                // lexed by a different rule up there, and says so.
                let doc = bytes.get(seg_start + 2) == Some(&b'!');
                pos = end;
                errs.push(LexError {
                    offset: seg_start,
                    message: if doc {
                        "Unbalanced documentation comment".to_owned()
                    } else {
                        "Unbalanced comment".to_owned()
                    },
                });
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

        // Unknown gap character — emit as whitespace, whole.
        //
        // The comment here said "shouldn't happen". It does: upstream's own
        // `test/lexing/extended_ascii.sail` is `let _ = \u{2122};`, and `sail`
        // reports "Unexpected character: \226" and carries on. Advancing one
        // *byte* gave this token a span ending inside the character, and
        // `LexedStr::text` panics slicing there — the parser crashed on a
        // file upstream merely rejects.
        errs.push(LexError {
            offset: seg_start,
            message: format!("Unexpected character: {}", char_escaped(bytes[pos])),
        });
        pos += crate::hand_lexer::utf8_len(bytes[pos]);
        out.push((FullToken::Whitespace, Span::new(seg_start, pos)));
    }
}

/// OCaml's `Char.escaped`, which is how upstream spells the byte it did
/// not recognise: `Unexpected character: \226` for the first byte of `™`.
fn char_escaped(b: u8) -> String {
    match b {
        b'\\' => "\\\\".to_owned(),
        b'"' => "\\\"".to_owned(),
        b'\'' => "\\'".to_owned(),
        b'\n' => "\\n".to_owned(),
        b'\t' => "\\t".to_owned(),
        b'\r' => "\\r".to_owned(),
        0x08 => "\\b".to_owned(),
        0x20..=0x7e => (b as char).to_string(),
        _ => format!("\\{b:03}"),
    }
}

/// What upstream's lexer raises on that this one lexes anyway.
///
/// `sail/src/lib/lexer.mll` raises `err_lex` in fourteen places. Every one
/// of them stops the file; this lexer recovered from all of them without a
/// word, so seven files upstream rejects outright drew **zero**
/// diagnostics here. The other seven are inside attributes, pragmas and
/// documentation comments, which this lexer takes as one token each and
/// does not look inside.
///
/// Each check below cites the rule it mirrors. Two of them are gaps rather
/// than tokens and live in `emit_trivia`: the unclosed `/*`, and the byte
/// no rule matches.
fn lex_errors(input: &str, tokens: &[(Token, Span)], errs: &mut Vec<LexError>) {
    for (token, span) in tokens {
        match token {
            // `("import", fun p -> raise (err_lex p "import is a reserved
            // keyword"))` and the same for `module` and `where`
            // (`lexer.mll`, upstream 0a0e2a017).
            // They are in the keyword table, so this is not "a name that
            // happens to be taken" — no position accepts them.
            Token::Id(name) if matches!(name.as_str(), "import" | "module" | "where") => {
                errs.push(LexError {
                    offset: span.start,
                    message: format!("{name} is a reserved keyword"),
                });
            }
            Token::String(_) => string_errors(input, *span, false, errs),
            Token::MultilineString(_) => {
                string_errors(input, *span, true, errs);
                multiline_prefix_errors(input, *span, errs);
            }
            Token::Directive { .. } => directive_errors(input, *span, errs),
            _ => {}
        }
    }

    // `"*/" { raise (err_lex ... "Unbalanced comment") }` (`lexer.mll:243`).
    // `*/` is a well-formed `operator2` upstream as well, and that rule
    // wins the tie only by sitting above the operator rule — so the
    // question is not what this lexer made of it but whether the two
    // characters are adjacent in the source. They come out `Multiply`
    // then `Divide` here, and no Sail expression puts those two side by
    // side: there is no prefix `/`.
    for pair in tokens.windows(2) {
        let [(a, sa), (b, sb)] = pair else { continue };
        if matches!(a, Token::Multiply) && matches!(b, Token::Divide) && sa.end == sb.start {
            errs.push(LexError { offset: sa.start, message: "Unbalanced comment".to_owned() });
        }
    }
}

/// What upstream's `pragma` sub-lexer raises about a directive's payload.
///
/// A directive here is one token — `$name` plus the rest of the line —
/// where upstream runs a sub-lexer over the same text (`lexer.mll:296`).
/// Its two `raise`s are about where the payload *stops*, which is a
/// question the token's own span answers.
fn directive_errors(input: &str, span: Span, errs: &mut Vec<LexError>) {
    // `| eof { raise (err_lex pos "File ended before newline in
    // directive") }` (`lexer.mll:309`), at the `$`. The token stops at the
    // newline, so reaching the end of the file *is* the missing newline.
    if span.end == input.len() {
        errs.push(LexError {
            offset: span.start,
            message: "File ended before newline in directive".to_owned(),
        });
    }

    // `$include <prelude.sail> /* block comment */ more` — "the pragma
    // cannot continue after a block comment", so that `$pragma fo/*
    // comment */o` does not silently have the value `foo` (`lexer.mll:293`
    // says exactly that, above the rule). Whitespace may follow, and a
    // `//` or another `/*` ends or continues it; anything else is the
    // error, at the character itself.
    let text = &input.as_bytes()[span.start..span.end];
    let mut i = 0;
    let mut after_block = false;
    while i < text.len() {
        if text[i] == b'/' && i + 1 < text.len() && text[i + 1] == b'*' {
            i += 2;
            while i + 1 < text.len() && !(text[i] == b'*' && text[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(text.len());
            after_block = true;
            continue;
        }
        if text[i] == b'/' && i + 1 < text.len() && text[i + 1] == b'/' {
            break;
        }
        if after_block && text[i] != b' ' && text[i] != b'\t' {
            errs.push(LexError {
                offset: span.start + i,
                message: "Directive continued after block comment".to_owned(),
            });
            break;
        }
        i += 1;
    }
}

/// Every line of a multi-line string starts with the closing delimiter's
/// own indentation.
///
/// `process_multiline` (`lexer.mll:142`) takes the last line — everything
/// between the final newline and the `"""` — as the prefix, requires it to
/// be whitespace, and then requires every other non-empty line to start
/// with it. That is what makes the literal's value independent of how far
/// the code around it is indented, and it is a *syntax* error rather than
/// a lexical one upstream.
///
/// "Line N" counts from the opening delimiter's line, which is what makes
/// it checkable against `test/lexing/mls_dedent.expect`: the offending
/// line is file line 8, the `"""` is on line 2, and upstream says 6.
fn multiline_prefix_errors(input: &str, span: Span, errs: &mut Vec<LexError>) {
    let text = &input[span.start..span.end];
    let Some(body) = text.strip_prefix("\"\"\"").and_then(|t| t.strip_suffix("\"\"\"")) else {
        return; // unterminated; `string_errors` has already said so
    };
    let mut lines: Vec<&str> = body.split('\n').collect();
    let Some(prefix) = lines.pop() else { return };
    if !prefix.chars().all(|c| c == ' ' || c == '\t') {
        errs.push(LexError {
            offset: span.start,
            message: "Final line before \"\"\" delimiter in multi-line string must only \
                      contain whitespace"
                .to_owned(),
        });
        return;
    }

    // Offset of each line, so the caret lands on the offending line rather
    // than on the literal.
    let mut at = span.start + "\"\"\"".len();
    for (n, line) in lines.iter().enumerate() {
        if !line.is_empty() && !line.starts_with(prefix) {
            errs.push(LexError {
                offset: at,
                message: format!(
                    "Line {n} in multi-line string literal has an invalid prefix.\n\
                     It must contain the exact sequence of whitespace characters that \
                     preceed the final \"\"\" delimiter.",
                ),
            });
            return;
        }
        at += line.len() + 1;
    }
}

/// The two things upstream raises about a string's contents.
///
/// Termination is judged on the token's own text rather than on what the
/// lexer stored, because a multiline string keeps only its contents and a
/// regular one keeps its quotes — the source text is the same question
/// either way.
fn string_errors(input: &str, span: Span, multiline: bool, errs: &mut Vec<LexError>) {
    let text = &input[span.start..span.end];
    let (open, message) = if multiline {
        ("\"\"\"", "Multiline string literal not terminated")
    } else {
        ("\"", "String literal not terminated")
    };
    let body = &text[open.len().min(text.len())..];
    if !(body.len() >= open.len() && body.ends_with(open)) {
        // `| eof { raise (err_lex pos "...not terminated") }`
        // (`lexer.mll:367`, `:378`) — `pos` is the opening quote.
        errs.push(LexError { offset: span.start, message: message.to_owned() });
    }

    // `escape_sequence = ('\\' ['\\''"''\''  'n''t''b''r']) | ('\\' digit
    // digit digit) | ('\\' 'x' hexdigit hexdigit)` (`lexer.mll:190`), plus
    // the line continuation `'\\' '\n' ws` (`:363`); anything else after a
    // backslash falls to `| '\\' { raise ... }`.
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'\\' {
            i += 1;
            continue;
        }
        let rest = &bytes[i + 1..];
        let len = match rest.first() {
            Some(b'\\' | b'"' | b'\'' | b'n' | b't' | b'b' | b'r' | b'\n') => 2,
            Some(b'0'..=b'9') if rest.len() >= 3 && rest[1..3].iter().all(u8::is_ascii_digit) => 4,
            Some(b'x') if rest.len() >= 3 && rest[1..3].iter().all(u8::is_ascii_hexdigit) => 4,
            _ => {
                errs.push(LexError {
                    offset: span.start + i,
                    message: "String literal contains illegal backslash escape sequence".to_owned(),
                });
                2
            }
        };
        i += len;
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
    let (tokens, errors) = lex_full_fidelity_with_errors(text);
    if tokens.len() == 1 {
        let kind = tokens[0].0.syntax_kind();
        Some((kind, errors.first().map(|error| error.message.clone())))
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
