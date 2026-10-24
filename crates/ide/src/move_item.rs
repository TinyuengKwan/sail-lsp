//! Move item up/down via CST ancestor walk.
//!
//! definitions, match arms, block items, and list elements.

use ide_db::text_edit::TextEdit as IdeTextEdit;
use ide_db::FileDb;
use parser::SyntaxKind as SK;

/// Direction for moving an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// Sail syntax kinds that can be moved as siblings.
///
/// adapted for Sail's grammar.
const MOVABLE_KINDS: &[SK] = &[
    // Top-level definitions
    SK::CALLABLE_DEF,
    SK::CALLABLE_SPEC,
    SK::TYPE_ALIAS_DEF,
    SK::NAMED_DEF,
    SK::SCATTERED_DEF,
    SK::SCATTERED_CLAUSE_DEF,
    SK::DEFAULT_DEF,
    SK::FIXITY_DEF,
    SK::INSTANTIATION_DEF,
    SK::DIRECTIVE_DEF,
    SK::END_DEF,
    SK::CONSTRAINT_DEF,
    SK::TERMINATION_MEASURE_DEF,
    SK::OUTCOME_DEF,
    // Nested constructs
    SK::MATCH_ARM,
    SK::BLOCK_ITEM,
    SK::FIELD_INIT,
];

/// Move the item at cursor position in the given direction.
///
/// Returns text edits to swap the item with its neighbor.
pub fn move_item(
    file: &dyn FileDb,
    offset: usize,
    direction: Direction,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let (root, _) = syntax::parse_text(text);

    let rowan_offset = rowan::TextSize::from(offset as u32);

    // Find the token at cursor
    let token = root.token_at_offset(rowan_offset).right_biased()?;

    // Walk ancestors to find a movable node
    let movable = token.parent_ancestors().find(|node| MOVABLE_KINDS.contains(&node.kind()))?;

    // Find sibling to swap with
    let swap_target = match direction {
        Direction::Up => movable.prev_sibling()?,
        Direction::Down => movable.next_sibling()?,
    };

    // Only swap with same-kind or movable-kind siblings
    if !MOVABLE_KINDS.contains(&swap_target.kind()) {
        return None;
    }

    // Build swap edits
    let a_range = movable.text_range();
    let b_range = swap_target.text_range();
    let a_text = movable.text().to_string();
    let b_text = swap_target.text().to_string();

    // Determine order: edits must be applied in reverse position order
    // to avoid offset invalidation.
    let (first_range, first_text, second_range, second_text) = if a_range.start() < b_range.start()
    {
        (b_range, a_text, a_range, b_text)
    } else {
        (a_range, b_text, b_range, a_text)
    };

    Some(vec![
        IdeTextEdit {
            range: base_db::text_range(
                u32::from(first_range.start()) as usize,
                u32::from(first_range.end()) as usize,
            ),
            new_text: first_text,
        },
        IdeTextEdit {
            range: base_db::text_range(
                u32::from(second_range.start()) as usize,
                u32::from(second_range.end()) as usize,
            ),
            new_text: second_text,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn move_function_down() {
        let source = "function a() = 1\nfunction b() = 2\n";
        let file = TestFile::new(source);
        // Cursor at start of "a"
        let edits = move_item(&file, 0, Direction::Down);
        assert!(edits.is_some(), "should produce edits");
    }

    #[test]
    fn move_function_up() {
        let source = "function a() = 1\nfunction b() = 2\n";
        let file = TestFile::new(source);
        // Cursor at "b" (offset ~18)
        let edits = move_item(&file, 18, Direction::Up);
        assert!(edits.is_some(), "should produce edits");
    }

    #[test]
    fn cannot_move_first_item_up() {
        let source = "function a() = 1\nfunction b() = 2\n";
        let file = TestFile::new(source);
        let edits = move_item(&file, 0, Direction::Up);
        assert!(edits.is_none(), "first item can't move up");
    }
}

#[cfg(test)]
mod analysis_tests {
    use crate::fixture;
    use crate::MoveDirection;

    /// Through `Analysis`, which is the door the server now has.
    ///
    /// The module was re-exported from `ide` and the server never named it,
    /// so the tests above measured code no editor could reach.
    #[test]
    fn a_definition_swaps_with_its_neighbour() {
        let source = "function a() = 1\nfunction b() = 2\n";
        let (analysis, files) = fixture::analysis(source);
        let url = analysis.url_for_file_id(files[0].0).expect("fixture file has a url");

        let on_a = source.find('a').expect("fixture");
        let edits = analysis.move_item(url, on_a, MoveDirection::Down).unwrap();
        assert!(!edits.is_empty(), "the first of two definitions can move down");

        let mut moved = source.to_owned();
        for edit in edits.iter().rev() {
            moved.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        assert!(
            moved.find("function b").expect("b survives")
                < moved.find("function a").expect("a survives"),
            "the two did not swap: {moved:?}"
        );

        // Nothing above the first one to swap with.
        assert!(analysis.move_item(url, on_a, MoveDirection::Up).unwrap().is_empty());
    }
}
