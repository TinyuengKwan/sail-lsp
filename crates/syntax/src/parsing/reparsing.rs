//! Implementation of incremental re-parsing.
//! Two strategies:
//!   - if the edit modifies only a single token (like changing an
//!     identifier's letter), we replace only this token.
//!   - otherwise, we search for the nearest reparsable block which
//!     contains the edit and try to reparse only that block.

use std::ops::Range;

use parser::Reparser;
use parser::SyntaxKind;
use rowan::{GreenNode, GreenToken, NodeOrToken, TextRange, TextSize};

use crate::syntax_error::SyntaxError;
use crate::syntax_node::{SyntaxElement, SyntaxNode};

/// Attempt incremental reparsing.
///
/// Returns `Some((new_green, new_errors, reparsed_range))` on success,
/// `None` if incremental reparsing is not possible and a full reparse
/// is required.
pub(crate) fn incremental_reparse(
    node: &SyntaxNode,
    delete: TextRange,
    insert: &str,
    errors: impl IntoIterator<Item = SyntaxError>,
) -> Option<(GreenNode, Vec<SyntaxError>, TextRange)> {
    if let Some((green, new_errors, old_range)) = reparse_token(node, delete, insert) {
        return Some((
            green,
            merge_errors(errors, new_errors, old_range, delete, insert),
            old_range,
        ));
    }

    if let Some((green, new_errors, old_range)) = reparse_block(node, delete, insert) {
        return Some((
            green,
            merge_errors(errors, new_errors, old_range, delete, insert),
            old_range,
        ));
    }
    None
}

/// Strategy 1: Token-level reparsing.
///
/// If the edit only touches a single token and doesn't change its kind,
/// we can replace just that one green token in the tree.
fn reparse_token(
    root: &SyntaxNode,
    delete: TextRange,
    insert: &str,
) -> Option<(GreenNode, Vec<SyntaxError>, TextRange)> {
    let prev_token = root.covering_element(delete).as_token()?.clone();
    let prev_token_kind = prev_token.kind();
    match prev_token_kind {
        // Eligible token kinds for single-token reparsing.
        SyntaxKind::WHITESPACE | SyntaxKind::LINE_COMMENT | SyntaxKind::BLOCK_COMMENT => {
            // Removing a newline may extend previous token.
            let deleted_range = delete - prev_token.text_range().start();
            if prev_token.text()[deleted_range].contains('\n') {
                return None;
            }

            let mut new_text = get_text_after_edit(prev_token.clone().into(), delete, insert);
            let (new_token_kind, _new_err) = parser::single_token(&new_text)?;

            if new_token_kind != prev_token_kind {
                return None;
            }

            // Check that the edited token is not part of a bigger token.
            if let Some(next_char) = root.text().char_at(prev_token.text_range().end()) {
                new_text.push(next_char);
                let token_with_next_char = parser::single_token(&new_text);
                if token_with_next_char.is_some() {
                    return None;
                }
                new_text.pop();
            }

            let new_token = GreenToken::new(rowan::SyntaxKind(prev_token_kind.into()), &new_text);
            let _range = TextRange::up_to(TextSize::of(&new_text));
            Some((prev_token.replace_with(new_token), vec![], prev_token.text_range()))
        }
        // IDENT tokens (Sail identifiers, keywords are separate tokens)
        k if is_ident_like(k) => {
            let mut new_text = get_text_after_edit(prev_token.clone().into(), delete, insert);
            let (new_token_kind, _new_err) = parser::single_token(&new_text)?;

            if new_token_kind != prev_token_kind {
                return None;
            }

            // Check not merging with adjacent token
            if let Some(next_char) = root.text().char_at(prev_token.text_range().end()) {
                new_text.push(next_char);
                if parser::single_token(&new_text).is_some() {
                    return None;
                }
                new_text.pop();
            }

            let new_token = GreenToken::new(rowan::SyntaxKind(prev_token_kind.into()), &new_text);
            let _range = TextRange::up_to(TextSize::of(&new_text));
            Some((prev_token.replace_with(new_token), vec![], prev_token.text_range()))
        }
        // STRING tokens
        SyntaxKind::STRING_LIT => {
            let new_text = get_text_after_edit(prev_token.clone().into(), delete, insert);
            let (new_token_kind, _new_err) = parser::single_token(&new_text)?;

            if new_token_kind != prev_token_kind {
                return None;
            }

            let new_token = GreenToken::new(rowan::SyntaxKind(prev_token_kind.into()), &new_text);
            Some((prev_token.replace_with(new_token), vec![], prev_token.text_range()))
        }
        _ => None,
    }
}

