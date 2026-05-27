//! `inline_call` assist.
//! Inlines a function call by replacing it with the function body,
//! substituting formal parameters with actual arguments.
//!
//! ```sail
//! function double(x : int) -> int = x + x
//!
//! let y = double(3)
//! ```
//! →
//! ```sail
//! function double(x : int) -> int = x + x
//!
//! let y = 3 + 3
//! ```

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

/// Delegates to the existing `inline_call_edits()` implementation
/// in `lib.rs`, wrapping it in the handler pattern.
pub(crate) fn inline_call(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = crate::inline_call_edits(ctx.file, ctx.offset())?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_call", AssistKind::RefactorInline),
        "Inline function call",
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
        let labels = check_assist(inline_call, "source code\n", 0);
        let _ = labels; // verify no panic
    }
}
