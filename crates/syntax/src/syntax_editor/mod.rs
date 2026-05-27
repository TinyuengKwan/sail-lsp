//! Multi-site syntax tree editor.
//! Provides `SyntaxEditor` for coordinating multiple edits on a syntax
//! tree. Unlike `ted` (which applies edits immediately one at a time),
//! `SyntaxEditor` collects edits and applies them in batch, correctly
//! handling position shifts from earlier edits.
//!
//! # Usage
//!
//! ```ignore
//! let (mut editor, root) = SyntaxEditor::new(root);
//! editor.insert(Position::after(&some_node), new_element);
//! editor.replace(old_node, new_node);
//! editor.delete(unused_node);
//! let edit = editor.finish();
//! let new_root = edit.new_root().clone();
//! ```

use std::{
    collections::HashMap,
    num::NonZeroU32,
    ops::RangeInclusive,
    sync::atomic::{AtomicU32, Ordering},
};

use rowan::TextRange;

use crate::{SyntaxElement, SyntaxNode, SyntaxToken};

mod edit_algo;
/// Predefined high-level edit operations.
pub mod edits;
mod mapping;

pub use mapping::{SyntaxMapping, SyntaxMappingBuilder};

/// A unique annotation tag that can be attached to syntax elements before
/// editing. After `finish()`, the annotation can be used to find the
/// corresponding elements in the new tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(transparent)]
pub struct SyntaxAnnotation(NonZeroU32);

impl SyntaxAnnotation {
    /// Create a new unique annotation.
    pub fn new() -> Self {
        Self::default()
    }
}

impl Default for SyntaxAnnotation {
    fn default() -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(1);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        Self(NonZeroU32::new(id).expect("syntax annotation id overflow"))
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
    pub fn after(elem: impl Element) -> Position {
        Position { repr: PositionRepr::After(elem.syntax_element()) }
    }

    /// Position before an element (after its previous sibling, or first child of parent).
    pub fn before(elem: impl Element) -> Position {
        let elem = elem.syntax_element();
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
    pub fn first_child_of(node: &(impl Into<SyntaxNode> + Clone)) -> Position {
        Position { repr: PositionRepr::FirstChild(node.clone().into()) }
    }

    /// Position as last child of a node.
    pub fn last_child_of(node: &(impl Into<SyntaxNode> + Clone)) -> Position {
        let node = node.clone().into();
        if let Some(last) = node.last_child_or_token() {
            Position { repr: PositionRepr::After(last) }
        } else {
            Position { repr: PositionRepr::FirstChild(node) }
        }
    }

    /// The parent node that this position is within.
    #[allow(dead_code)]
    pub(crate) fn parent(&self) -> SyntaxNode {
        match &self.repr {
            PositionRepr::FirstChild(parent) => parent.clone(),
            PositionRepr::After(elem) => elem.parent().unwrap(),
        }
    }

    /// Returns `(parent, child_index)` where `child_index` is where the new
    /// element(s) should be inserted.
    #[allow(dead_code)]
    pub(crate) fn place(&self) -> (SyntaxNode, usize) {
        match &self.repr {
            PositionRepr::FirstChild(parent) => (parent.clone(), 0),
            PositionRepr::After(elem) => (elem.parent().unwrap(), elem.index() + 1),
        }
    }
}

/// A single queued change in the editor.
#[derive(Debug)]
enum Change {
    /// Insert a single element at a position.
    Insert(Position, SyntaxElement),
    /// Insert multiple elements at a position.
    InsertAll(Position, Vec<SyntaxElement>),
    /// Replace an element with another (None = delete).
    Replace(SyntaxElement, Option<SyntaxElement>),
    /// Replace an element with multiple elements.
    ReplaceWithMany(SyntaxElement, Vec<SyntaxElement>),
    /// Replace a contiguous range of siblings with new elements.
    ReplaceAll(RangeInclusive<SyntaxElement>, Vec<SyntaxElement>),
}

/// Classification of change types for ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum ChangeKind {
    Insert,
    ReplaceRange,
    Replace,
}

impl Change {
    pub(crate) fn change_kind(&self) -> ChangeKind {
        match self {
            Change::Insert(_, _) | Change::InsertAll(_, _) => ChangeKind::Insert,
            Change::Replace(_, _) | Change::ReplaceWithMany(_, _) => ChangeKind::Replace,
            Change::ReplaceAll(_, _) => ChangeKind::ReplaceRange,
        }
    }

