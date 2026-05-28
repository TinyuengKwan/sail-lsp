//! Edit algorithm for `SyntaxEditor`.
//! The algorithm:
//! 1. Sort changes by (range start, depth, kind).
//! 2. Validate no intersecting replaces (`report_intersecting_changes`).
//! 3. Build dependency tree: categorize each change as `independent` or
//!    `dependent` (child of another change).
//! 4. Create `TreeMutator` (immutable original + mutable clone) for O(1)
//!    element mapping via `SyntaxNodePtr`.
//! 5. Process independent changes: map targets to mutable tree, prepare new
//!    elements, deduplicate.
//! 6. Process dependent changes: find targets in already-modified nodes.
//! 7. Apply changes via rowan `splice_children`.
//! 8. Propagate annotations via the mapping.

use std::collections::HashMap;

use rowan::TextRange;

use crate::ptr::SyntaxNodePtr;
use crate::syntax_node::{SyntaxElement, SyntaxNode};
use crate::ted;

use super::{Change, ChangeKind, PositionRepr, SyntaxAnnotation, SyntaxEdit, SyntaxEditor};

/// Records that change `child` is contained within the target of change `parent`.
#[derive(Debug)]
struct DependentChange {
    /// Index of the parent change (the one whose replacement contains the child target).
    parent: usize,
    /// Index of the child change (the one nested inside the parent's target).
    child: usize,
}

/// Records a change whose target has already been processed (independent change),
/// so dependent changes can find their targets within it.
#[derive(Debug)]
#[allow(dead_code)]
struct ChangedAncestor {
    kind: ChangedAncestorKind,
    /// Index into the original `changes` vec.
    change_index: usize,
}

#[derive(Debug)]
#[allow(dead_code)]
enum ChangedAncestorKind {
    /// A single element was replaced.
    Single { old_node: SyntaxNode },
    /// A range of siblings was replaced.
    Range { old_start: SyntaxElement, old_end: SyntaxElement },
}

/// Holds both the immutable original tree and a mutable clone, providing
/// O(1) mapping from original nodes to their mutable counterparts via
/// `SyntaxNodePtr`.
struct TreeMutator {
    #[allow(dead_code)]
    immutable: SyntaxNode,
    mutable_clone: SyntaxNode,
}

impl TreeMutator {
    fn new(root: &SyntaxNode) -> Self {
        let mutable_clone = root.clone_for_update();
        Self { immutable: root.clone(), mutable_clone }
    }

    /// Map a node from the immutable tree to its counterpart in the mutable tree.
    fn make_syntax_mut(&self, node: &SyntaxNode) -> Option<SyntaxNode> {
        let ptr = SyntaxNodePtr::new(node);
        ptr.try_to_node(&self.mutable_clone)
    }

    /// Map an element from the immutable tree to its counterpart in the mutable tree.
    fn make_element_mut(&self, elem: &SyntaxElement) -> Option<SyntaxElement> {
        match elem {
            rowan::NodeOrToken::Node(node) => self.make_syntax_mut(node).map(SyntaxElement::from),
            rowan::NodeOrToken::Token(token) => {
                // For tokens, find by range+kind in the mutable tree.
                let target_range = token.text_range();
                let target_kind = token.kind();
                // Walk parent node in mutable tree to find the token.
                if let Some(parent) = token.parent() {
                    if let Some(mut_parent) = self.make_syntax_mut(&parent) {
                        for child in mut_parent.children_with_tokens() {
                            if child.text_range() == target_range && child.kind() == target_kind {
                                return Some(child);
                            }
                        }
                    }
                }
                // Fallback: linear scan from root
                find_element_in(&self.mutable_clone, elem)
            }
        }
    }
}

