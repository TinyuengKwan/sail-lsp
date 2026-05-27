//! `remove_unused_imports` assist.

// Removable trait for tree-mutation assists (removing match arms, field inits).
use syntax::ast::edit_in_place::Removable;

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

/// Remove a block item in-place using the `Removable` trait.
/// Used when removing AST nodes structurally (e.g., unused imports, dead arms).
#[allow(dead_code)]
fn remove_block_item(item: &syntax::ast::BlockItem) {
    item.remove();
}

pub(crate) fn remove_unused_imports(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::remove_unused_imports_edits(ctx.file)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("remove_unused_imports", AssistKind::Source),
        "Remove unused imports",
        ctx.range,
        te,
    );
    Some(())
}

// given a function def, generate its type signature (val spec).
//
// Trigger: cursor on a `function` definition that lacks a `val` spec.
// Action: insert `val name : (param_types) -> return_type` above.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(remove_unused_imports, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
