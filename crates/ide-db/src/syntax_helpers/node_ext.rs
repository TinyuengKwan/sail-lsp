//! Syntax node extension methods for IDE features.
//! Higher-level helpers that combine AST + semantics for common IDE patterns.

use syntax::{SyntaxKind as SK, SyntaxNode, SyntaxToken};

/// Is this node an expression?
///
/// The list is the grammar's, and it lives here because two crates ask:
/// `ide-ssr` to decide what a fragment is, and `ide-completion` to decide
/// whether the cursor is in an expression at all — which it used to answer
/// by counting braces, so a one-expression function body (`= foo|`, brace
/// depth 0) was read as the top level of the file and completion offered
/// item keywords where a name belonged.
pub fn is_expr_kind(kind: SK) -> bool {
    matches!(
        kind,
        SK::CALL_EXPR
            | SK::IF_EXPR
            | SK::MATCH_EXPR
            | SK::LET_EXPR
            | SK::BLOCK_EXPR
            | SK::LITERAL_EXPR
            | SK::IDENT_EXPR
            | SK::TUPLE_EXPR
            | SK::FIELD_ACCESS_EXPR
            | SK::INDEX_EXPR
            | SK::RETURN_EXPR
            | SK::FOREACH_EXPR
            | SK::VECTOR_EXPR
            | SK::STRUCT_EXPR
            | SK::ASSIGN_EXPR
            | SK::BIN_EXPR
            | SK::PREFIX_EXPR
            | SK::REF_EXPR
            | SK::CAST_EXPR
            | SK::CONSTRAINT_EXPR
            | SK::ASSERT_EXPR
            | SK::CONFIG_EXPR
            | SK::EXIT_EXPR
            | SK::LIST_EXPR
            | SK::REPEAT_EXPR
            | SK::SIZEOF_EXPR
            | SK::SUBRANGE_EXPR
            | SK::THROW_EXPR
            | SK::TRY_EXPR
            | SK::TYVAR_EXPR
            | SK::UPDATE_EXPR
            | SK::VAR_EXPR
            | SK::VECTOR_UPDATE_EXPR
            | SK::WHILE_EXPR
    )
}

/// Check if a token is inside a comment.
pub fn is_in_comment(token: &SyntaxToken) -> bool {
    matches!(token.kind(), SK::LINE_COMMENT | SK::BLOCK_COMMENT | SK::DOC_COMMENT)
}

/// Check if a token is inside a string literal.
pub fn is_in_string(token: &SyntaxToken) -> bool {
    matches!(token.kind(), SK::STRING_LIT | SK::MULTILINE_STRING_LIT)
}

/// Find the first identifier token in a node's descendants.
pub fn first_ident(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.descendants_with_tokens().filter_map(|el| el.into_token()).find(|t| t.kind() == SK::IDENT)
}

// There is no `ident_tokens`. It collected every `IDENT` under a node;
// the callers that want them iterate `descendants_with_tokens` with their
// own filter, because none of them wants *every* identifier — they want
// the first, or the ones in a particular child.

// There is no `def_name_text` either. It forwarded to
// `syntax::definition_name`, which is already the one place that answers
// "what does this definition call itself" — a re-export in a helpers
// module is a second name for it, not a second answer.
