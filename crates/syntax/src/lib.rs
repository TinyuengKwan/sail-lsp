//! Sail syntax layer — rowan CST, typed AST wrappers, and parse pipeline.
//!
//! lossless CST (`parsing`), typed AST wrappers (`ast`),
//! stable syntax pointers (`ptr`), CST→ParsedFile lowering
//! (`cst_lower`), salsa parse queries (`parse_query`), and the
//! preprocessor symbol/option definitions (`preprocess`).
//!
//! # Alignment notes (see DIFF_NOTES.md for full details)
//!
//! ALIGN — shape matches RA; minor implementation difference noted inline.
//! CUSTOM — intentional divergence from RA; no alignment planned.
//! TODO   — alignment is desirable but deferred.

// validation are all private modules with precise re-exports.
#[cfg(test)]
mod grammar_tests;
mod parsing;
mod ptr;
mod syntax_error;
mod syntax_node;
mod token_text;
mod validation;

pub mod algo;
pub mod ast;
/// Fuzz testing entry points.
pub mod fuzz;
/// Compatibility hacks.
pub mod hacks;
pub mod syntax_editor;
/// token kinds (CUSTOM — `is_definition_kind`/`is_statement_kind` enumerate
/// Sail AST variants, not Rust ones).
pub mod ted;
/// Small utilities.
pub mod utils;

/// Generated files are committed; run `cargo test -p syntax -- codegen` to validate.
pub mod codegen;
pub mod cst_lower;
pub mod parse_query;
pub mod parser_lower;
pub mod preprocess;

use std::marker::PhantomData;
// TODO: switch to triomphe if clone overhead becomes measurable on large files.
use std::sync::Arc;

pub use crate::{
    ast::{AstNode, AstToken},
    parsing::{parse_text, parse_text_with_fixities, FixityContext},
    ptr::{AstPtr, SyntaxNodePtr},
    syntax_error::SyntaxError,
    syntax_node::{
        PreorderWithTokens, SailLanguage, SyntaxElement, SyntaxElementChildren, SyntaxNode,
        SyntaxNodeChildren, SyntaxToken, SyntaxTreeBuilder,
    },
    token_text::TokenText,
};
pub use parser::{Span, SyntaxKind, Token, T};
pub use rowan::{
    api::Preorder, Direction, GreenNode, NodeOrToken, SyntaxText, TextRange, TextSize,
    TokenAtOffset, WalkEvent,
};
pub use smol_str::{format_smolstr, SmolStr, SmolStrBuilder, ToSmolStr};

/// Re-export `SourceFile` as the default AST root.
pub use crate::ast::SourceFile;

/// Result of parsing: a syntax tree + accumulated errors.
///
/// Parameterized by the root AST node type (typically `SourceFile`).
///
/// Differences: uses `std::sync::Arc` (RA uses `triomphe::Arc`); no
/// `ParseNodeDropper` RAII guard (low priority — Sail files are small).
#[derive(Debug, PartialEq, Eq)]
pub struct Parse<T> {
    green: Option<GreenNode>,
    errors: Option<Arc<[SyntaxError]>>,
    _ty: PhantomData<fn() -> T>,
}

impl<T> Clone for Parse<T> {
    fn clone(&self) -> Parse<T> {
        Parse { green: self.green.clone(), errors: self.errors.clone(), _ty: PhantomData }
    }
}

impl<T> Parse<T> {
    pub(crate) fn new(green: GreenNode, errors: Vec<SyntaxError>) -> Parse<T> {
        Parse {
            green: Some(green),
            errors: if errors.is_empty() { None } else { Some(errors.into()) },
            _ty: PhantomData,
        }
    }

    pub fn syntax_node(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.as_ref().unwrap().clone())
    }

    /// Parse errors + validation errors.
    pub fn errors(&self) -> Vec<SyntaxError> {
        let mut errors = if let Some(e) = self.errors.as_deref() { e.to_vec() } else { vec![] };
        validation::validate(&self.syntax_node(), &mut errors);
        errors
    }
}

