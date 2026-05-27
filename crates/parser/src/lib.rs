//! Sail raw lexer + token types.
//!
//! (`Token`, `Span`), the hand-written tokenizer (`hand_lexer`),
//! full-fidelity lexer (`lex_full`), and rowan SyntaxKind definitions.
//!
//! chumsky dependency eliminated; hand-written lexer replaces it.

mod event;
/// Grammar rules — Pratt parser for Sail source files.
///
/// in to align with RA's crate layout (grammar lives in parser crate).
mod grammar;
mod hand_lexer;
mod input;
mod lex_full;
mod lexer;
mod output;
mod parser;
mod shortcuts;
mod syntax_kind;
mod token_set;

pub use T_ as T;

pub use event::ParseError;
pub use grammar::FixityContext;
pub use hand_lexer::tokenize;
pub use input::Input;
pub use lex_full::single_token;
pub use lexer::{Span, Token};
pub use output::{Output, Step};
pub use shortcuts::{LexedStr, StrStep};
pub use syntax_kind::SyntaxKind;

pub(crate) use parser::Parser;
pub(crate) use token_set::TokenSet;

/// Sail literal values. Shared between parse-level AST and HIR.
/// Lives in `parser` so both `syntax` and `hir-def` can import it
/// without circular dependencies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Literal {
    Bool(bool),
    Unit,
    Number(String),
    Binary(String),
    Hex(String),
    String(String),
    Undefined,
    BitZero,
    BitOne,
}

/// Top-level parse entry points.
///
/// 4 variants (SourceFile, Expr, Type, Pattern). Sail implements the subset needed for LSP.
pub enum TopEntryPoint {
    /// Parse a complete Sail source file.
    SourceFile,
    /// Parse a single expression (for REPL/eval/hover).
    Expr,
    /// Parse a type annotation (for hover/completion).
    Type,
    /// Parse a pattern (for match arm completion).
    Pattern,
}

/// Prefix entry points for incremental parsing.
///
/// Used for parsing fragments within incremental re-parsing of sub-trees
/// and type annotation parsing in `lower.rs`.
pub enum PrefixEntryPoint {
    /// Parse a type expression.
    Ty,
    /// Parse an expression.
    Expr,
    /// Parse a pattern.
    Pat,
    /// Parse a top-level pattern (with alternatives).
    PatTop,
    /// Parse a statement.
    Stmt,
    /// Parse a path.
    Path,
    /// Parse a single top-level item.
    Item,
}

/// Incremental re-parser for a sub-tree.
/// Given a `SyntaxKind` identifying a node that was edited, returns
/// a `Reparser` that can re-parse just that node's token range.
pub struct Reparser(fn(&mut Parser<'_>));

impl Reparser {
    /// Check whether incremental re-parsing is possible for a node.
    /// Returns `Some(Reparser)` if the given `node` kind supports
    /// incremental re-parsing, `None` otherwise.
    /// Check whether incremental re-parsing is possible for a node.
    /// Returns `Some(Reparser)` if the given `node` kind supports
    /// incremental re-parsing, `None` otherwise.
    ///
    /// Supported node kinds (mirroring RA's 10 kinds, adapted for Sail):
    /// - `CALLABLE_DEF`: function/mapping definition body
    /// - `BLOCK_EXPR`: `{ ... }` block expression
    /// - `MATCH_ARM`: single match arm (pattern => expr)
    /// - `BODY`: body sub-structure
    /// - `NAMED_DEF`: struct/enum/union definitions
    /// - `TYPE_ALIAS_DEF`: type alias definitions
    /// - `SCATTERED_DEF`: scattered function/union/enum heads
    /// - `SCATTERED_CLAUSE_DEF`: scattered clauses
    pub fn for_node(
        node: SyntaxKind,
        first_child: Option<SyntaxKind>,
        _parent: Option<SyntaxKind>,
    ) -> Option<Reparser> {
        // Each reparser consumes all tokens and wraps in the correct node kind.
        // RA supports ~10 kinds (BLOCK_EXPR, RECORD_FIELD_LIST, VARIANT_LIST,
        // MATCH_ARM_LIST, etc.). Sail equivalents + Sail-specific kinds below.
        let res = match node {
            // Reparse a function/mapping definition using the real grammar.
            SyntaxKind::CALLABLE_DEF => |p: &mut Parser<'_>| {
                p.parse_definition();
            },
            // Reparse struct/enum/union/bitfield using real grammar.
            SyntaxKind::NAMED_DEF => |p: &mut Parser<'_>| {
                p.parse_definition();
            },
            // Reparse type alias using real grammar.
            SyntaxKind::TYPE_ALIAS_DEF => |p: &mut Parser<'_>| {
                p.parse_definition();
            },
            // Reparse scattered definition head.
            SyntaxKind::SCATTERED_DEF => |p: &mut Parser<'_>| {
                p.parse_definition();
            },
            // Reparse scattered clause.
            SyntaxKind::SCATTERED_CLAUSE_DEF => |p: &mut Parser<'_>| {
                p.parse_definition();
            },

            SyntaxKind::BLOCK_EXPR if first_child == Some(SyntaxKind::L_CURLY) => {
                |p: &mut Parser<'_>| {
                    p.parse_expr();
                }
            }

            // Reparse a match arm using real pattern + expression grammar.
            SyntaxKind::MATCH_ARM => |p: &mut Parser<'_>| {
                let m = p.start();
                p.parse_pattern(TokenSet::EMPTY);
                if p.at(SyntaxKind::FAT_R_ARROW) {
                    p.bump_any();
                }
                p.parse_expr();
                m.complete(p, SyntaxKind::MATCH_ARM);
            },
            // Body: reparse as expression.
            SyntaxKind::BODY => |p: &mut Parser<'_>| {
                let m = p.start();
                p.parse_expr();
                m.complete(p, SyntaxKind::BODY);
            },
            // Block item: reparse as expression (statement).
            SyntaxKind::BLOCK_ITEM => |p: &mut Parser<'_>| {
                let m = p.start();
                p.parse_expr();
                m.complete(p, SyntaxKind::BLOCK_ITEM);
            },

            _ => return None,
        };
        Some(Reparser(res))
    }

    /// Re-parse the given token range.
    pub fn parse(self, tokens: &Input) -> Output {
        let mut p = Parser::new(tokens);
        (self.0)(&mut p);
        let events = p.finish();
        crate::event::process(events)
    }
}