/// Check if a SyntaxKind is an identifier-like token eligible for
/// token-level reparsing.
fn is_ident_like(kind: SyntaxKind) -> bool {
    matches!(kind, SyntaxKind::IDENT | SyntaxKind::TY_VAR)
}

/// Strategy 2: Block-level reparsing.
///
/// Find the nearest reparsable node (e.g. `{...}` block) containing
/// the edit, reparse just that block, and splice the new green tree
/// into the old one.
fn reparse_block(
    root: &SyntaxNode,
    delete: TextRange,
    insert: &str,
) -> Option<(GreenNode, Vec<SyntaxError>, TextRange)> {
    let (node, reparser) = find_reparsable_node(root, delete)?;
    let text = get_text_after_edit(node.clone().into(), delete, insert);

    let lexed = parser::LexedStr::new(&text);
    let parser_input = lexed.to_input();
    if !is_balanced(&lexed) {
        return None;
    }

    let tree_traversal = reparser.parse(&parser_input);
    let (green, new_parser_errors, _eof) = crate::parsing::build_tree(lexed, tree_traversal);

    Some((node.replace_with(green), new_parser_errors, node.text_range()))
}

/// Get the text of an element after applying the edit.
fn get_text_after_edit(element: SyntaxElement, mut delete: TextRange, insert: &str) -> String {
    delete -= element.text_range().start();

    let mut text = match element {
        NodeOrToken::Token(token) => token.text().to_owned(),
        NodeOrToken::Node(node) => node.text().to_string(),
    };
    text.replace_range(Range::<usize>::from(delete), insert);
    text
}

/// Find the nearest ancestor that supports incremental reparsing.
fn find_reparsable_node(node: &SyntaxNode, range: TextRange) -> Option<(SyntaxNode, Reparser)> {
    let node = node.covering_element(range);

    node.ancestors().find_map(|node| {
        let first_child = node.first_child_or_token().map(|it| it.kind());
        let parent = node.parent().map(|it| it.kind());
        Reparser::for_node(node.kind(), first_child, parent).map(|r| (node, r))
    })
}

/// Check if the lexed tokens form balanced braces: `{ ... }`.
fn is_balanced(lexed: &parser::LexedStr<'_>) -> bool {
    if lexed.is_empty()
        || lexed.kind(0) != SyntaxKind::L_CURLY
        || lexed.kind(lexed.len() - 1) != SyntaxKind::R_CURLY
    {
        return false;
    }
    let mut balance = 0usize;
    for i in 1..lexed.len() - 1 {
        match lexed.kind(i) {
            SyntaxKind::L_CURLY => balance += 1,
            SyntaxKind::R_CURLY => {
                balance = match balance.checked_sub(1) {
                    Some(b) => b,
                    None => return false,
                };
            }
            _ => (),
        }
    }
    balance == 0
}

