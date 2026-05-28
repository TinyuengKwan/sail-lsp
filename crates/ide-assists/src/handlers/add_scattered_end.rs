//! `add_scattered_end` assist.
//!
//! Sail-specific: adds missing `end name` for scattered definitions.
//! No RA counterpart (Rust has no scattered definitions).
//!
//! Trigger: cursor on a `scattered` head declaration that has no
//! corresponding `end` marker in the same file.
//! Action: append `end name` after the last clause.
//!
//! Example:
//! ```sail
//! scattered function execute
//! function clause execute(instr) = false
//! // cursor on scattered → generates:
//! end execute
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn add_scattered_end(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.file.text();
    let offset = ctx.offset();
    let item_tree = ctx.file.item_tree()?;

    // Find a ScatteredHead at cursor position
    for &id in item_tree.top_level_items() {
        if id.item_kind(item_tree) != hir_def::item_tree::ItemKind::ScatteredHead {
            continue;
        }
        let id_span = id.span(item_tree);
        if offset < id_span.start || offset > id_span.end {
            continue;
        }

        let name = id.name(item_tree).as_str();

        // Check if `end name` already exists in the file
        let has_end = item_tree.top_level_items().iter().any(|eid| {
            eid.item_kind(item_tree) == hir_def::item_tree::ItemKind::EndMarker
                && eid.name(item_tree).as_str() == name
        });
        if has_end {
            return None; // Already has end marker
        }

        // Find the last clause or the head itself for insert position
        let mut last_span_end = id_span.end;
        for &eid in item_tree.top_level_items() {
            let espan = eid.span(item_tree);
            if eid.name(item_tree).as_str() == name && espan.end > last_span_end {
                last_span_end = espan.end;
            }
        }

        // Find end of line after the last clause
        let insert_pos =
            text[last_span_end..].find('\n').map(|i| last_span_end + i + 1).unwrap_or(text.len());

        let end_text = format!("\nend {name}\n");

        let edit =
            TextEdit { range: base_db::text_range(insert_pos, insert_pos), new_text: end_text };

        acc.add_with_edits(
            AssistId("add_scattered_end", AssistKind::QuickFix),
            format!("Add `end {name}`"),
            ctx.range,
            vec![edit],
        );
        return Some(());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use ide_db::test_utils::TestFile;

    fn check_assist(before: &str, cursor: usize, expected: expect_test::Expect) {
        let file = TestFile::new(before);
        let ctx = AssistContext::new(&file, base_db::text_range(cursor, cursor));
        let mut acc = Assists::new();
        add_scattered_end(&mut acc, &ctx);
        let assists = acc.finish();
        if assists.is_empty() {
            expected.assert_eq("(no assist)");
        } else {
            let labels: Vec<&str> = assists.iter().map(|a| a.label.as_str()).collect();
            expected.assert_eq(&labels.join("\n"));
        }
    }

    #[test]
    fn adds_end_for_scattered_without_end() {
        let src = "scattered function execute\nfunction clause execute(x) = x\n";
        check_assist(src, 5, expect!["Add `end execute`"]);
    }

    #[test]
    fn no_assist_for_non_scattered() {
        check_assist("function foo(x) = x\n", 5, expect!["(no assist)"]);
    }
}
