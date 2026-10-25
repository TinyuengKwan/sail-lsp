//! `organize_imports` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn organize_imports(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = organize_imports_edits(ctx.file)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("organize_imports", AssistKind::Source),
        "Normalize include formatting",
        ctx.range,
        te,
    );
    Some(())
}

pub fn organize_imports_edits(file: &dyn FileDb) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    if text.is_empty() {
        return None;
    }

    let mut lines: Vec<(usize, usize, &str)> = Vec::new();
    let mut cursor = 0usize;
    for chunk in text.split_inclusive('\n') {
        let end = cursor + chunk.len();
        lines.push((cursor, end, chunk));
        cursor = end;
    }
    if cursor < text.len() {
        lines.push((cursor, text.len(), &text[cursor..]));
    }
    if lines.is_empty() {
        return None;
    }

    let mut first_code = 0usize;
    while first_code < lines.len() {
        let trimmed = lines[first_code].2.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            first_code += 1;
            continue;
        }
        break;
    }
    if first_code >= lines.len() {
        return None;
    }

    let first_trimmed = lines[first_code].2.trim();
    if !is_import_directive(first_trimmed) || first_trimmed.contains("//") {
        return None;
    }

    let mut block_end = first_code;
    let mut directives: Vec<String> = Vec::new();
    while block_end < lines.len() {
        let trimmed = lines[block_end].2.trim();
        if trimmed.is_empty() {
            block_end += 1;
            continue;
        }
        if trimmed.starts_with("//") {
            return None;
        }
        if !is_import_directive(trimmed) || trimmed.contains("//") {
            break;
        }
        directives.push(trimmed.to_string());
        block_end += 1;
    }

    if directives.len() < 2 {
        return None;
    }

    // Sail includes execute in source order and can change preprocessor state.
    // Preserve both order and repetition; only normalize whitespace.

    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let old_range_start = lines[first_code].0;
    let old_range_end = lines[block_end - 1].1;
    let mut replacement = directives.join(eol);
    if text.get(old_range_end.saturating_sub(1)..old_range_end) == Some("\n") {
        replacement.push_str(eol);
    }

    let old_block = text.get(old_range_start..old_range_end)?;
    if old_block == replacement {
        return None;
    }

    let edit = IdeTextEdit {
        range: base_db::text_range(old_range_start, old_range_end),
        new_text: replacement,
    };
    Some(vec![edit])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;
    use expect_test::expect;

    #[test]
    fn applies() {
        check_assist(
            organize_imports,
            "  $include \"b.sail\"  \n$include \"a.sail\"\n$0val f : int -> int\n",
            expect![[r#"
            $include "b.sail"
            $include "a.sail"
            val f : int -> int
        "#]],
        );
    }
}
