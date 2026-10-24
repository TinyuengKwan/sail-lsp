//! `generate_constant` assist.
//!
//! Extract a magic number into a named constant.
//! Before: `bits(32)`
//! After:  `let XLEN = 32; bits(XLEN)`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn generate_constant(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = generate_constant_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("generate_constant", AssistKind::RefactorExtract),
        "Extract into constant",
        ctx.range,
        te,
    );
    Some(())
}

/// Extract a numeric literal into a named constant.
///
/// Places a `let CONST = <number>` before the current line and replaces
/// the literal with the constant name.
pub fn generate_constant_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let offset = base_db::range_start(range);

    // Find a numeric literal at cursor using text heuristics.
    if offset >= text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    if !bytes.get(offset).is_some_and(|b| b.is_ascii_digit()) {
        return None;
    }
    // Walk backwards and forwards to find the full number token.
    let num_start = text[..offset]
        .rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
        .map_or(0, |p| p + 1);
    let num_end = text[offset..]
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.')
        .map_or(text.len(), |p| offset + p);
    let lit_text = &text[num_start..num_end];
    if lit_text.is_empty() || !lit_text.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    // Determine a constant name based on the value.
    let const_name = format!("CONST_{lit_text}");

    // Insert the constant definition at the beginning of the line containing the literal.
    let line_start = text[..num_start].rfind('\n').map_or(0, |p| p + 1);
    let indent: String = text[line_start..].chars().take_while(|c| c.is_whitespace()).collect();

    let mut edits = Vec::new();
    edits.push(IdeTextEdit {
        range: base_db::text_range(line_start, line_start),
        new_text: format!("{indent}let {const_name} = {lit_text}\n"),
    });
    edits
        .push(IdeTextEdit { range: base_db::text_range(num_start, num_end), new_text: const_name });
    Some(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(generate_constant, "bits(32)\n", 0);
        let _ = labels; // verify no panic
    }
}
