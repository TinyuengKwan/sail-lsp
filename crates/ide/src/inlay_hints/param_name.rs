//! Parameter name hints at call sites.

use super::*;

/// Replaces `collect_core_inlay_hints` by walking Expr::Call in each body.
pub(super) fn collect_hir_parameter_hints<'a>(
    files: &[(&'a Url, &'a dyn FileDb)],
    _current_uri: &Url,
    current_file: &dyn FileDb,
    begin: usize,
    end: usize,
    hints: &mut Vec<IdeDbInlayHint>,
) {
    use hir_def::hir::Expr;

    let Some(bodies) = current_file.bodies() else {
        return;
    };
    // Build a lookup table: callee name → param names
    let param_lookup: std::collections::HashMap<String, Vec<String>> = {
        let mut map = std::collections::HashMap::new();
        for (_, f) in files {
            if let Some(parsed) = f.parsed() {
                for head in &parsed.callable_heads {
                    let params: Vec<String> =
                        head.params.iter().filter_map(|p| p.name.clone()).collect();
                    if !params.is_empty() {
                        map.entry(head.name.clone()).or_insert(params);
                    }
                }
            }
        }
        map
    };

    for entry in bodies.entries() {
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
                let Some(param_names) = param_lookup.get(callee_name) else {
                    continue;
                };
                // Generate parameter hints
                for (i, arg_id) in args.iter().enumerate() {
                    if i >= param_names.len() {
                        break;
                    }
                    let param_name = &param_names[i];
                    if let Some(arg_span) = entry.source_map.expr_syntax(*arg_id) {
                        if arg_span.start >= begin && arg_span.end <= end {
                            // Skip if arg text matches param name
                            let arg_text =
                                current_file.text().get(arg_span.start..arg_span.end).unwrap_or("");
                            if arg_text.trim() == param_name {
                                continue;
                            }
                            hints.push(IdeDbInlayHint {
                                offset: arg_span.start,
                                label: format!("{param_name}:"),
                                kind: IdeDbInlayHintKind::Parameter,
                                tooltip: None,
                                padding_left: Some(true),
                                padding_right: Some(true),
                                data: Some(serde_json::json!({
                                    "kind": "parameter",
                                    "param": param_name,
                                })),
                            });
                        }
                    }
                }
            }
        }
    }
}
