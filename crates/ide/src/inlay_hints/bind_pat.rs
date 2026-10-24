//! Type hints for let/var bindings.

use super::*;

/// Collect type hints for let/var bindings from InferenceResult.
///
/// `type_check` is an optional externally-provided TypeCheckResult
/// (from salsa `infer_body`). If None, falls back to
/// `FileDb::binding_type_text` (legacy File path).
pub(super) fn collect_type_hints(
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
    type_check: Option<&hir::TypeCheckResult>,
) {
    let Some(bodies) = current_file.bodies() else {
        return;
    };

    for entry in bodies.entries() {
        let body = &entry.body;
        for (pat_id, pat) in body.iter_pats() {
            let hir::Pat::Bind(name) = pat else {
                continue;
            };
            let pat_span = match entry.source_map.pat_syntax(pat_id) {
                Some(s) => s,
                None => continue,
            };
            if pat_span.end < begin || pat_span.start > end {
                continue;
            }
            if name.starts_with('_') {
                continue;
            }

            // Try salsa-provided TypeCheckResult first, then FileDb fallback
            let ty_text = type_check
                .and_then(|tcr| tcr.binding_type_text(pat_span, Some(bodies)))
                .or_else(|| current_file.binding_type_text(pat_span));

            if let Some(ty_text) = ty_text {
                if ty_text == "unit" || ty_text == "unknown" {
                    continue;
                }
                hints.push(InlayHint {
                    offset: pat_span.end,
                    label: format!(": {ty_text}"),
                    kind: InlayHintKind::Type,
                    tooltip: Some(format!("Inferred type of `{name}`")),
                    padding_left: Some(false),
                    padding_right: Some(true),
                    data: Some(serde_json::json!({
                        "kind": "type",
                        "binding": name,
                    })),
                });
            }
        }
    }
}
