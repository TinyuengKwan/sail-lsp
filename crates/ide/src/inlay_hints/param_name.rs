//! Parameter name hints at call sites.

use super::*;

/// Replaces `collect_core_inlay_hints` by walking Expr::Call in each body.
pub(super) fn collect_hir_parameter_hints(
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    use hir::Expr;

    for bodies in body_groups(current_file, begin, end) {
        for entry in bodies.entries() {
            if !span_intersects_range(entry.def_span, begin, end) {
                continue;
            }
            for (id, hir) in entry.body.iter_exprs() {
                let span = entry.source_map.expr_syntax(id).unwrap_or(parser::Span::new(0, 0));
                if span.end < begin || span.start > end {
                    continue;
                }

                if let Expr::Call { callee, args } = hir {
                    // Get callee name
                    let callee_name = match entry.body.expr(*callee) {
                        Some(Expr::Ident(name)) => name.as_str(),
                        _ => continue,
                    };
                    // Skip synthetic modifiers
                    if callee_name.starts_with("_mod_")
                        || callee_name.starts_with("_get_")
                        || callee_name.starts_with("_set_")
                        || callee_name.starts_with("_update_")
                    {
                        continue;
                    }
                    let selected: Option<Vec<Vec<String>>> =
                        current_file.file_text().and_then(|(db, file)| {
                            let sema = hir::Semantics::new(db);
                            if matches!(
                                sema.resolve_path(db, file, span.start, callee_name),
                                Some(hir::PathResolution::Local(_))
                            ) {
                                Some(Vec::new())
                            } else {
                                sema.callable_parameters_at(file, span.start, callee_name).map(
                                    |groups| {
                                        groups
                                            .into_iter()
                                            .map(|parameters| {
                                                parameters.into_iter().map(|p| p.name).collect()
                                            })
                                            .collect()
                                    },
                                )
                            }
                        });
                    let selected_names;
                    let param_names = if let Some(signatures) = selected {
                        let Some(first) = signatures.first() else { continue };
                        selected_names = first.clone();
                        // Clauses and overload alternatives must agree on names.
                        if signatures.iter().skip(1).any(|names| names != &selected_names) {
                            continue;
                        }
                        &selected_names
                    } else {
                        let local = current_file.parsed().and_then(|parsed| {
                            parsed
                                .callable_heads
                                .iter()
                                .filter(|head| {
                                    head.name == callee_name
                                        && head.label_span.start < span.start
                                        && head.params.iter().any(|param| param.name.is_some())
                                })
                                .max_by_key(|head| head.label_span.start)
                        });
                        if let Some(head) = local {
                            selected_names =
                                head.params.iter().filter_map(|param| param.name.clone()).collect();
                            &selected_names
                        } else {
                            continue;
                        }
                    };
                    // Generate parameter hints
                    for (i, arg_id) in args.iter().enumerate() {
                        if i >= param_names.len() {
                            break;
                        }
                        let param_name = &param_names[i];
                        if param_name == "_" {
                            continue;
                        }
                        if let Some(arg_span) = entry.source_map.expr_syntax(*arg_id) {
                            if span_starts_in_range(arg_span, begin, end) {
                                // Skip if arg text matches param name
                                let arg_text = current_file
                                    .text()
                                    .get(arg_span.start..arg_span.end)
                                    .unwrap_or("");
                                if arg_text.trim() == param_name {
                                    continue;
                                }
                                hints.push(InlayHint {
                                    label_parts: Vec::new(),
                                    offset: arg_span.start,
                                    label: format!("{param_name}:"),
                                    kind: InlayHintKind::Parameter,
                                    tooltip: None,
                                    padding_left: Some(true),
                                    padding_right: Some(true),
                                    data: Some(serde_json::json!({
                                        "kind": "parameter",
                                        "param": param_name,
                                        "callee": callee_name,
                                        "index": i,
                                    })),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
}
