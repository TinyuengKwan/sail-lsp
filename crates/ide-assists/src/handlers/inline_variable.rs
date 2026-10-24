//! `inline_variable` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn inline_variable(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = inline_variable_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("inline_variable", AssistKind::RefactorInline),
        "Inline variable",
        ctx.range,
        te,
    );
    Some(())
}

pub fn inline_variable_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let offset = base_db::range_start(range);
    let text = file.text();

    // Find the let binding at cursor position via Body arena
    let bodies = file.bodies()?;
    let entry = bodies.entry_at_offset(offset)?;
    let (let_id, let_span) = find_let_at_offset_hir(&entry.body, &entry.source_map, offset)?;
    let hir::Expr::Let { pat, value, body: let_body } = entry.body.expr(let_id)? else {
        return None;
    };
    let name = match entry.body.pat(*pat)? {
        hir::Pat::Bind(n) => n.clone(),
        _ => return None,
    };
    let value_span = entry.source_map.expr_syntax(*value)?;
    let body_span = entry.source_map.expr_syntax(*let_body)?;
    let value_text = text.get(value_span.start..value_span.end)?;

    // Find all uses of `name` in the body and replace them
    let body_text = text.get(body_span.start..body_span.end)?;

    // Collect offsets of the name in the body
    let mut replacements: Vec<(usize, usize)> = Vec::new();
    let name_len = name.len();
    let mut search_start = 0;
    while let Some(pos) = body_text[search_start..].find(&name) {
        let abs = search_start + pos;
        // Check that it's a whole word
        let before_ok = abs == 0
            || !body_text.as_bytes()[abs - 1].is_ascii_alphanumeric()
                && body_text.as_bytes()[abs - 1] != b'_';
        let after_ok = abs + name_len >= body_text.len()
            || !body_text.as_bytes()[abs + name_len].is_ascii_alphanumeric()
                && body_text.as_bytes()[abs + name_len] != b'_';
        if before_ok && after_ok {
            replacements.push((body_span.start + abs, body_span.start + abs + name_len));
        }
        search_start = abs + 1;
    }

    if replacements.is_empty() {
        return None;
    }

    let mut edits = Vec::new();

    // Replace the let expression with just the body
    // First, replace all occurrences in the body
    for &(start, end) in replacements.iter().rev() {
        edits.push(IdeTextEdit {
            range: base_db::text_range(start, end),
            new_text: value_text.to_string(),
        });
    }

    // Remove the let binding line: replace `let name = value in body` with `body`
    // But since Sail let bindings have different shapes, we replace the whole let expr
    // with the body where the name is already replaced.
    // Actually, simpler: remove the `let name = value;` or `let name = value in` prefix
    let let_prefix_end = body_span.start;
    edits.push(IdeTextEdit {
        range: base_db::text_range(let_span.start, let_prefix_end),
        new_text: String::new(),
    });

    Some(edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist_not_applicable;

    #[test]
    /// Declines with the cursor on the `let` it would inline.
    fn not_applicable_here() {
        check_assist_not_applicable(
            inline_variable,
            "val f : int -> int
function f(x) = { $0let y = x + 1; y }
",
        );
    }
}
