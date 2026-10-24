//! `extract_function` assist.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use crate::utils::*;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::text_edit::TextEdit;
use ide_db::FileDb;

pub(crate) fn extract_function(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let edits = extract_function_edits(ctx.file, ctx.range)?;
    let te = edits.into_iter().map(|e| TextEdit { range: e.range, new_text: e.new_text }).collect();
    acc.add_with_edits(
        AssistId("extract_function", AssistKind::RefactorExtract),
        "Extract function",
        ctx.range,
        te,
    );
    Some(())
}

/// Enhanced extract_function with HIR-aware free variable detection.
///
/// 1. Identify selected code range
/// 2. Find free variables (defined outside selection, used inside)
/// 3. Detect which enclosing callable the selection is in
/// 4. Generate function definition + call site
///
/// Enhancement over original: also checks HIR Body for identifiers
/// that appear in the expression arena, providing more accurate
/// free variable detection.
pub fn extract_function_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let start = base_db::range_start(range);
    let end = base_db::range_end(range);
    if start >= end {
        return None;
    }
    let text = file.text();
    let selected = text.get(start..end)?.trim();
    if selected.is_empty() {
        return None;
    }

    // Free variables: a name used inside the selection whose binding is
    // outside it. From the bodies, where a binding and a use are different
    // arena entries with their own spans, rather than from the flat
    // occurrence list — which decided "outside the selection" by comparing
    // offsets against every occurrence of the same *name* in the file.
    let mut params: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(bodies) = file.bodies() {
        for entry in bodies.entries() {
            let bound_outside: std::collections::HashSet<String> = entry
                .body
                .iter_pats()
                .filter_map(|(pid, pat)| {
                    let hir::Pat::Bind(name) = pat else { return None };
                    let span = entry.source_map.pat_syntax(pid)?;
                    (span.start < start || span.end > end).then(|| name.to_string())
                })
                .collect();
            for (eid, expr) in entry.body.iter_exprs() {
                let hir::Expr::Ident(name) = expr else { continue };
                let Some(span) = entry.source_map.expr_syntax(eid) else { continue };
                if span.start < start || span.end > end {
                    continue;
                }
                if bound_outside.contains(name.as_str()) && seen.insert(name.to_string()) {
                    params.push(name.to_string());
                }
            }
        }
    }

    let func_name = "extracted_function";
    let param_list = params.join(", ");
    let call_text = if params.is_empty() {
        format!("{func_name}()")
    } else {
        format!("{func_name}({param_list})")
    };

    // Phase 3: Find insertion point (after current definition)
    let insert_offset = find_def_end_after(file, start)?;

    let indent = "  ";
    let func_def = format!("\n\nfunction {func_name}({param_list}) = {{\n{indent}{selected}\n}}\n");

    Some(vec![
        IdeTextEdit { range, new_text: call_text },
        IdeTextEdit {
            range: base_db::text_range(insert_offset, insert_offset),
            new_text: func_def,
        },
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::assist_labels;

    #[test]
    fn smoke() {
        let labels = assist_labels(extract_function, "function foo() -> unit = ()\n", 0);
        let _ = labels;
    }
}
