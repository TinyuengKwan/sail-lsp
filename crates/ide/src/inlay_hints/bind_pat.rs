//! Type hints for let/var bindings.

use super::*;

/// Resolve each visible binding through the shared semantic queries.
pub(super) fn collect_type_hints(
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    let groups = body_groups(current_file, begin, end);
    for bodies in groups {
        for entry in bodies.entries() {
            if !span_intersects_range(entry.def_span, begin, end) {
                continue;
            }
            let body = &entry.body;
            for (pat_id, pat) in body.iter_pats() {
                let hir::Pat::Bind(name) = pat else {
                    continue;
                };
                let pat_span = match entry.source_map.pat_syntax(pat_id) {
                    Some(s) => s,
                    None => continue,
                };
                if !span_ends_in_range(pat_span, begin, end) {
                    continue;
                }
                if name.starts_with('_') {
                    continue;
                }

                let ty_text = current_file.file_text().and_then(|(db, ft)| {
                    hir::Semantics::new(db).binding_type_display(ft, pat_span)
                });

                if let Some(ty_text) = ty_text {
                    if ty_text == "unit" || ty_text == "unknown" {
                        continue;
                    }
                    hints.push(InlayHint {
                        label_parts: Vec::new(),
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
}