impl TopEntryPoint {
    /// Parse the given `Input` and produce an `Output`.
    /// This is the REAL entry point that calls grammar rules.
    /// The pipeline is: `LexedStr → Input → TopEntryPoint::parse → Output`
    pub fn parse(self, input: &Input) -> Output {
        let mut p = Parser::new(input);
        match self {
            TopEntryPoint::SourceFile => grammar::entry::top::source_file(&mut p),
            TopEntryPoint::Expr => {
                let m = p.start();
                grammar::entry::top::expr(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
            TopEntryPoint::Type => {
                let m = p.start();
                grammar::entry::top::type_(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
            TopEntryPoint::Pattern => {
                let m = p.start();
                grammar::entry::top::pattern(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
        }
        let events = p.finish();
        crate::event::process(events)
    }

    /// Parse with a fixity context for dynamic operator precedence.
    pub fn parse_with_fixities(
        self,
        input: &Input,
        fixities: crate::parser::FixityContext,
    ) -> Output {
        let mut p = Parser::new_with_fixities(input, fixities);
        match self {
            TopEntryPoint::SourceFile => grammar::entry::top::source_file(&mut p),
            TopEntryPoint::Expr => {
                let m = p.start();
                grammar::entry::top::expr(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
            TopEntryPoint::Type => {
                let m = p.start();
                grammar::entry::top::type_(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
            TopEntryPoint::Pattern => {
                let m = p.start();
                grammar::entry::top::pattern(&mut p);
                m.complete(&mut p, SyntaxKind::SOURCE_FILE);
            }
        }
        let events = p.finish();
        crate::event::process(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_entry_point_wraps_tokens_in_source_file() {
        let tokens = lex_full::lex_full_fidelity("val x : int\n");
        let input = Input::from_full_tokens(&tokens);
        let output = TopEntryPoint::SourceFile.parse(&input);
        let steps: Vec<_> = output.iter().collect();

        // First step: Enter(SOURCE_FILE)
        assert!(matches!(&steps[0], Step::Enter { kind } if *kind == SyntaxKind::SOURCE_FILE));
        // Last step must be Exit (closing SOURCE_FILE)
        assert!(matches!(&steps[steps.len() - 1], Step::Exit));
        // Should contain at least one token
        assert!(steps.iter().any(|s| matches!(s, Step::Token { .. })));
    }

    #[test]
    fn parse_produces_balanced_output() {
        let sources = &[
            "val x : int\n",
            "function f() = { let x = 1; x }\n",
            "function f(x : int, y : int) = x + y\n",
            "function foo() = {\n  let x = 1;\n  let y = 2;\n  y\n}\n",
        ];
        for source in sources {
            let lexed = LexedStr::new(source);
            let input = lexed.to_input_with_text(source);
            let output = TopEntryPoint::SourceFile.parse(&input);
            // Verify balanced tree: depth starts at 0, ends at 0
            let mut depth: i32 = 0;
            for step in output.iter() {
                match step {
                    Step::Enter { .. } => depth += 1,
                    Step::Exit => depth -= 1,
                    _ => {}
                }
            }
            assert_eq!(depth, 0, "unbalanced tree for {:?}", source);
        }
    }

    #[test]
    fn parse_no_errors_for_salsa_test_inputs() {
        let sources = &[
            "function f() = { let xs : list(bool) = [|true, 1|]; () }\n",
            "val f : (int, int) -> int\nfunction f(x, y) = x + y\n",
            "function foo() = {\n  let x = 1;\n  let y = 2;\n  y\n}\n",
        ];
        for source in sources {
            let lexed = LexedStr::new(source);
            let input = lexed.to_input_with_text(source);
            let output = TopEntryPoint::SourceFile.parse(&input);
            // No errors in output
            let has_error = output.iter().any(|s| matches!(s, Step::Error { .. }));
            assert!(!has_error, "errors for {:?}", source);
        }
    }
}