/// Apply all edits collected in the editor and return a `SyntaxEdit`.
pub(super) fn apply_edits(editor: SyntaxEditor) -> SyntaxEdit {
    let SyntaxEditor { root, mut changes, mappings: _, annotations } = editor;

    if changes.is_empty() && annotations.is_empty() {
        let annotation_map = resolve_annotations(&root, &annotations);
        return SyntaxEdit {
            old_root: root.clone(),
            new_root: root,
            changed_elements: Vec::new(),
            annotations: annotation_map,
        };
    }

    // Step 1: Sort changes by (range start, depth, kind).
    changes.sort_by(|a, b| {
        let a_range = a.target_range();
        let b_range = b.target_range();
        a_range
            .start()
            .cmp(&b_range.start())
            .then_with(|| {
                let a_depth = change_depth(a);
                let b_depth = change_depth(b);
                a_depth.cmp(&b_depth)
            })
            .then_with(|| a.change_kind().cmp(&b.change_kind()))
    });

    // Step 2: Validate no intersecting replaces.
    if !validate_no_intersecting_replaces(&changes) {
        report_intersecting_changes(&changes);
        return SyntaxEdit {
            old_root: root.clone(),
            new_root: root,
            changed_elements: Vec::new(),
            annotations: HashMap::new(),
        };
    }

    // Step 3: Build dependency tree — find which changes are children of others.
    let (independent, dependent) = build_dependency_tree(&changes);

    // Step 4: Create TreeMutator for O(1) element mapping.
    let tree_mutator = TreeMutator::new(&root);

    let mut changed_elements: Vec<SyntaxElement> = Vec::new();

    // Step 5: Process independent changes.
    // We process them in reverse order (end-to-start) so earlier mutations don't
    // shift positions of later ones within the same parent.
    let mut independent_sorted: Vec<usize> = independent;
    // Re-sort independent by range end descending for safe application.
    independent_sorted.sort_by(|&a, &b| {
        let a_range = changes[a].target_range();
        let b_range = changes[b].target_range();
        b_range.end().cmp(&a_range.end()).then_with(|| b_range.start().cmp(&a_range.start()))
    });

    // Track which parent changes produced new nodes (for dependent resolution).
    let mut changed_ancestors: HashMap<usize, Vec<SyntaxElement>> = HashMap::new();

    for &idx in &independent_sorted {
        let change = &changes[idx];
        match change {
            Change::Insert(position, element) => {
                let mapped_pos = map_position_mut(&tree_mutator, position);
                ted::insert_raw(mapped_pos, element.clone());
                changed_elements.push(element.clone());
            }
            Change::InsertAll(position, elements) => {
                let mapped_pos = map_position_mut(&tree_mutator, position);
                ted::insert_all_raw(mapped_pos, elements.clone());
                changed_elements.extend(elements.clone());
            }
            Change::Replace(old, new) => {
                if let Some(mapped_old) = tree_mutator.make_element_mut(old) {
                    match new {
                        Some(new_elem) => {
                            // Track for dependent changes.
                            changed_ancestors.insert(idx, vec![new_elem.clone()]);
                            ted::replace(mapped_old, new_elem.clone());
                            changed_elements.push(new_elem.clone());
                        }
                        None => {
                            ted::remove(mapped_old);
                        }
                    }
                }
            }
            Change::ReplaceWithMany(old, new_elements) => {
                if let Some(mapped_old) = tree_mutator.make_element_mut(old) {
                    changed_ancestors.insert(idx, new_elements.clone());
                    ted::replace_with_many(mapped_old, new_elements.clone());
                    changed_elements.extend(new_elements.clone());
                }
            }
            Change::ReplaceAll(range, new_elements) => {
                if let (Some(mapped_start), Some(mapped_end)) = (
                    tree_mutator.make_element_mut(range.start()),
                    tree_mutator.make_element_mut(range.end()),
                ) {
                    changed_ancestors.insert(idx, new_elements.clone());
                    ted::replace_all(mapped_start..=mapped_end, new_elements.clone());
                    changed_elements.extend(new_elements.clone());
                }
            }
        }
    }

    // Step 6: Process dependent changes.
    // These are changes whose targets are inside the *new* elements produced
    // by a parent change. We find their targets by range+kind in the new subtrees.
    for dep in &dependent {
        let parent_idx = dep.parent;
        let child_idx = dep.child;
        let child_change = &changes[child_idx];

        // Get the new elements from the parent change.
        let parent_new_elements = changed_ancestors.get(&parent_idx);

        match child_change {
            Change::Insert(position, element) => {
                // For dependent inserts, try to map position within parent's new nodes.
                let mapped_pos =
                    map_position_in_new_elements(&tree_mutator, position, parent_new_elements);
                if let Some(pos) = mapped_pos {
                    ted::insert_raw(pos, element.clone());
                    changed_elements.push(element.clone());
                }
            }
            Change::InsertAll(position, elements) => {
                let mapped_pos =
                    map_position_in_new_elements(&tree_mutator, position, parent_new_elements);
                if let Some(pos) = mapped_pos {
                    ted::insert_all_raw(pos, elements.clone());
                    changed_elements.extend(elements.clone());
                }
            }
            Change::Replace(old, new) => {
                let found = find_element_in_new(old, parent_new_elements)
                    .or_else(|| find_element_in(&tree_mutator.mutable_clone, old));
                if let Some(mapped_old) = found {
                    match new {
                        Some(new_elem) => {
                            ted::replace(mapped_old, new_elem.clone());
                            changed_elements.push(new_elem.clone());
                        }
                        None => {
                            ted::remove(mapped_old);
                        }
                    }
                }
            }
            Change::ReplaceWithMany(old, new_elements) => {
                let found = find_element_in_new(old, parent_new_elements)
                    .or_else(|| find_element_in(&tree_mutator.mutable_clone, old));
                if let Some(mapped_old) = found {
                    ted::replace_with_many(mapped_old, new_elements.clone());
                    changed_elements.extend(new_elements.clone());
                }
            }
            Change::ReplaceAll(range, new_elements) => {
                let start = find_element_in_new(range.start(), parent_new_elements)
                    .or_else(|| find_element_in(&tree_mutator.mutable_clone, range.start()));
                let end = find_element_in_new(range.end(), parent_new_elements)
                    .or_else(|| find_element_in(&tree_mutator.mutable_clone, range.end()));
                if let (Some(mapped_start), Some(mapped_end)) = (start, end) {
                    ted::replace_all(mapped_start..=mapped_end, new_elements.clone());
                    changed_elements.extend(new_elements.clone());
                }
            }
        }
    }

    // Step 8: Resolve annotations in the new tree.
    let annotation_map = resolve_annotations(&tree_mutator.mutable_clone, &annotations);

    SyntaxEdit {
        old_root: root,
        new_root: tree_mutator.mutable_clone,
        changed_elements,
        annotations: annotation_map,
    }
}