impl<T: ast::AstNode> Parse<T> {
    /// Converts this parse result into a parse result for an untyped syntax tree.
    pub fn to_syntax(mut self) -> Parse<SyntaxNode> {
        let green = self.green.take();
        let errors = self.errors.take();
        Parse { green, errors, _ty: PhantomData }
    }

    /// Gets the parsed syntax tree as a typed ast node.
    pub fn tree(&self) -> T {
        T::cast(self.syntax_node()).unwrap()
    }

    /// Converts from `Parse<T>` to `Result<T, Vec<SyntaxError>>`.
    pub fn ok(self) -> Result<T, Vec<SyntaxError>> {
        let errors = self.errors();
        if !errors.is_empty() {
            Err(errors)
        } else {
            Ok(self.tree())
        }
    }
}

impl Parse<SyntaxNode> {
    /// Cast from `Parse<SyntaxNode>` to `Parse<N>`.
    pub fn cast<N: ast::AstNode>(mut self) -> Option<Parse<N>> {
        if N::cast(self.syntax_node()).is_some() {
            Some(Parse { green: self.green.take(), errors: self.errors.take(), _ty: PhantomData })
        } else {
            None
        }
    }
}

impl Parse<ast::SourceFile> {
    /// Debug dump of the tree + errors.
    pub fn debug_dump(&self) -> String {
        let mut buf = format!("{:#?}", self.tree().syntax());
        for err in self.errors() {
            use std::fmt::Write;
            let _ = write!(buf, "error {:?}: {}\n", err.range(), err);
        }
        buf
    }
}

impl ast::SourceFile {
    /// Parse Sail source text into a `Parse<SourceFile>`.
    pub fn parse(text: &str) -> Parse<ast::SourceFile> {
        let (root, errors) = parsing::parse_text_to_syntax_errors(text);
        let green = root.green().into();
        Parse::new(green, errors)
    }

    /// Reparse after an edit. Tries incremental reparsing first, falls
    /// back to full reparse if not possible.
    pub fn reparse(
        parse: &Parse<ast::SourceFile>,
        delete: TextRange,
        insert: &str,
    ) -> Parse<ast::SourceFile> {
        if let Some(result) = Self::incremental_reparse(parse, delete, insert) {
            return result;
        }
        Self::full_reparse(parse, delete, insert)
    }

    /// Attempt incremental reparsing.
    fn incremental_reparse(
        parse: &Parse<ast::SourceFile>,
        delete: TextRange,
        insert: &str,
    ) -> Option<Parse<ast::SourceFile>> {
        parsing::incremental_reparse(
            &parse.syntax_node(),
            delete,
            insert,
            parse.errors().iter().cloned(),
        )
        .map(|(green_node, errors, _reparsed_range)| Parse {
            green: Some(green_node),
            errors: if errors.is_empty() { None } else { Some(errors.into()) },
            _ty: PhantomData,
        })
    }

    /// Full reparse fallback.
    fn full_reparse(
        parse: &Parse<ast::SourceFile>,
        delete: TextRange,
        insert: &str,
    ) -> Parse<ast::SourceFile> {
        let mut text = parse.syntax_node().text().to_string();
        text.replace_range(std::ops::Range::<usize>::from(delete), insert);
        ast::SourceFile::parse(&text)
    }

