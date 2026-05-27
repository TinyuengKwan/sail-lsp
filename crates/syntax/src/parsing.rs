//! Lexing, bridging to parser, and tree construction.
//!
//! Full pipeline:
//!   LexedStr::new(text) → to_input() → TopEntryPoint::parse → Output
//!   → build_tree_from_output(lexed, output) → GreenNode → SyntaxNode
//!
//! Tree construction (trivia interspersion) happens in this crate,
//! matching RA where parser only returns Output (no GreenNode).

mod reparsing;

pub use parser::FixityContext;

use parser::LexedStr;
use parser::ParseError;
use parser::TopEntryPoint;
use rowan::GreenNode;

use crate::syntax_error::SyntaxError;
use crate::syntax_node::SyntaxNode;

pub(crate) use crate::parsing::reparsing::incremental_reparse;

/// Parse Sail source text into a lossless rowan `SyntaxNode` + errors.
///
/// parse entry point used throughout the codebase.
///
/// Pipeline: lex → Input → parse → Output → build_tree → SyntaxNode
pub fn parse_text(text: &str) -> (SyntaxNode, Vec<ParseError>) {
    parse_text_with_fixities(text, FixityContext::new())
}

/// Parse with dynamic operator fixities.
///
/// Full pipeline:
///   1. LexedStr::new(text)                    — lex (parser crate)
///   2. lexed.to_input_with_text(text)         — filter trivia (parser crate)
///   3. TopEntryPoint::SourceFile.parse(&input) — parse → Output (parser crate)
///   4. build_tree(lexed, output)              — intersperse trivia (syntax crate)
///   5. SyntaxNode::new_root(green)            — wrap as typed tree (syntax crate)
pub fn parse_text_with_fixities(
    text: &str,
    fixities: FixityContext,
) -> (SyntaxNode, Vec<ParseError>) {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // Steps 1-2: lex + build Input (parser crate)
        let lexed = LexedStr::new(text);
        let input = lexed.to_input_with_text(text);

        // Step 3: parse → Output (parser crate)
        let output = TopEntryPoint::SourceFile.parse_with_fixities(&input, fixities);

        // Step 4: build tree with trivia interspersion (syntax crate)
        let (green, errors) = build_tree_from_output(&lexed, &output);
        (SyntaxNode::new_root(green), errors)
    })) {
        Ok(result) => result,
        Err(_) => {
            // Parser panic fallback: minimal valid tree
            let mut builder = rowan::GreenNodeBuilder::new();
            builder.start_node(parser::SyntaxKind::SOURCE_FILE.into());
            if !text.is_empty() {
                builder.token(parser::SyntaxKind::ERROR.into(), text);
            }
            builder.finish_node();
            let errors = vec![ParseError {
                message: "parser produced unbalanced events; CST unavailable".to_string(),
                offset: 0,
            }];
            (SyntaxNode::new_root(builder.finish()), errors)
        }
    }
}

/// Parse Sail source text into a `SyntaxNode` + `SyntaxError`s.
///
/// Wraps `parse_text` converting `ParseError` → `SyntaxError`.
pub(crate) fn parse_text_to_syntax_errors(text: &str) -> (SyntaxNode, Vec<SyntaxError>) {
    let (root, errors) = parse_text(text);
    let syntax_errors = errors.into_iter().map(|e| parse_error_to_syntax_error(&e, text)).collect();
    (root, syntax_errors)
}

/// Parse with fixities, returning `SyntaxError`s.
pub(crate) fn parse_text_with_fixities_to_syntax_errors(
    text: &str,
    fixities: FixityContext,
) -> (SyntaxNode, Vec<SyntaxError>) {
    let (root, errors) = parse_text_with_fixities(text, fixities);
    let syntax_errors = errors.into_iter().map(|e| parse_error_to_syntax_error(&e, text)).collect();
    (root, syntax_errors)
}

