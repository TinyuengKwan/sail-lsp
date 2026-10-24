//! `inline_const_as_literal` assist.
//!
//! Replace a constant reference with its literal value.
//! Given: `let SIZE = 32`
//! Before: `bits(SIZE)`
//! After:  `bits(32)`

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn inline_const_as_literal(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = inline_const_as_literal_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_const_as_literal", AssistKind::RefactorInline),
        "Inline constant as literal",
        ctx.range,
        te,
    );
    Some(())
}

/// Replace a constant reference with its literal value.
///
/// Scans the file for a top-level `let <NAME> = <literal>` binding, then
/// checks whether the identifier under the cursor matches that name.  If so,
/// replaces the reference with the literal text.
pub fn inline_const_as_literal_edits(
    file: &dyn FileDb,
    range: TextRange,
) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let offset = base_db::range_start(range);

    // Extract the word (identifier) at the cursor using text heuristics.
    if offset >= text.len() {
        return None;
    }
    let bytes = text.as_bytes();
    if !bytes.get(offset).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
        return None;
    }
    let word_start =
        text[..offset].rfind(|c: char| !c.is_ascii_alphanumeric() && c != '_').map_or(0, |p| p + 1);
    let word_end = text[offset..]
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .map_or(text.len(), |p| offset + p);
    let ident = &text[word_start..word_end];
    if ident.is_empty() || ident.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }

    // Scan for `let <ident> = <literal>` at top level (simple heuristic).
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("let ") {
            let rest = rest.trim_start();
            if let Some(after_name) = rest.strip_prefix(ident) {
                // Ensure the match is exact (not a prefix of a longer name).
                if after_name.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_') {
                    continue;
                }
                let after_name = after_name.trim_start();
                // Accept `= <literal>` or `: <type> = <literal>`
                let rhs = if let Some(after_eq) = after_name.strip_prefix('=') {
                    after_eq.trim()
                } else if let Some(after_colon) = after_name.strip_prefix(':') {
                    // Skip type annotation to find `=`
                    if let Some(eq_pos) = after_colon.find('=') {
                        after_colon[eq_pos + 1..].trim()
                    } else {
                        continue;
                    }
                } else {
                    continue;
                };
                // Check that rhs is a numeric literal (possibly negative).
                let literal = rhs.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
                if literal.is_empty() {
                    continue;
                }
                let is_literal = literal.starts_with(|c: char| c.is_ascii_digit())
                    || (literal.starts_with('-')
                        && literal[1..].starts_with(|c: char| c.is_ascii_digit()));
                if !is_literal {
                    continue;
                }
                // Don't replace the definition itself.
                if let Some(def_pos) = text.find(trimmed) {
                    let def_end = def_pos + trimmed.len();
                    if word_start >= def_pos && word_end <= def_end {
                        return None;
                    }
                }
                return Some(vec![IdeTextEdit {
                    range: base_db::text_range(word_start, word_end),
                    new_text: literal.to_string(),
                }]);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(inline_const_as_literal, "let SIZE = 32\nbits(SIZE)\n", 0);
        let _ = labels; // verify no panic
    }
}
