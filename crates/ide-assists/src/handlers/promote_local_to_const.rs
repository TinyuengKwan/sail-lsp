//! `promote_local_to_const` assist.
//!
//! Promote a local let binding to a top-level definition.
//! Before (inside function): `let width = 32`
//! After (top-level): `let width : int = 32`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn promote_local_to_const(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = promote_local_to_const_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("promote_local_to_const", AssistKind::RefactorExtract),
        "Promote to top-level constant",
        ctx.range,
        te,
    );
    Some(())
}

/// Promote a local let-binding to a top-level definition.
///
/// Finds a `let <name> = <value>` inside a function body and offers to move
/// it to the top of the file as `let <name> : int = <value>`.
pub fn promote_local_to_const_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let offset = base_db::range_start(range);

    // Find the line at the cursor.
    let line_start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    let line_end = text[offset..].find('\n').map_or(text.len(), |p| offset + p);
    let line = text.get(line_start..line_end)?;
    let trimmed = line.trim();

    // Must be a `let` binding with some indentation (i.e. inside a function body).
    if !trimmed.starts_with("let ") {
        return None;
    }
    // Must be indented (local, not already top-level).
    if line.starts_with("let ") {
        return None;
    }

    let rest = trimmed.strip_prefix("let ")?.trim_start();
    // Extract name and value: `name = value` or `name : type = value`
    let eq_pos = rest.find('=')?;
    let before_eq = rest[..eq_pos].trim();
    let after_eq = rest[eq_pos + 1..].trim().trim_end_matches(';').trim();

    if before_eq.is_empty() || after_eq.is_empty() {
        return None;
    }

    // Build top-level definition.
    let has_type = before_eq.contains(':');
    let top_level = if has_type {
        format!("let {before_eq} = {after_eq}\n")
    } else {
        format!("let {before_eq} : int = {after_eq}\n")
    };

    // Insert at start of file, remove original line.
    let mut edits = Vec::new();
    edits.push(IdeTextEdit { range: base_db::text_range(0, 0), new_text: top_level });
    // Remove the original local binding line (including trailing newline).
    let remove_end = if line_end < text.len() { line_end + 1 } else { line_end };
    edits.push(IdeTextEdit {
        range: base_db::text_range(line_start, remove_end),
        new_text: String::new(),
    });
    Some(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels =
            assist_labels(promote_local_to_const, "function f() = { let width = 32 }\n", 0);
        let _ = labels; // verify no panic
    }
}