    /// The text range affected by this change (for ordering).
    fn target_range(&self) -> TextRange {
        match self {
            Change::Insert(pos, _) | Change::InsertAll(pos, _) => match &pos.repr {
                PositionRepr::FirstChild(parent) => parent.text_range(),
                PositionRepr::After(elem) => elem.text_range(),
            },
            Change::Replace(old, _) | Change::ReplaceWithMany(old, _) => old.text_range(),
            Change::ReplaceAll(range, _) => {
                TextRange::new(range.start().text_range().start(), range.end().text_range().end())
            }
        }
    }

    /// The parent node that this change operates on.
    #[allow(dead_code)]
    fn target_parent(&self) -> SyntaxNode {
        match self {
            Change::Insert(pos, _) | Change::InsertAll(pos, _) => pos.parent(),
            Change::Replace(old, _) | Change::ReplaceWithMany(old, _) => old.parent().unwrap(),
            Change::ReplaceAll(range, _) => range.start().parent().unwrap(),
        }
    }
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Change::Insert(_, element) => {
                write!(f, "\x1b[42m{element}\x1b[0m")
            }
            Change::InsertAll(_, elements) => {
                write!(f, "\x1b[42m")?;
                for e in elements {
                    write!(f, "{e}")?;
                }
                write!(f, "\x1b[0m")
            }
            Change::Replace(old, new) => {
                if let Some(new) = new {
                    write!(f, "\x1b[41m{old}\x1b[42m{new}\x1b[0m")
                } else {
                    write!(f, "\x1b[41m{old}\x1b[0m")
                }
            }
            Change::ReplaceWithMany(old, new) => {
                write!(f, "\x1b[41m{old}\x1b[42m")?;
                for e in new {
                    write!(f, "{e}")?;
                }
                write!(f, "\x1b[0m")
            }
            Change::ReplaceAll(range, new) => {
                write!(f, "\x1b[41m{}..{}\x1b[42m", range.start(), range.end())?;
                for e in new {
                    write!(f, "{e}")?;
                }
                write!(f, "\x1b[0m")
            }
        }
    }
}

/// Trait for types that can be converted to a `SyntaxElement`.
pub trait Element {
    fn syntax_element(self) -> SyntaxElement;
}

