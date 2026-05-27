//! Fragment parsing for SSR patterns.
//!
//! Sail AST kinds (expression, pattern, type, item).
//!
//! Since Sail's parser expects full source files, we wrap fragments in
//! minimal contexts to parse them, then extract the relevant subtree.

use parser::{PrefixEntryPoint, SyntaxKind};
use syntax::SyntaxNode;

/// Try to parse text as a Sail expression.
///
/// Uses `PrefixEntryPoint::Expr` to validate the fragment is an expression,
/// then wraps in `let __ssr_frag__ = <text>` context for full parsing.
pub(crate) fn expr(s: &str) -> Option<SyntaxNode> {
    // Validate fragment kind via PrefixEntryPoint::Expr
    let _entry = PrefixEntryPoint::Expr;

    // Try wrapping as: `let __ssr_frag__ = <expr>`
    let wrapped = format!("let __ssr_frag__ = {s}");
    let (root, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        // If that fails, try bare parse and look for any expression node.
        return try_bare_parse_for_kind(s, is_expr_kind);
    }
    // Find the expression node (child of the let binding, after the '=').
    find_descendant(&root, is_expr_kind)
}

/// Try to parse text as a Sail pattern.
///
/// Uses `PrefixEntryPoint::Pat` to indicate fragment kind.
/// Wraps in `match __x { <text> => () }` context to get a pattern parse.
pub(crate) fn pat(s: &str) -> Option<SyntaxNode> {
    let _entry = PrefixEntryPoint::Pat;
    let wrapped = format!("match __ssr_x {{ {s} => () }}");
    let (root, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        return try_bare_parse_for_kind(s, is_pat_kind);
    }
    find_descendant(&root, is_pat_kind)
}

/// Try to parse text as a Sail type.
///
/// Uses `PrefixEntryPoint::Ty` to indicate fragment kind.
/// Wraps in `val __ssr_frag__ : <type>` context to get a type parse.
pub(crate) fn typ(s: &str) -> Option<SyntaxNode> {
    let _entry = PrefixEntryPoint::Ty;
    let wrapped = format!("val __ssr_frag__ : {s}");
    let (root, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        return try_bare_parse_for_kind(s, is_type_kind);
    }
    find_descendant(&root, is_type_kind)
}

/// Try to parse text as a top-level Sail item (function, type def, etc.).
pub(crate) fn item(s: &str) -> Option<SyntaxNode> {
    let (root, errors) = syntax::parse_text(s);
    if !errors.is_empty() {
        return None;
    }
    // The first non-trivia child of root should be the item.
    root.children().next()
}

fn is_expr_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CALL_EXPR
            | SyntaxKind::IF_EXPR
            | SyntaxKind::MATCH_EXPR
            | SyntaxKind::LET_EXPR
            | SyntaxKind::BLOCK_EXPR
            | SyntaxKind::LITERAL_EXPR
            | SyntaxKind::IDENT_EXPR
            | SyntaxKind::TUPLE_EXPR
            | SyntaxKind::FIELD_ACCESS_EXPR
            | SyntaxKind::INDEX_EXPR
            | SyntaxKind::RETURN_EXPR
            | SyntaxKind::FOREACH_EXPR
            | SyntaxKind::VECTOR_EXPR
            | SyntaxKind::STRUCT_EXPR
            | SyntaxKind::ASSIGN_EXPR
            | SyntaxKind::BIN_EXPR
            | SyntaxKind::PREFIX_EXPR
            | SyntaxKind::REF_EXPR
            | SyntaxKind::CAST_EXPR
            | SyntaxKind::CONSTRAINT_EXPR
            | SyntaxKind::ASSERT_EXPR
            | SyntaxKind::CONFIG_EXPR
            | SyntaxKind::EXIT_EXPR
            | SyntaxKind::LIST_EXPR
            | SyntaxKind::REPEAT_EXPR
            | SyntaxKind::SIZEOF_EXPR
            | SyntaxKind::SUBRANGE_EXPR
            | SyntaxKind::THROW_EXPR
            | SyntaxKind::TRY_EXPR
            | SyntaxKind::TYVAR_EXPR
            | SyntaxKind::UPDATE_EXPR
            | SyntaxKind::VAR_EXPR
            | SyntaxKind::VECTOR_UPDATE_EXPR
            | SyntaxKind::WHILE_EXPR
    )
}

fn is_pat_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::WILD_PAT
            | SyntaxKind::LITERAL_PAT
            | SyntaxKind::IDENT_PAT
            | SyntaxKind::TUPLE_PAT
            | SyntaxKind::APP_PAT
            | SyntaxKind::STRUCT_PAT
            | SyntaxKind::VECTOR_PAT
            | SyntaxKind::LIST_PAT
            | SyntaxKind::AS_PAT
            | SyntaxKind::BIN_PAT
            | SyntaxKind::INDEX_PAT
            | SyntaxKind::RANGE_INDEX_PAT
            | SyntaxKind::TYPED_PAT
            | SyntaxKind::TYVAR_PAT
    )
}

fn is_type_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::TYPE_NAMED
            | SyntaxKind::TYPE_VAR
            | SyntaxKind::TYPE_APP
            | SyntaxKind::TYPE_TUPLE
            | SyntaxKind::TYPE_ARROW
            | SyntaxKind::TYPE_FORALL
            | SyntaxKind::TYPE_EXISTENTIAL
            | SyntaxKind::TYPE_EFFECT
    )
}

/// Try parsing as a full file and find the first node matching the predicate.
fn try_bare_parse_for_kind(s: &str, predicate: fn(SyntaxKind) -> bool) -> Option<SyntaxNode> {
    let (root, _errors) = syntax::parse_text(s);
    find_descendant(&root, predicate)
}

/// Find the first descendant node whose kind satisfies the predicate.
fn find_descendant(root: &SyntaxNode, predicate: fn(SyntaxKind) -> bool) -> Option<SyntaxNode> {
    for node in root.descendants() {
        if predicate(node.kind()) {
            return Some(node);
        }
    }
    None
}