    /// Parse with dynamic operator fixities.
    ///
    /// infix/postfix/prefix fixities that are declared at runtime in Sail source.
    pub fn parse_with_fixities(text: &str, fixities: FixityContext) -> Parse<ast::SourceFile> {
        let (root, errors) = parsing::parse_text_with_fixities_to_syntax_errors(text, fixities);
        let green = root.green().into();
        Parse::new(green, errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::AstNode;

    #[test]
    fn parse_source_file() {
        let parse = ast::SourceFile::parse("val x : int\n");
        let tree = parse.tree();
        assert_eq!(tree.syntax().kind(), parser::SyntaxKind::SOURCE_FILE);
        assert!(parse.errors().is_empty());
    }

    #[test]
    fn parse_lossless_round_trip() {
        let input = "function f(x, y) = x + y\n";
        let parse = ast::SourceFile::parse(input);
        assert_eq!(parse.syntax_node().text().to_string(), input);
    }

    #[test]
    fn parse_ok_succeeds_on_valid() {
        let parse = ast::SourceFile::parse("val x : int\n");
        assert!(parse.ok().is_ok());
    }

    #[test]
    fn parse_empty() {
        let parse = ast::SourceFile::parse("");
        let _tree = parse.tree();
        assert!(parse.errors().is_empty());
    }

    /// D1-3: Validate sail.ungram parses successfully.
    #[test]
    fn sail_ungram_parses() {
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar =
            grammar_text.parse().expect("sail.ungram should parse successfully");
        // Verify non-trivial grammar (not empty)
        let node_count = grammar.iter().count();
        assert!(node_count >= 70, "expected at least 70 grammar nodes, got {node_count}");
    }

    /// D1-3: Verify sail.ungram covers all composite SyntaxKind variants.
    #[test]
    fn ungram_covers_composite_kinds() {
        let grammar_text = include_str!("../sail.ungram");
        let grammar: ungrammar::Grammar = grammar_text.parse().unwrap();

        // Collect all node names from the grammar
        let node_names: std::collections::HashSet<String> =
            grammar.iter().map(|node| grammar[node].name.clone()).collect();

        // These are the composite SyntaxKind variants that should have
        // corresponding ungrammar nodes. Sentinels (EOF, TOMBSTONE) and
        // internal nodes (DEFINITION, ERROR) are excluded.
        let expected = [
            "SourceFile",
            "CallableDef",
            "CallableSpec",
            "TypeAliasDef",
            "NamedDef",
            "ScatteredDef",
            "ScatteredClauseDef",
            "DefaultDef",
            "FixityDef",
            "InstantiationDef",
            "DirectiveDef",
            "EndDef",
            "ConstraintDef",
            "TerminationMeasureDef",
            // Expression alternation members
            "LiteralExpr",
            "IdentExpr",
            "TyvarExpr",
            "RefExpr",
            "BinExpr",
            "PrefixExpr",
            "CallExpr",
            "FieldAccessExpr",
            "IndexExpr",
            "SubrangeExpr",
            "VectorUpdateExpr",
            "IfExpr",
            "MatchExpr",
            "TryExpr",
            "BlockExpr",
            "LetExpr",
            "VarExpr",
            "ReturnExpr",
            "ThrowExpr",
            "ExitExpr",
            "AssertExpr",
            "AssignExpr",
            "CastExpr",
            "ForeachExpr",
            "WhileExpr",
            "RepeatExpr",
            "TupleExpr",
            "ListExpr",
            "VectorExpr",
            "StructExpr",
            "UpdateExpr",
            "SizeofExpr",
            "ConstraintExpr",
            "ConfigExpr",
            // Pattern alternation members
            "WildPat",
            "LiteralPat",
            "IdentPat",
            "TyvarPat",
            "TypedPat",
            "TuplePat",
            "ListPat",
            "VectorPat",
            "AppPat",
            "StructPat",
            "BinPat",
            "IndexPat",
            "RangeIndexPat",
            "AsPat",
            // Type alternation members
            "TypeNamed",
            "TypeVar",
            "TypeApp",
            "TypeTuple",
            "TypeArrow",
            "TypeForall",
            "TypeExistential",
            "TypeEffect",
            // Sub-structures
            "MatchArm",
            "BlockItem",
            "FieldInit",
            "ParamList",
            "ArgList",
            "TypeParamList",
            "Quantifier",
            "Attribute",
            "Name",
            "Body",
        ];

        let mut missing = Vec::new();
        for name in &expected {
            if !node_names.contains(*name) {
                missing.push(*name);
            }
        }
        assert!(
            missing.is_empty(),
            "sail.ungram is missing nodes for these SyntaxKind variants: {:?}",
            missing
        );
    }
}