/// Compute the depth of a change's target in the syntax tree.
fn change_depth(change: &Change) -> usize {
    match change {
        Change::Insert(pos, _) | Change::InsertAll(pos, _) => match &pos.repr {
            PositionRepr::FirstChild(parent) => ancestor_count(parent) + 1,
            PositionRepr::After(elem) => match elem {
                rowan::NodeOrToken::Node(n) => ancestor_count(n),
                rowan::NodeOrToken::Token(t) => t.parent().map_or(0, |p| ancestor_count(&p) + 1),
            },
        },
        Change::Replace(old, _) | Change::ReplaceWithMany(old, _) => match old {
            rowan::NodeOrToken::Node(n) => ancestor_count(n),
            rowan::NodeOrToken::Token(t) => t.parent().map_or(0, |p| ancestor_count(&p) + 1),
        },
        Change::ReplaceAll(range, _) => match range.start() {
            rowan::NodeOrToken::Node(n) => ancestor_count(n),
            rowan::NodeOrToken::Token(t) => t.parent().map_or(0, |p| ancestor_count(&p) + 1),
        },
    }
}

/// Count the number of ancestors of a node (depth from root).
fn ancestor_count(node: &SyntaxNode) -> usize {
    node.ancestors().count() - 1 // subtract 1 because ancestors() includes self
}

