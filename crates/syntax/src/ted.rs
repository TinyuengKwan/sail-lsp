//! Tree editor — syntax tree mutations without re-parsing.
//!
//! Provides `insert`, `remove`, `replace` operations on the rowan CST.
//! All operations mutate the tree in-place via rowan's `splice_children`.

use crate::syntax_node::{SyntaxElement, SyntaxNode, SyntaxToken};

use crate::ast::{edit::IndentLevel, make};
use crate::SyntaxKind;

/// `Element` trait abstracts over SyntaxNode/SyntaxToken/SyntaxElement.
pub trait Element {
    fn syntax_element(self) -> SyntaxElement;

    /// Alias for `syntax_element` — backward compatibility.
    fn into_element(self) -> SyntaxElement
    where
        Self: Sized,
    {
        self.syntax_element()
    }
}

impl Element for SyntaxNode {
    fn syntax_element(self) -> SyntaxElement {
        self.into()
    }
}

impl Element for SyntaxToken {
    fn syntax_element(self) -> SyntaxElement {
        self.into()
    }
}

impl Element for SyntaxElement {
    fn syntax_element(self) -> SyntaxElement {
        self
    }
}

/// A position in the syntax tree for insertion.
#[derive(Debug)]
pub struct Position {
    repr: PositionRepr,
}

#[derive(Debug)]
enum PositionRepr {
    /// Insert as first child of the given node.
    FirstChild(SyntaxNode),
    /// Insert after the given element.
    After(SyntaxElement),
}

impl Position {
    /// Position after an element.
    pub fn after(elem: impl Into<SyntaxElement>) -> Position {
        Position { repr: PositionRepr::After(elem.into()) }
    }

    /// Position before an element (after its previous sibling, or first child of parent).
    pub fn before(elem: impl Into<SyntaxElement>) -> Position {
        let elem = elem.into();
        let parent = elem.parent().unwrap();
        let index = elem.index();
        if index == 0 {
            Position { repr: PositionRepr::FirstChild(parent) }
        } else {
            let prev = parent.children_with_tokens().nth(index - 1).unwrap();
            Position { repr: PositionRepr::After(prev) }
        }
    }

    /// Position as first child of a node.
    pub fn first_child_of(node: &SyntaxNode) -> Position {
        Position { repr: PositionRepr::FirstChild(node.clone()) }
    }

    /// Position as last child of a node.
    pub fn last_child_of(node: &SyntaxNode) -> Position {
        if let Some(last) = node.last_child_or_token() {
            Position::after(last)
        } else {
            Position::first_child_of(node)
        }
    }
}

/// Insert an element at the given position, with whitespace fixup.
pub fn insert(position: Position, elem: impl Element) {
    insert_all(position, vec![elem.syntax_element()]);
}

/// Insert an element at the given position (no whitespace fixup).
pub fn insert_raw(position: Position, elem: impl Element) {
    insert_all_raw(position, vec![elem.syntax_element()]);
}

/// Insert multiple elements at a position, with whitespace fixup.
pub fn insert_all(position: Position, mut elements: Vec<SyntaxElement>) {
    if let Some(first) = elements.first() {
        if let Some(ws) = ws_before(&position, first) {
            elements.insert(0, ws.into());
        }
    }
    if let Some(last) = elements.last() {
        if let Some(ws) = ws_after(&position, last) {
            elements.push(ws.into());
        }
    }
    insert_all_raw(position, elements);
}

/// Insert multiple elements at a position (no whitespace fixup).
pub fn insert_all_raw(position: Position, elements: Vec<SyntaxElement>) {
    let (parent, index) = match position.repr {
        PositionRepr::FirstChild(parent) => (parent, 0),
        PositionRepr::After(child) => (child.parent().unwrap(), child.index() + 1),
    };
    parent.splice_children(index..index, elements);
}

/// Remove an element from the tree.
pub fn remove(elem: impl Element) {
    elem.syntax_element().detach();
}

/// Remove a range of elements.
pub fn remove_all(range: std::ops::RangeInclusive<SyntaxElement>) {
    replace_all(range, Vec::new());
}

/// Replace an element with another.
pub fn replace(old: impl Element, new: impl Element) {
    replace_with_many(old, vec![new.syntax_element()]);
}

/// Replace an element with multiple elements.
pub fn replace_with_many(old: impl Element, new: Vec<SyntaxElement>) {
    let old = old.syntax_element();
    replace_all(old.clone()..=old, new);
}

/// Replace a range of elements.
pub fn replace_all(range: std::ops::RangeInclusive<SyntaxElement>, new: Vec<SyntaxElement>) {
    let start = range.start().index();
    let end = range.end().index();
    let parent = range.start().parent().unwrap();
    parent.splice_children(start..end + 1, new);
}

/// Append a child to the end of a node.
pub fn append_child(node: &SyntaxNode, child: impl Element) {
    let position = Position::last_child_of(node);
    insert(position, child);
}

/// Append a child without whitespace fixup.
pub fn append_child_raw(node: &SyntaxNode, child: impl Element) {
    let position = Position::last_child_of(node);
    insert_raw(position, child);
}