impl Element for SyntaxElement {
    fn syntax_element(self) -> SyntaxElement {
        self
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

impl<E: Element + Clone> Element for &E {
    fn syntax_element(self) -> SyntaxElement {
        self.clone().syntax_element()
    }
}

/// A batch syntax tree editor.
/// Collects multiple edit operations and applies them in batch via `finish()`.
#[derive(Debug)]
pub struct SyntaxEditor {
    root: SyntaxNode,
    changes: Vec<Change>,
    mappings: SyntaxMapping,
    annotations: Vec<(SyntaxElement, SyntaxAnnotation)>,
}

impl SyntaxEditor {
    /// Create a new editor for the given root node.
    ///
    /// If the root has a parent or is mutable, it is cloned into a fresh
    /// subtree so that edits are isolated. Returns the editor and the
    /// (possibly cloned) root.
    pub fn new(root: SyntaxNode) -> (Self, SyntaxNode) {
        let root = if root.parent().is_some() { root.clone_subtree() } else { root };
        let editor = Self {
            root: root.clone(),
            changes: Vec::new(),
            mappings: SyntaxMapping::default(),
            annotations: Vec::new(),
        };
        (editor, root)
    }

    /// Typed-node variant of [`SyntaxEditor::new`].
    pub fn with_ast_node<T: crate::ast::AstNode>(root: &T) -> (Self, T) {
        let (editor, new_root) = Self::new(root.syntax().clone());
        (editor, T::cast(new_root).unwrap())
    }

    /// Attach an annotation to an element. After `finish()`, the annotation
    /// can be used to locate the corresponding element in the new tree.
    pub fn add_annotation(&mut self, element: impl Element, annotation: SyntaxAnnotation) {
        self.annotations.push((element.syntax_element(), annotation));
    }

    /// Attach the same annotation to multiple elements.
    pub fn add_annotation_all(
        &mut self,
        elements: Vec<impl Element>,
        annotation: SyntaxAnnotation,
    ) {
        self.annotations.extend(
            elements.into_iter().map(|e| e.syntax_element()).zip(std::iter::repeat(annotation)),
        );
    }

    /// Merge another editor's changes into this one.
    ///
    /// The other editor must operate on the same root (or a subtree of it).
    pub fn merge(&mut self, other: SyntaxEditor) {
        self.changes.extend(other.changes);
        self.annotations.extend(other.annotations);
        self.mappings.merge(other.mappings);
    }

    /// Schedule an insertion at the given position.
    pub fn insert(&mut self, position: Position, element: impl Element) {
        self.changes.push(Change::Insert(position, element.syntax_element()));
    }

    /// Schedule insertion of multiple elements at the given position.
    pub fn insert_all(&mut self, position: Position, elements: Vec<SyntaxElement>) {
        self.changes.push(Change::InsertAll(position, elements));
    }

    /// Insert with automatic whitespace before/after.
    pub fn insert_with_whitespace(
        &mut self,
        position: Position,
        element: impl Element,
        factory: &crate::ast::syntax_factory::SyntaxFactory,
    ) {
        self.insert_all_with_whitespace(position, vec![element.syntax_element()], factory)
    }

    /// Insert multiple elements with automatic whitespace.
    pub fn insert_all_with_whitespace(
        &mut self,
        position: Position,
        mut elements: Vec<SyntaxElement>,
        factory: &crate::ast::syntax_factory::SyntaxFactory,
    ) {
        // Add whitespace before the first element if there's a preceding sibling.
        if let PositionRepr::After(prev) = &position.repr {
            if !elements.is_empty() && !prev.kind().is_trivia() {
                elements.insert(0, factory.whitespace(" ").into());
            }
        }
        self.insert_all(position, elements)
    }

    /// Schedule deletion of an element.
    pub fn delete(&mut self, element: impl Element) {
        self.changes.push(Change::Replace(element.syntax_element(), None));
    }

    /// Schedule deletion of a contiguous range of siblings.
    pub fn delete_all(&mut self, range: RangeInclusive<SyntaxElement>) {
        self.changes.push(Change::ReplaceAll(range, Vec::new()));
    }

    /// Schedule replacement of an element with another.
    pub fn replace(&mut self, old: impl Element, new: impl Element) {
        self.changes.push(Change::Replace(old.syntax_element(), Some(new.syntax_element())));
    }

    /// Schedule replacement of an element with multiple elements.
    pub fn replace_with_many(&mut self, old: impl Element, new: Vec<SyntaxElement>) {
        self.changes.push(Change::ReplaceWithMany(old.syntax_element(), new));
    }

    /// Schedule replacement of a contiguous range of siblings.
    pub fn replace_all(&mut self, range: RangeInclusive<SyntaxElement>, new: Vec<SyntaxElement>) {
        self.changes.push(Change::ReplaceAll(range, new));
    }

    /// Apply all queued edits and return the result.
    pub fn finish(self) -> SyntaxEdit {
        edit_algo::apply_edits(self)
    }

    /// Add mappings from another source.
    pub fn add_mappings(&mut self, other: SyntaxMapping) {
        self.mappings.merge(other);
    }

    /// Number of pending changes.
    pub fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether there are no pending changes.
    pub fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }
}

/// The result of applying edits via `SyntaxEditor::finish()`.
pub struct SyntaxEdit {
    old_root: SyntaxNode,
    new_root: SyntaxNode,
    changed_elements: Vec<SyntaxElement>,
    annotations: HashMap<SyntaxAnnotation, Vec<SyntaxElement>>,
}

impl SyntaxEdit {
    /// The root before edits.
    pub fn old_root(&self) -> &SyntaxNode {
        &self.old_root
    }

    /// The root after edits.
    pub fn new_root(&self) -> &SyntaxNode {
        &self.new_root
    }

    /// Elements that were changed (inserted/replaced).
    pub fn changed_elements(&self) -> &[SyntaxElement] {
        &self.changed_elements
    }

    /// Find elements by annotation in the new tree.
    pub fn find_annotation(&self, annotation: SyntaxAnnotation) -> &[SyntaxElement] {
        self.annotations.get(&annotation).map_or(&[], |v| v.as_slice())
    }
}

impl std::fmt::Debug for SyntaxEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyntaxEdit")
            .field("old_root", &self.old_root)
            .field("new_root", &self.new_root)
            .field("n_changed", &self.changed_elements.len())
            .field("n_annotations", &self.annotations.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parsing::parse_text;

    #[test]
    fn editor_empty_is_noop() {
        let (root, _) = parse_text("function f(x) = x\n");
        let original_text = root.text().to_string();
        let (editor, _) = SyntaxEditor::new(root.clone());
        assert!(editor.is_empty());
        let edit = editor.finish();
        assert_eq!(edit.new_root().text().to_string(), original_text);
    }

    #[test]
    fn editor_annotation_round_trip() {
        let (root, _) = parse_text("function f(x) = x\n");
        let (mut editor, root) = SyntaxEditor::new(root);
        let ann = SyntaxAnnotation::new();
        let first_child = root.first_child().unwrap();
        editor.add_annotation(&first_child, ann);
        let edit = editor.finish();
        let found = edit.find_annotation(ann);
        assert!(!found.is_empty());
    }
}
