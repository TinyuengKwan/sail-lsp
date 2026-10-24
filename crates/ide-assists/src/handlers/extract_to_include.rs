//! `extract_to_include` assist (stub).
//!
//! For Sail, "extract module" means extracting a section of code to a new file
//! and adding a `$include` directive. The actual file creation is complex, so
//! this is currently a stub with a TODO.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn extract_to_include(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = extract_to_include_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("extract_to_include", AssistKind::RefactorExtract),
        "Extract to $include file",
        ctx.range,
        te,
    );
    Some(())
}

/// Extract selected code into a separate file with a `$include` directive.
///
/// TODO: Actual file creation requires workspace-level coordination.
/// This is a stub that always returns `None`.
pub fn extract_to_include_edits(_file: &dyn FileDb, _range: TextRange) -> Option<Vec<IdeTextEdit>> {
    // TODO: Implement extraction to $include file.
    // Steps would be:
    // 1. Take the selected range of code
    // 2. Create a new file with that code
    // 3. Replace the selection with `$include "new_file.sail"`
    // This requires SourceChange with file creation support.
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(extract_to_include, "val foo : unit\n", 0);
        let _ = labels; // verify no panic (stub always returns None)
    }
}
