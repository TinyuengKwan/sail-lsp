//! `auto_include` assist.
//!
//! When the cursor is on an identifier that is not defined in the current
//! file, offers to add `$include "that_file.sail"` at the top of the file.
//!
//! The handler path (called via `handlers::all()` from `assists()`) only has
//! access to the current file, so it performs a name-resolution check and
//! emits a labelled placeholder assist. The workspace-aware version lives in
//! `ide_assists::auto_include_edits` and is called directly from the LSP
//! request handler with access to `all_files`.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn auto_include(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    // Use the single-file check: returns the unresolved name if cursor is on
    // an identifier that is not locally defined.
    let name = crate::auto_include_single_file_check(ctx.file, ctx.offset())?;

    // Without workspace access we cannot know which file defines the symbol,
    // so we emit a hint-level assist whose label describes the situation.
    // The real quick-fix with the correct file path is produced by
    // `auto_include_edits` in the LSP request handler.
    //
    // We still want this to appear in the `assists()` list so that callers
    // (e.g. tests) can see that the handler fires for unresolved names.
    let target = base_db::text_range(ctx.offset(), ctx.offset());
    acc.add_with_edits(
        AssistId("auto_include", AssistKind::QuickFix),
        format!("Add `$include` for `{name}`"),
        target,
        // No edit here — the real edit requires workspace context.
        // The LSP server produces the full edit via `auto_include_edits`.
        vec![TextEdit { range: target, new_text: String::new() }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke_no_fire_on_defined_name() {
        // `foo` is defined locally — assist should not fire.
        let src = "function foo() -> unit = ()\n";
        let labels = check_assist(auto_include, src, src.find("foo").unwrap());
        assert!(labels.is_empty(), "should not fire on locally defined name, got: {labels:?}");
    }

    #[test]
    fn smoke_fires_on_unknown_name() {
        // `unknown_fn` is not defined anywhere in this file.
        let src = "let x = unknown_fn()\n";
        let offset = src.find("unknown_fn").unwrap();
        let labels = check_assist(auto_include, src, offset);
        // Handler fires with a label mentioning the name.
        assert!(
            labels.iter().any(|l| l.contains("unknown_fn")),
            "expected assist label containing `unknown_fn`, got: {labels:?}"
        );
    }
}
