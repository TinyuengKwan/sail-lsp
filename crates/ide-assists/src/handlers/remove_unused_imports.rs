//! `remove_unused_imports` assist.

// Removable trait for tree-mutation assists (removing match arms, field inits).
use syntax::ast::edit_in_place::Removable;

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

/// Remove a block item in-place using the `Removable` trait.
/// Used when removing AST nodes structurally (e.g., unused imports, dead arms).
#[expect(dead_code)]
fn remove_block_item(item: &syntax::ast::BlockItem) {
    item.remove();
}

pub(crate) fn remove_unused_imports(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = remove_unused_imports_edits(ctx.file)?;
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

pub fn remove_unused_imports_edits(file: &dyn FileDb) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();

    // Collect all $include paths
    let mut edits = Vec::new();
    let mut cursor = 0usize;
    for chunk in text.split_inclusive('\n') {
        let line_start = cursor;
        let line_end = cursor + chunk.len();
        let trimmed = chunk.trim();
        cursor = line_end;

        if let Some(path) = trimmed.strip_prefix("$include \"").and_then(|s| {
            s.strip_suffix('"')
                .or_else(|| s.strip_suffix("\"\n"))
                .or_else(|| s.strip_suffix("\"\r\n"))
        }) {
            // Check if this include is actually used
            // Simple heuristic: check if the included file's name appears in any
            // cross-file reference. If no symbols from this file are referenced, flag it.
            // For now, we check against the file name stem
            let stem =
                std::path::Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or(path);

            // Whether this file calls anything at all. The test is as coarse
            // as the one it replaces — neither knows which names came from
            // *this* include — but it counts resolved call sites from the
            // callgraph rather than every name-shaped token in the file.
            let is_used = file.callgraph().is_some_and(|g| g.site_count() > 0);

            // If clearly unused (no external references at all), add removal edit
            // This is conservative - we only remove if we're confident
            if !is_used && !stem.is_empty() {
                edits.push(IdeTextEdit {
                    range: base_db::text_range(line_start, line_end),
                    new_text: String::new(),
                });
            }
        }
    }

    if edits.is_empty() {
        None
    } else {
        Some(edits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            remove_unused_imports,
            "$0$include \"unused.sail\"\nval f : int -> int\nfunction f(x) = x\n",
            expect![[r#"
            val f : int -> int
            function f(x) = x
        "#]],
        );
    }
}