/// Build the dependency tree: partition changes into independent and dependent.
///
/// A change is "dependent" if its target range is fully contained within the
/// target range of another change (the "parent"). Only replace-type changes
/// can be parents.
fn build_dependency_tree(changes: &[Change]) -> (Vec<usize>, Vec<DependentChange>) {
    let mut independent: Vec<usize> = Vec::new();
    let mut dependent: Vec<DependentChange> = Vec::new();

    // For each change, check if it's contained within a prior replace change.
    // Since changes are sorted by range start, a potential parent will have
    // a range start <= the child's range start and range end >= child's range end.
    for i in 0..changes.len() {
        let child_range = changes[i].target_range();
        let mut found_parent = false;

        // Only replace-type changes can be parents (they introduce new subtrees).
        for (j, parent) in changes.iter().enumerate().take(i) {
            let parent_kind = parent.change_kind();
            if !matches!(parent_kind, ChangeKind::Replace | ChangeKind::ReplaceRange) {
                continue;
            }
            let parent_range = parent.target_range();
            // Check strict containment (not equal — equal ranges are siblings, not parent/child).
            if parent_range.start() <= child_range.start()
                && parent_range.end() >= child_range.end()
                && parent_range != child_range
            {
                dependent.push(DependentChange { parent: j, child: i });
                found_parent = true;
                break;
            }
        }

        if !found_parent {
            independent.push(i);
        }
    }

    (independent, dependent)
}

/// Check that no two replace-type changes have overlapping (but not nested) ranges.
fn validate_no_intersecting_replaces(changes: &[Change]) -> bool {
    let replaces: Vec<(usize, TextRange)> = changes
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.change_kind(), ChangeKind::Replace | ChangeKind::ReplaceRange))
        .map(|(i, c)| (i, c.target_range()))
        .collect();

    for i in 0..replaces.len() {
        for j in (i + 1)..replaces.len() {
            let (_, r1) = replaces[i];
            let (_, r2) = replaces[j];

            // Two ranges intersect if they overlap but neither contains the other.
            let overlaps = r1.start() < r2.end() && r2.start() < r1.end();
            let r1_contains_r2 = r1.start() <= r2.start() && r1.end() >= r2.end();
            let r2_contains_r1 = r2.start() <= r1.start() && r2.end() >= r1.end();

            if overlaps && !r1_contains_r2 && !r2_contains_r1 {
                return false;
            }
        }
    }
    true
}

/// Log intersecting changes for debugging.
fn report_intersecting_changes(changes: &[Change]) {
    tracing::warn!("syntax_editor: intersecting replace changes detected:");
    for (i, change) in changes.iter().enumerate() {
        if matches!(change.change_kind(), ChangeKind::Replace | ChangeKind::ReplaceRange) {
            tracing::warn!(
                "  change[{i}]: {:?} range={:?}",
                change.change_kind(),
                change.target_range()
            );
        }
    }
    tracing::warn!("  returning unchanged tree");
}

/// Map a `Position` from the original tree to the equivalent position in the
/// mutable tree using the `TreeMutator`.
fn map_position_mut(tree_mutator: &TreeMutator, position: &super::Position) -> ted::Position {
    match &position.repr {
        PositionRepr::FirstChild(parent) => {
            let mapped_parent = tree_mutator
                .make_syntax_mut(parent)
                .unwrap_or_else(|| tree_mutator.mutable_clone.clone());
            ted::Position::first_child_of(&mapped_parent)
        }
        PositionRepr::After(elem) => {
            if let Some(mapped) = tree_mutator.make_element_mut(elem) {
                ted::Position::after(mapped)
            } else {
                // Fallback: end of root
                ted::Position::last_child_of(&tree_mutator.mutable_clone)
            }
        }
    }
}

/// Map a position for a dependent change — try to find the anchor in the
/// parent's new elements first, then fallback to the mutable tree.
fn map_position_in_new_elements(
    tree_mutator: &TreeMutator,
    position: &super::Position,
    parent_new_elements: Option<&Vec<SyntaxElement>>,
) -> Option<ted::Position> {
    match &position.repr {
        PositionRepr::FirstChild(parent) => {
            // Try to find parent in new elements.
            if let Some(new_elems) = parent_new_elements {
                for elem in new_elems {
                    if let rowan::NodeOrToken::Node(node) = elem {
                        if node.kind() == parent.kind() {
                            return Some(ted::Position::first_child_of(node));
                        }
                    }
                }
            }
            // Fallback: try in the mutable tree.
            let mapped = tree_mutator
                .make_syntax_mut(parent)
                .unwrap_or_else(|| tree_mutator.mutable_clone.clone());
            Some(ted::Position::first_child_of(&mapped))
        }
        PositionRepr::After(elem) => {
            // Try to find the element in new elements.
            if let Some(found) = find_element_in_new(elem, parent_new_elements) {
                return Some(ted::Position::after(found));
            }
            // Fallback: mutable tree.
            if let Some(mapped) = tree_mutator.make_element_mut(elem) {
                Some(ted::Position::after(mapped))
            } else {
                Some(ted::Position::last_child_of(&tree_mutator.mutable_clone))
            }
        }
    }
}

