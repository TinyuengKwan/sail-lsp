//! `extract_to_include` assist (stub).
//!
//! For Sail, "extract module" means extracting a section of code to a new file
//! and adding a `$include` directive. The actual file creation is complex, so
//! this is currently a stub with a TODO.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn extract_to_include(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::extract_to_include_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("extract_to_include", AssistKind::RefactorExtract),
        "Extract to $include file",
        ctx.range,
        te,
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(extract_to_include, "val foo : unit\n", 0);
        let _ = labels; // verify no panic (stub always returns None)
    }
}