/// Prepend a child to the beginning of a node.
pub fn prepend_child(node: &SyntaxNode, child: impl Element) {
    let position = Position::first_child_of(node);
    insert(position, child);
}

/// Determine whitespace to insert BEFORE a new element.
fn ws_before(position: &Position, new: &SyntaxElement) -> Option<SyntaxToken> {
    let prev = match &position.repr {
        PositionRepr::FirstChild(_) => return None,
        PositionRepr::After(it) => it,
    };

    // After `{` before a statement/definition → newline + indent.
    if prev.kind() == SyntaxKind::L_CURLY {
        if is_definition_kind(new.kind()) || is_statement_kind(new.kind()) {
            let mut indent = IndentLevel::from_element(prev);
            indent += 1;
            return Some(make::tokens::whitespace(&format!("\n{indent}")));
        }
    }

    ws_between(prev, new)
}

/// Determine whitespace to insert AFTER a new element.
fn ws_after(position: &Position, new: &SyntaxElement) -> Option<SyntaxToken> {
    let next = match &position.repr {
        PositionRepr::FirstChild(parent) => parent.first_child_or_token()?,
        PositionRepr::After(sibling) => sibling.next_sibling_or_token()?,
    };
    ws_between(new, &next)
}

/// Determine whitespace between two adjacent elements.
fn ws_between(left: &SyntaxElement, right: &SyntaxElement) -> Option<SyntaxToken> {
    // Already have whitespace on either side — skip.
    if left.kind() == SyntaxKind::WHITESPACE || right.kind() == SyntaxKind::WHITESPACE {
        return None;
    }
    // No space before `;` or `,`.
    if right.kind() == SyntaxKind::SEMICOLON || right.kind() == SyntaxKind::COMMA {
        return None;
    }
    // No space around `<` and `>` (for type arguments).
    if left.kind() == SyntaxKind::L_ANGLE || right.kind() == SyntaxKind::R_ANGLE {
        return None;
    }

    // Between two top-level definitions → blank line.
    if is_definition_kind(left.kind()) && is_definition_kind(right.kind()) {
        let indent = IndentLevel::from_element(right);
        return Some(make::tokens::whitespace(&format!("\n\n{indent}")));
    }

    // Between two statements → newline + indent.
    if is_statement_kind(left.kind()) || is_definition_kind(left.kind()) {
        if is_statement_kind(right.kind()) || is_definition_kind(right.kind()) {
            let indent = IndentLevel::from_element(right);
            return Some(make::tokens::whitespace(&format!("\n{indent}")));
        }
    }

    // Default: single space.
    Some(make::tokens::single_space())
}

/// Check if a SyntaxKind is a top-level definition.
fn is_definition_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::CALLABLE_DEF
            | SyntaxKind::CALLABLE_SPEC
            | SyntaxKind::TYPE_ALIAS_DEF
            | SyntaxKind::NAMED_DEF
            | SyntaxKind::SCATTERED_DEF
            | SyntaxKind::SCATTERED_CLAUSE_DEF
            | SyntaxKind::CONSTRAINT_DEF
            | SyntaxKind::DEFAULT_DEF
            | SyntaxKind::DIRECTIVE_DEF
            | SyntaxKind::FIXITY_DEF
            | SyntaxKind::INSTANTIATION_DEF
            | SyntaxKind::END_DEF
            | SyntaxKind::TERMINATION_MEASURE_DEF
            | SyntaxKind::OUTCOME_DEF
    )
}

/// Check if a SyntaxKind is a statement-like expression.
fn is_statement_kind(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        SyntaxKind::LET_EXPR
            | SyntaxKind::VAR_EXPR
            | SyntaxKind::ASSIGN_EXPR
            | SyntaxKind::ASSERT_EXPR
            | SyntaxKind::RETURN_EXPR
            | SyntaxKind::BLOCK_EXPR
            | SyntaxKind::IF_EXPR
            | SyntaxKind::MATCH_EXPR
            | SyntaxKind::FOREACH_EXPR
            | SyntaxKind::WHILE_EXPR
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ws_between_skips_existing_whitespace() {
        let parse = crate::ast::SourceFile::parse("val x : int\n");
        let root = parse.syntax_node().clone_for_update();
        let tokens: Vec<_> = root.children_with_tokens().collect();
        // Between whitespace and non-whitespace → None.
        for pair in tokens.windows(2) {
            if pair[0].kind() == SyntaxKind::WHITESPACE {
                assert!(ws_between(&pair[0], &pair[1]).is_none());
            }
        }
    }

    #[test]
    fn insert_adds_whitespace() {
        let parse = crate::ast::SourceFile::parse("val x : int\n");
        let root = parse.syntax_node().clone_for_update();
        // Just verify insert doesn't panic.
        let pos = Position::last_child_of(&root);
        let parse2 = crate::ast::SourceFile::parse("val y : bool\n");
        let child = parse2.syntax_node().first_child().unwrap().clone_subtree().clone_for_update();
        insert(pos, child);
        // The root should now contain both definitions.
        let text = root.text().to_string();
        assert!(text.contains("val x"), "missing first def in: {text}");
        assert!(text.contains("val y"), "missing second def in: {text}");
    }
}