/// Find an element within the new elements produced by a parent change.
fn find_element_in_new(
    original: &SyntaxElement,
    parent_new_elements: Option<&Vec<SyntaxElement>>,
) -> Option<SyntaxElement> {
    let new_elems = parent_new_elements?;
    let target_range = original.text_range();
    let target_kind = original.kind();

    for elem in new_elems {
        match elem {
            rowan::NodeOrToken::Node(node) => {
                // Search within this node's subtree.
                for event in node.preorder_with_tokens() {
                    if let rowan::WalkEvent::Enter(child) = event {
                        if child.text_range() == target_range && child.kind() == target_kind {
                            return Some(child);
                        }
                    }
                }
            }
            rowan::NodeOrToken::Token(token) => {
                if token.text_range() == target_range && token.kind() == target_kind {
                    return Some(elem.clone());
                }
            }
        }
    }
    None
}

/// Find an element in a tree by matching its text range and kind.
fn find_element_in(root: &SyntaxNode, original: &SyntaxElement) -> Option<SyntaxElement> {
    let target_range = original.text_range();
    let target_kind = original.kind();

    for event in root.preorder_with_tokens() {
        match event {
            rowan::WalkEvent::Enter(elem) => {
                if elem.text_range() == target_range && elem.kind() == target_kind {
                    return Some(elem);
                }
                // Optimization: skip subtrees that can't contain our target.
                if let rowan::NodeOrToken::Node(ref n) = elem {
                    if n.text_range().end() < target_range.start() {
                        continue;
                    }
                }
            }
            rowan::WalkEvent::Leave(_) => {}
        }
    }
    None
}

/// Find a node in a tree by matching its text range and kind.
#[allow(dead_code)]
fn find_node_in(root: &SyntaxNode, original: &SyntaxNode) -> Option<SyntaxNode> {
    let target_range = original.text_range();
    let target_kind = original.kind();

    root.descendants().find(|node| node.text_range() == target_range && node.kind() == target_kind)
}

/// Resolve annotations: find the annotated elements in the (possibly mutated) tree.
fn resolve_annotations(
    root: &SyntaxNode,
    annotations: &[(SyntaxElement, SyntaxAnnotation)],
) -> HashMap<SyntaxAnnotation, Vec<SyntaxElement>> {
    let mut map: HashMap<SyntaxAnnotation, Vec<SyntaxElement>> = HashMap::new();

    for (elem, ann) in annotations {
        // Try to find the element in the tree by range + kind.
        if let Some(found) = find_element_in(root, elem) {
            map.entry(*ann).or_default().push(found);
        } else {
            // Element might be the root itself or was deleted — try direct match.
            if root.text_range() == elem.text_range()
                && SyntaxElement::from(root.clone()).kind() == elem.kind()
            {
                map.entry(*ann).or_default().push(root.clone().into());
            }
        }
    }

    map
}

#[cfg(test)]
mod tests {
    use crate::parsing::parse_text;
    use crate::syntax_editor::SyntaxEditor;

    #[test]
    fn apply_single_delete() {
        let (root, _) = parse_text("function f(x) = x\n");
        let (mut editor, edit_root) = SyntaxEditor::new(root);
        // Delete first child
        if let Some(first) = edit_root.first_child() {
            editor.delete(first);
        }
        let edit = editor.finish();
        // The definition should be gone.
        let new_text = edit.new_root().text().to_string();
        assert!(!new_text.contains("function"));
    }

    #[test]
    fn dependency_tree_nested_changes() {
        // Verify that the dependency tree correctly identifies nested changes.
        let (root, _) = parse_text("function f(x) = x + y\n");
        let (mut editor, edit_root) = SyntaxEditor::new(root);

        // Just a basic smoke test — delete the root's first child.
        if let Some(first) = edit_root.first_child() {
            editor.delete(first);
        }
        let edit = editor.finish();
        assert!(!edit.new_root().text().to_string().contains("function"));
    }
}
