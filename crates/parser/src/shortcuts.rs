//! Convenience bridge between the parser and the outside world.
//!
//! reconstructing a rowan `GreenNode` from parser `Output`, and
//! `LexedStr::intersperse_trivia` for weaving trivia tokens back in.

use crate::lex_full::{lex_full_fidelity, FullToken};
use crate::lexer::Span;
use crate::output::{Output, Step};
use crate::syntax_kind::SyntaxKind;

/// A single step when building a syntax tree from source text.
/// Unlike `Step` (which uses indices into `Input`), `StrStep` carries
/// the actual text slice and byte positions for errors — ready for
/// direct consumption by the tree builder.
pub enum StrStep<'a> {
    /// A token with its concrete text.
    Token { kind: SyntaxKind, text: &'a str },
    /// Enter a composite node.
    Enter { kind: SyntaxKind },
    /// Exit a composite node.
    Exit,
    /// A parse error at a byte position.
    Error { msg: &'a str, pos: usize },
}

/// A tokenized source string — pairs each token with its text slice.
///
/// wraps the full-fidelity token stream from `lex_full`.
pub struct LexedStr<'a> {
    text: &'a str,
    tokens: Vec<(FullToken, Span)>,
}

impl<'a> LexedStr<'a> {
    /// Tokenize source text.
    pub fn new(text: &'a str) -> Self {
        let tokens = lex_full_fidelity(text);
        Self { text, tokens }
    }

    /// The original source text.
    pub fn as_str(&self) -> &str {
        self.text
    }

    /// Number of tokens (including trivia).
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Kind of the `i`-th token.
    pub fn kind(&self, i: usize) -> SyntaxKind {
        self.tokens[i].0.syntax_kind()
    }

    /// Text of the `i`-th token.
    pub fn text(&self, i: usize) -> &str {
        let span = &self.tokens[i].1;
        &self.text[span.start..span.end]
    }

    /// Build an `Input` from non-trivia tokens (for feeding to the parser).
    pub fn to_input(&self) -> crate::Input {
        let mut input = crate::Input::with_capacity(self.tokens.len());
        for (tok, span) in &self.tokens {
            if !tok.is_trivia() {
                input.push(tok.syntax_kind(), (span.end - span.start) as u32);
            }
        }
        input
    }

    /// Build an `Input` with token texts (for dynamic fixity lookup).
    ///
    /// Like `to_input` but also stores the text of each non-trivia token,
    /// which the parser needs for dynamic fixity resolution.
    pub fn to_input_with_text(&self, source: &str) -> crate::Input {
        let mut input = crate::Input::with_capacity(self.tokens.len());
        for (tok, span) in &self.tokens {
            if !tok.is_trivia() {
                let text = source[span.start..span.end].to_string();
                input.push_with_text(tok.syntax_kind(), (span.end - span.start) as u32, text);
            }
        }
        input
    }

    /// Interleave parser output with trivia tokens, calling `sink`
    /// for each resulting step.
    /// Returns `true` if all tokens were consumed without error.
    pub fn intersperse_trivia(&self, output: &Output, sink: &mut dyn FnMut(StrStep<'_>)) -> bool {
        let mut token_idx = 0usize; // index into self.tokens
        let mut error = false;
        let mut is_first_enter = true;

        for step in output.iter() {
            match step {
                Step::Token { kind, n_input_tokens } => {
                    // Emit leading trivia before this non-trivia token
                    token_idx = self.emit_trivia(token_idx, sink);

                    // Emit the actual token(s)
                    let n = n_input_tokens as usize;
                    if n == 1 && token_idx < self.tokens.len() {
                        let span = &self.tokens[token_idx].1;
                        let text = &self.text[span.start..span.end];
                        sink(StrStep::Token { kind, text });
                        token_idx += 1;
                    } else {
                        // Multi-token: concatenate text spans
                        for _ in 0..n {
                            if token_idx < self.tokens.len() {
                                let span = &self.tokens[token_idx].1;
                                let text = &self.text[span.start..span.end];
                                sink(StrStep::Token { kind, text });
                                token_idx += 1;
                            }
                        }
                    }
                }
                Step::Enter { kind } => {
                    if is_first_enter {
                        // For the root node, emit Enter first, then leading
                        // trivia goes inside the node (required by rowan —
                        // can't add tokens before any node is open).
                        sink(StrStep::Enter { kind });
                        token_idx = self.emit_trivia(token_idx, sink);
                        is_first_enter = false;
                    } else {
                        // For inner nodes, emit trivia before entering.
                        token_idx = self.emit_trivia(token_idx, sink);
                        sink(StrStep::Enter { kind });
                    }
                }
                Step::Exit => {
                    sink(StrStep::Exit);
                }
                Step::Error { msg } => {
                    let pos = if token_idx < self.tokens.len() {
                        self.tokens[token_idx].1.start
                    } else {
                        self.text.len()
                    };
                    sink(StrStep::Error { msg, pos });
                    error = true;
                }
            }
        }

        // Emit any trailing trivia (inside the root node since Exit hasn't fired yet)
        self.emit_trivia(token_idx, sink);

        !error
    }

    /// Emit consecutive trivia tokens starting at `idx`.
    /// Returns the index of the next non-trivia token.
    fn emit_trivia(&self, mut idx: usize, sink: &mut dyn FnMut(StrStep<'_>)) -> usize {
        while idx < self.tokens.len() && self.tokens[idx].0.is_trivia() {
            let kind = self.tokens[idx].0.syntax_kind();
            let span = &self.tokens[idx].1;
            let text = &self.text[span.start..span.end];
            sink(StrStep::Token { kind, text });
            idx += 1;
        }
        idx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexed_str_basic() {
        let ls = LexedStr::new("val x : int\n");
        assert!(!ls.is_empty());
        assert_eq!(ls.as_str(), "val x : int\n");
        // First non-trivia token should be "val"
        assert_eq!(ls.text(0), "val");
    }

    #[test]
    fn to_input_filters_trivia() {
        let ls = LexedStr::new("val x : int\n");
        let input = ls.to_input();
        // Should have 4 non-trivia tokens: val, x, :, int
        assert_eq!(input.len(), 4);
    }

    #[test]
    fn intersperse_trivia_roundtrip() {
        let ls = LexedStr::new("val x : int\n");
        let input = ls.to_input();
        let output = crate::TopEntryPoint::SourceFile.parse(&input);

        let mut steps = Vec::new();
        let ok = ls.intersperse_trivia(&output, &mut |step| {
            steps.push(match step {
                StrStep::Token { kind, text } => format!("Token({:?}, {:?})", kind, text),
                StrStep::Enter { kind } => format!("Enter({:?})", kind),
                StrStep::Exit => "Exit".to_string(),
                StrStep::Error { msg, pos } => format!("Error({:?}, {})", msg, pos),
            });
        });
        assert!(ok, "should succeed without errors");
        // Should have Enter(SOURCE_FILE), tokens (trivia+non-trivia)..., Exit, trailing trivia
        assert!(steps[0].contains("Enter"));
        // Exit may not be the absolute last step due to trailing trivia
        assert!(steps.iter().any(|s| s == "Exit"));
    }
}