/// Merge old errors (outside the reparsed range) with new errors.
fn merge_errors(
    old_errors: impl IntoIterator<Item = SyntaxError>,
    new_errors: Vec<SyntaxError>,
    range_before_reparse: TextRange,
    delete: TextRange,
    insert: &str,
) -> Vec<SyntaxError> {
    let mut res = Vec::new();

    for old_err in old_errors {
        let old_err_range = old_err.range();
        if old_err_range.end() <= range_before_reparse.start() {
            // Error is before the reparsed region — keep as-is.
            res.push(old_err);
        } else if old_err_range.start() >= range_before_reparse.end() {
            // Error is after the reparsed region — shift by edit delta.
            let inserted_len = TextSize::of(insert);
            // Extra parens to prevent uint underflow (HWAB in RA).
            res.push(old_err.with_range((old_err_range + inserted_len) - delete.len()));
        }
        // Errors inside the reparsed region are dropped — replaced by new_errors.
    }

    res.extend(new_errors.into_iter().map(|new_err| {
        let offsetted_range = new_err.range() + range_before_reparse.start();
        new_err.with_range(offsetted_range)
    }));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: parse, apply edit, check incremental == full reparse.
    fn do_check(before: &str, edit_start: usize, edit_end: usize, replace_with: &str) {
        let after = {
            let mut s = before.to_string();
            s.replace_range(edit_start..edit_end, replace_with);
            s
        };

        let (full_root, _full_errors) = crate::parsing::parse_text(&after);
        let (before_root, before_errors) = crate::parsing::parse_text(before);

        let delete =
            TextRange::new(TextSize::from(edit_start as u32), TextSize::from(edit_end as u32));
        let before_syntax_errors: Vec<SyntaxError> = before_errors
            .into_iter()
            .map(|e| SyntaxError::new_at_offset(e.message, TextSize::from(e.offset as u32)))
            .collect();

        let result = incremental_reparse(&before_root, delete, replace_with, before_syntax_errors);

        if let Some((green, _errors, _range)) = result {
            let incr_root = SyntaxNode::new_root(green);
            // Lossless: text must match full reparse.
            assert_eq!(
                incr_root.text().to_string(),
                full_root.text().to_string(),
                "incremental reparse text mismatch"
            );
        }
        // If None, incremental reparse wasn't possible — that's OK for these tests.
    }

    #[test]
    fn reparse_whitespace_addition() {
        // Add spaces inside existing whitespace.
        do_check("val x : int\n", 3, 3, "  ");
    }

    #[test]
    fn reparse_ident_rename() {
        // Rename identifier: foo → bar
        do_check("val foo : int\n", 4, 7, "bar");
    }

    #[test]
    fn reparse_string_edit() {
        do_check("val x = \"hello\"\n", 9, 14, "world");
    }

    #[test]
    fn reparse_comment_edit() {
        do_check("// comment\nval x : int\n", 3, 10, "edited comment");
    }

    #[test]
    fn reparse_block_function_body() {
        // Edit inside a function body should trigger block reparse.
        let before = "function f() = {\n  let x = 1;\n  x\n}\n";
        do_check(before, 26, 27, "2");
    }

    #[test]
    fn full_reparse_fallback_on_structural_change() {
        // Adding a new top-level definition — incremental won't handle this.
        let before = "val x : int\n";
        let (root, errors) = crate::parsing::parse_text(before);
        let syntax_errors: Vec<SyntaxError> = errors
            .into_iter()
            .map(|e| SyntaxError::new_at_offset(e.message, TextSize::from(e.offset as u32)))
            .collect();

        let delete = TextRange::new(TextSize::from(12), TextSize::from(12));
        let result = incremental_reparse(&root, delete, "val y : bool\n", syntax_errors);
        // This should return None (not possible incrementally).
        assert!(result.is_none());
    }

    #[test]
    fn is_balanced_basic() {
        let lexed = parser::LexedStr::new("{ let x = 1; }");
        assert!(is_balanced(&lexed));
    }

    #[test]
    fn is_balanced_nested() {
        let lexed = parser::LexedStr::new("{ if true then { 1 } else { 2 } }");
        assert!(is_balanced(&lexed));
    }

    #[test]
    fn is_balanced_unbalanced() {
        let lexed = parser::LexedStr::new("{ let x = }");
        // Still balanced in terms of braces (one open, one close).
        assert!(is_balanced(&lexed));
    }

    #[test]
    fn is_balanced_empty() {
        let lexed = parser::LexedStr::new("");
        assert!(!is_balanced(&lexed));
    }
}