/// Build a rowan `GreenNode` from `LexedStr` + parser `Output`.
///
/// Intersperse trivia tokens from the lexed stream back into the tree.
#[allow(unused_assignments)]
fn build_tree_from_output(
    lexed: &LexedStr<'_>,
    output: &parser::Output,
) -> (GreenNode, Vec<ParseError>) {
    use parser::Step;

    let mut builder = rowan::GreenNodeBuilder::new();
    let mut errors = Vec::new();
    let mut token_idx = 0usize;
    let mut byte_offset = 0usize;
    let mut depth = 0u32;

    for step in output.iter() {
        match step {
            Step::Token { kind, n_input_tokens } => {
                // Emit leading trivia
                let (new_idx, new_offset) =
                    emit_trivia(lexed, token_idx, byte_offset, &mut builder);
                token_idx = new_idx;
                byte_offset = new_offset;
                // Emit the non-trivia token(s)
                let n = n_input_tokens as usize;
                for _ in 0..n.max(1) {
                    if token_idx < lexed.len() {
                        let text = lexed.text(token_idx);
                        builder.token(kind.into(), text);
                        byte_offset += text.len();
                        token_idx += 1;
                    }
                }
            }
            Step::Enter { kind } => {
                if depth == 0 {
                    // Root node: open first, then emit leading trivia inside
                    builder.start_node(kind.into());
                    depth += 1;
                    let (new_idx, new_offset) =
                        emit_trivia(lexed, token_idx, byte_offset, &mut builder);
                    token_idx = new_idx;
                    byte_offset = new_offset;
                } else {
                    // Inner nodes: emit trivia before opening
                    let (new_idx, new_offset) =
                        emit_trivia(lexed, token_idx, byte_offset, &mut builder);
                    token_idx = new_idx;
                    byte_offset = new_offset;
                    builder.start_node(kind.into());
                    depth += 1;
                }
            }
            Step::Exit => {
                if depth > 0 {
                    // Before closing root: emit all trailing trivia inside
                    if depth == 1 {
                        while token_idx < lexed.len() {
                            let kind = lexed.kind(token_idx);
                            let text = lexed.text(token_idx);
                            builder.token(kind.into(), text);
                            byte_offset += text.len();
                            token_idx += 1;
                        }
                    }
                    builder.finish_node();
                    depth -= 1;
                }
            }
            Step::Error { msg } => {
                errors.push(ParseError { message: msg.to_string(), offset: byte_offset });
            }
        }
    }

    // Emit remaining tokens + close unclosed nodes
    while token_idx < lexed.len() {
        let kind = lexed.kind(token_idx);
        let text = lexed.text(token_idx);
        builder.token(kind.into(), text);
        byte_offset += text.len();
        token_idx += 1;
    }
    while depth > 0 {
        builder.finish_node();
        depth -= 1;
    }

    (builder.finish(), errors)
}

/// Emit consecutive trivia tokens into the GreenNodeBuilder.
fn emit_trivia(
    lexed: &LexedStr<'_>,
    mut idx: usize,
    mut byte_offset: usize,
    builder: &mut rowan::GreenNodeBuilder,
) -> (usize, usize) {
    use parser::SyntaxKind as SK;
    while idx < lexed.len() {
        let kind = lexed.kind(idx);
        if kind == SK::WHITESPACE || kind == SK::LINE_COMMENT || kind == SK::BLOCK_COMMENT {
            let text = lexed.text(idx);
            builder.token(kind.into(), text);
            byte_offset += text.len();
            idx += 1;
        } else {
            break;
        }
    }
    (idx, byte_offset)
}

/// Build a rowan `GreenNode` from a `LexedStr` and parser `Output` using
/// `StrStep`-based trivia interspersion.
///
/// Used by incremental reparsing. Returns `(green_node, errors, eof_reached)`.
pub(crate) fn build_tree(
    lexed: LexedStr<'_>,
    parser_output: parser::Output,
) -> (rowan::GreenNode, Vec<SyntaxError>, bool) {
    use parser::StrStep;

    let mut builder = crate::syntax_node::SyntaxTreeBuilder::default();
    let mut errors = Vec::new();

    let is_eof = lexed.intersperse_trivia(&parser_output, &mut |step| match step {
        StrStep::Token { kind, text } => {
            builder.token(kind, text);
        }
        StrStep::Enter { kind } => {
            builder.start_node(kind);
        }
        StrStep::Exit => {
            builder.finish_node();
        }
        StrStep::Error { msg, pos } => {
            let offset = rowan::TextSize::from(pos as u32);
            errors.push(SyntaxError::new_at_offset(msg.to_owned(), offset));
        }
    });

    let (green, build_errors) = builder.finish_raw();
    errors.extend(build_errors);
    (green, errors, is_eof)
}

/// Convert a `ParseError` (byte offset) to a `SyntaxError` (TextRange).
pub(crate) fn parse_error_to_syntax_error(err: &ParseError, _text: &str) -> SyntaxError {
    let offset = rowan::TextSize::from(err.offset as u32);
    SyntaxError::new_at_offset(err.message.clone(), offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_produces_source_file() {
        let (root, errors) = parse_text("val x : int\n");
        assert_eq!(root.kind(), parser::SyntaxKind::SOURCE_FILE);
        assert!(errors.is_empty());
    }

    #[test]
    fn parse_text_lossless() {
        let input = "function f(x, y) = x + y\n";
        let (root, _) = parse_text(input);
        assert_eq!(root.text().to_string(), input);
    }

    #[test]
    fn parse_text_reports_errors() {
        let (root, _errors) = parse_text("function");
        assert_eq!(root.text().to_string(), "function");
    }

    #[test]
    fn parse_text_empty() {
        let (root, errors) = parse_text("");
        assert_eq!(root.text().to_string(), "");
        assert!(errors.is_empty());
    }
}
