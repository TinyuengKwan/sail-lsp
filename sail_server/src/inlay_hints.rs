use crate::hover::infer_expr_type_text;
use crate::state::File;
use crate::symbols::{find_callable_signature, inlay_param_name};
use sail_parser::{
    core_ast::{DefinitionKind, SourceFile},
    BlockItem, Expr, NamedDefKind, Pattern, Span,
};
use tower_lsp::lsp_types::{
    InlayHint, InlayHintKind, InlayHintLabel, InlayHintTooltip, Range, Url,
};

fn span_starts_in_range(span: Span, begin: usize, end: usize) -> bool {
    span.start >= begin && span.start <= end
}

fn expr_symbol(expr: &(Expr, Span)) -> Option<&str> {
    match &expr.0 {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Field { field, .. } => Some(field.0.as_str()),
        _ => None,
    }
}

fn label_text(label: &InlayHintLabel) -> &str {
    match label {
        InlayHintLabel::String(text) => text.as_str(),
        InlayHintLabel::LabelParts(parts) => parts.first().map_or("", |part| part.value.as_str()),
    }
}

fn make_parameter_hint(file: &File, arg: &(Expr, Span), param_name: &str) -> InlayHint {
    InlayHint {
        position: file.source.position_at(arg.1.start),
        label: format!("{param_name}:").into(),
        kind: Some(InlayHintKind::PARAMETER),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: Some(true),
        data: Some(serde_json::json!({
            "kind": "parameter",
            "param": param_name,
        })),
    }
}

fn make_type_hint(file: &File, name_span: Span, ty: String) -> InlayHint {
    InlayHint {
        position: file.source.position_at(name_span.end),
        label: format!(": {ty}").into(),
        kind: Some(InlayHintKind::TYPE),
        text_edits: None,
        tooltip: None,
        padding_left: Some(true),
        padding_right: Some(false),
        data: Some(serde_json::json!({
            "kind": "type",
            "type": ty,
        })),
    }
}

fn maybe_push_parameter_hints_for_call<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    call: &sail_parser::Call,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    let Some(callee) = expr_symbol(&call.callee) else {
        return;
    };
    let Some(sig) = find_callable_signature(files.iter().copied(), current_uri, callee) else {
        return;
    };
    if sig.params.is_empty() {
        return;
    }

    for (arg_idx, arg) in call.args.iter().enumerate() {
        if arg_idx >= sig.params.len() || !span_starts_in_range(arg.1, begin, end) {
            continue;
        }
        let param_name = inlay_param_name(&sig.params[arg_idx].name);
        hints.push(make_parameter_hint(current_file, arg, param_name));
    }
}

fn visit_expr_parameter_hints<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    expr: &(Expr, Span),
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    match &expr.0 {
        Expr::Attribute { expr, .. } => {
            visit_expr_parameter_hints(files, current_uri, current_file, expr, begin, end, hints)
        }
        Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
            visit_expr_parameter_hints(files, current_uri, current_file, lhs, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, rhs, begin, end, hints);
        }
        Expr::Let { binding, body } => {
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                &binding.value,
                begin,
                end,
                hints,
            );
            visit_expr_parameter_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Var {
            target,
            value,
            body,
        } => {
            visit_expr_parameter_hints(files, current_uri, current_file, target, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, value, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Let(binding) => visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        &binding.value,
                        begin,
                        end,
                        hints,
                    ),
                    BlockItem::Var { target, value } => {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            target,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    BlockItem::Expr(expr) => visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        expr,
                        begin,
                        end,
                        hints,
                    ),
                }
            }
        }
        Expr::Return(expr)
        | Expr::Throw(expr)
        | Expr::Exit(Some(expr))
        | Expr::Prefix { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Field { expr, .. } => {
            visit_expr_parameter_hints(files, current_uri, current_file, expr, begin, end, hints)
        }
        Expr::Assert { condition, message } => {
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                condition,
                begin,
                end,
                hints,
            );
            if let Some(message) = message {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    message,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Exit(None) => {}
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr_parameter_hints(files, current_uri, current_file, cond, begin, end, hints);
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                then_branch,
                begin,
                end,
                hints,
            );
            if let Some(else_branch) = else_branch {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    else_branch,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                scrutinee,
                begin,
                end,
                hints,
            );
            for case in cases {
                if let Some(guard) = &case.0.guard {
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        guard,
                        begin,
                        end,
                        hints,
                    );
                }
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    &case.0.body,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Foreach(foreach) => visit_expr_parameter_hints(
            files,
            current_uri,
            current_file,
            &foreach.body,
            begin,
            end,
            hints,
        ),
        Expr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    measure,
                    begin,
                    end,
                    hints,
                );
            }
            visit_expr_parameter_hints(files, current_uri, current_file, body, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, until, begin, end, hints);
        }
        Expr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    measure,
                    begin,
                    end,
                    hints,
                );
            }
            visit_expr_parameter_hints(files, current_uri, current_file, cond, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Call(call) => {
            maybe_push_parameter_hints_for_call(
                files,
                current_uri,
                current_file,
                call,
                begin,
                end,
                hints,
            );
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                &call.callee,
                begin,
                end,
                hints,
            );
            for arg in &call.args {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    arg,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Index { expr, index } => {
            visit_expr_parameter_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, index, begin, end, hints);
        }
        Expr::Slice {
            expr,
            start,
            end: slice_end,
        } => {
            visit_expr_parameter_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, start, begin, end, hints);
            visit_expr_parameter_hints(
                files,
                current_uri,
                current_file,
                slice_end,
                begin,
                end,
                hints,
            );
        }
        Expr::VectorSlice {
            expr,
            start,
            length,
        } => {
            visit_expr_parameter_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, start, begin, end, hints);
            visit_expr_parameter_hints(files, current_uri, current_file, length, begin, end, hints);
        }
        Expr::Struct { fields, .. } => {
            for field in fields {
                if let sail_parser::FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        target,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                }
            }
        }
        Expr::Update { base, fields } => {
            visit_expr_parameter_hints(files, current_uri, current_file, base, begin, end, hints);
            for field in fields {
                if let sail_parser::FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        target,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                }
            }
        }
        Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
            for item in items {
                visit_expr_parameter_hints(
                    files,
                    current_uri,
                    current_file,
                    item,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::VectorUpdate { base, updates } => {
            visit_expr_parameter_hints(files, current_uri, current_file, base, begin, end, hints);
            for update in updates {
                match &update.0 {
                    sail_parser::VectorUpdate::Assignment { target, value } => {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            target,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    sail_parser::VectorUpdate::RangeAssignment {
                        start,
                        end: range_end,
                        value,
                    } => {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            start,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            range_end,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    sail_parser::VectorUpdate::Shorthand(_) => {}
                }
            }
        }
        Expr::Literal(_)
        | Expr::Ident(_)
        | Expr::TypeVar(_)
        | Expr::Ref(_)
        | Expr::Config(_)
        | Expr::SizeOf(_)
        | Expr::Constraint(_)
        | Expr::Error(_) => {}
    }
}

fn pattern_binding_name(pattern: &(Pattern, Span)) -> Option<(&str, Span)> {
    match &pattern.0 {
        Pattern::Attribute { pattern: inner, .. } => pattern_binding_name(inner),
        Pattern::Ident(name) => Some((name.as_str(), pattern.1)),
        Pattern::AsBinding { binding, .. } => Some((binding.0.as_str(), binding.1)),
        Pattern::Typed(_, _) | Pattern::AsType(_, _) => None,
        _ => None,
    }
}

fn target_binding_name(target: &(Expr, Span)) -> Option<(&str, Span)> {
    match &target.0 {
        Expr::Ident(name) => Some((name.as_str(), target.1)),
        Expr::Cast { .. } => None,
        _ => None,
    }
}

fn maybe_push_type_hint<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    name_span: Span,
    value: &(Expr, Span),
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    if !span_starts_in_range(name_span, begin, end) {
        return;
    }
    let Some(ty) = infer_expr_type_text(files, current_uri, current_file, value) else {
        return;
    };
    hints.push(make_type_hint(current_file, name_span, ty));
}

fn visit_expr_type_hints<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    expr: &(Expr, Span),
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    match &expr.0 {
        Expr::Attribute { expr, .. } => {
            visit_expr_type_hints(files, current_uri, current_file, expr, begin, end, hints)
        }
        Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
            visit_expr_type_hints(files, current_uri, current_file, lhs, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, rhs, begin, end, hints);
        }
        Expr::Let { binding, body } => {
            if let Some((_, name_span)) = pattern_binding_name(&binding.pattern) {
                maybe_push_type_hint(
                    files,
                    current_uri,
                    current_file,
                    name_span,
                    &binding.value,
                    begin,
                    end,
                    hints,
                );
            }
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                &binding.value,
                begin,
                end,
                hints,
            );
            visit_expr_type_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Var {
            target,
            value,
            body,
        } => {
            if let Some((_, name_span)) = target_binding_name(target) {
                maybe_push_type_hint(
                    files,
                    current_uri,
                    current_file,
                    name_span,
                    value,
                    begin,
                    end,
                    hints,
                );
            }
            visit_expr_type_hints(files, current_uri, current_file, target, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, value, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Let(binding) => {
                        if let Some((_, name_span)) = pattern_binding_name(&binding.pattern) {
                            maybe_push_type_hint(
                                files,
                                current_uri,
                                current_file,
                                name_span,
                                &binding.value,
                                begin,
                                end,
                                hints,
                            );
                        }
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            &binding.value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    BlockItem::Var { target, value } => {
                        if let Some((_, name_span)) = target_binding_name(target) {
                            maybe_push_type_hint(
                                files,
                                current_uri,
                                current_file,
                                name_span,
                                value,
                                begin,
                                end,
                                hints,
                            );
                        }
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            target,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    BlockItem::Expr(expr) => visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        expr,
                        begin,
                        end,
                        hints,
                    ),
                }
            }
        }
        Expr::Return(expr)
        | Expr::Throw(expr)
        | Expr::Exit(Some(expr))
        | Expr::Prefix { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Field { expr, .. } => {
            visit_expr_type_hints(files, current_uri, current_file, expr, begin, end, hints)
        }
        Expr::Assert { condition, message } => {
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                condition,
                begin,
                end,
                hints,
            );
            if let Some(message) = message {
                visit_expr_type_hints(files, current_uri, current_file, message, begin, end, hints);
            }
        }
        Expr::Exit(None) => {}
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr_type_hints(files, current_uri, current_file, cond, begin, end, hints);
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                then_branch,
                begin,
                end,
                hints,
            );
            if let Some(else_branch) = else_branch {
                visit_expr_type_hints(
                    files,
                    current_uri,
                    current_file,
                    else_branch,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                scrutinee,
                begin,
                end,
                hints,
            );
            for case in cases {
                if let Some(guard) = &case.0.guard {
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        guard,
                        begin,
                        end,
                        hints,
                    );
                }
                visit_expr_type_hints(
                    files,
                    current_uri,
                    current_file,
                    &case.0.body,
                    begin,
                    end,
                    hints,
                );
            }
        }
        Expr::Foreach(foreach) => visit_expr_type_hints(
            files,
            current_uri,
            current_file,
            &foreach.body,
            begin,
            end,
            hints,
        ),
        Expr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                visit_expr_type_hints(files, current_uri, current_file, measure, begin, end, hints);
            }
            visit_expr_type_hints(files, current_uri, current_file, body, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, until, begin, end, hints);
        }
        Expr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                visit_expr_type_hints(files, current_uri, current_file, measure, begin, end, hints);
            }
            visit_expr_type_hints(files, current_uri, current_file, cond, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, body, begin, end, hints);
        }
        Expr::Call(call) => {
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                &call.callee,
                begin,
                end,
                hints,
            );
            for arg in &call.args {
                visit_expr_type_hints(files, current_uri, current_file, arg, begin, end, hints);
            }
        }
        Expr::Index { expr, index } => {
            visit_expr_type_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, index, begin, end, hints);
        }
        Expr::Slice {
            expr,
            start,
            end: slice_end,
        } => {
            visit_expr_type_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, start, begin, end, hints);
            visit_expr_type_hints(
                files,
                current_uri,
                current_file,
                slice_end,
                begin,
                end,
                hints,
            );
        }
        Expr::VectorSlice {
            expr,
            start,
            length,
        } => {
            visit_expr_type_hints(files, current_uri, current_file, expr, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, start, begin, end, hints);
            visit_expr_type_hints(files, current_uri, current_file, length, begin, end, hints);
        }
        Expr::Struct { fields, .. } => {
            for field in fields {
                if let sail_parser::FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        target,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                }
            }
        }
        Expr::Update { base, fields } => {
            visit_expr_type_hints(files, current_uri, current_file, base, begin, end, hints);
            for field in fields {
                if let sail_parser::FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        target,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                }
            }
        }
        Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
            for item in items {
                visit_expr_type_hints(files, current_uri, current_file, item, begin, end, hints);
            }
        }
        Expr::VectorUpdate { base, updates } => {
            visit_expr_type_hints(files, current_uri, current_file, base, begin, end, hints);
            for update in updates {
                match &update.0 {
                    sail_parser::VectorUpdate::Assignment { target, value } => {
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            target,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    sail_parser::VectorUpdate::RangeAssignment {
                        start,
                        end: range_end,
                        value,
                    } => {
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            start,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            range_end,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                    sail_parser::VectorUpdate::Shorthand(_) => {}
                }
            }
        }
        Expr::Literal(_)
        | Expr::Ident(_)
        | Expr::TypeVar(_)
        | Expr::Ref(_)
        | Expr::Config(_)
        | Expr::SizeOf(_)
        | Expr::Constraint(_)
        | Expr::Error(_) => {}
    }
}

fn collect_core_inlay_hints<'a>(
    files: &[(&'a Url, &'a File)],
    current_uri: &Url,
    current_file: &File,
    ast: &SourceFile,
    begin: usize,
    end: usize,
    hints: &mut Vec<InlayHint>,
) {
    for (item, _) in &ast.defs {
        match &item.kind {
            DefinitionKind::Callable(def) => {
                let has_clauses = !def.clauses.is_empty();
                if let Some(rec_measure) = &def.rec_measure {
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        &rec_measure.0.body,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        &rec_measure.0.body,
                        begin,
                        end,
                        hints,
                    );
                }
                for clause in &def.clauses {
                    if let Some(guard) = &clause.0.guard {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            guard,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            guard,
                            begin,
                            end,
                            hints,
                        );
                    }
                    if let Some(body) = &clause.0.body {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            body,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            body,
                            begin,
                            end,
                            hints,
                        );
                    }
                    if let Some(body) = &clause.0.mapping_body {
                        for arm in &body.arms {
                            visit_expr_parameter_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.lhs,
                                begin,
                                end,
                                hints,
                            );
                            visit_expr_parameter_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.rhs,
                                begin,
                                end,
                                hints,
                            );
                            if let Some(guard) = &arm.0.guard {
                                visit_expr_parameter_hints(
                                    files,
                                    current_uri,
                                    current_file,
                                    guard,
                                    begin,
                                    end,
                                    hints,
                                );
                            }
                            visit_expr_type_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.lhs,
                                begin,
                                end,
                                hints,
                            );
                            visit_expr_type_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.rhs,
                                begin,
                                end,
                                hints,
                            );
                            if let Some(guard) = &arm.0.guard {
                                visit_expr_type_hints(
                                    files,
                                    current_uri,
                                    current_file,
                                    guard,
                                    begin,
                                    end,
                                    hints,
                                );
                            }
                        }
                    }
                }
                if !has_clauses {
                    if let Some(body) = &def.body {
                        visit_expr_parameter_hints(
                            files,
                            current_uri,
                            current_file,
                            body,
                            begin,
                            end,
                            hints,
                        );
                        visit_expr_type_hints(
                            files,
                            current_uri,
                            current_file,
                            body,
                            begin,
                            end,
                            hints,
                        );
                    }
                }
                if !has_clauses {
                    if let Some(body) = &def.mapping_body {
                        for arm in &body.arms {
                            visit_expr_parameter_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.lhs,
                                begin,
                                end,
                                hints,
                            );
                            visit_expr_parameter_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.rhs,
                                begin,
                                end,
                                hints,
                            );
                            if let Some(guard) = &arm.0.guard {
                                visit_expr_parameter_hints(
                                    files,
                                    current_uri,
                                    current_file,
                                    guard,
                                    begin,
                                    end,
                                    hints,
                                );
                            }
                            visit_expr_type_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.lhs,
                                begin,
                                end,
                                hints,
                            );
                            visit_expr_type_hints(
                                files,
                                current_uri,
                                current_file,
                                &arm.0.rhs,
                                begin,
                                end,
                                hints,
                            );
                            if let Some(guard) = &arm.0.guard {
                                visit_expr_type_hints(
                                    files,
                                    current_uri,
                                    current_file,
                                    guard,
                                    begin,
                                    end,
                                    hints,
                                );
                            }
                        }
                    }
                }
            }
            DefinitionKind::Named(def) => {
                if matches!(def.kind, NamedDefKind::Let | NamedDefKind::Var) && def.ty.is_none() {
                    if let Some(value) = &def.value {
                        maybe_push_type_hint(
                            files,
                            current_uri,
                            current_file,
                            def.name.1,
                            value,
                            begin,
                            end,
                            hints,
                        );
                    }
                }
                if let Some(value) = &def.value {
                    visit_expr_parameter_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                    visit_expr_type_hints(
                        files,
                        current_uri,
                        current_file,
                        value,
                        begin,
                        end,
                        hints,
                    );
                }
            }
            DefinitionKind::Scattered(_)
            | DefinitionKind::ScatteredClause(_)
            | DefinitionKind::CallableSpec(_)
            | DefinitionKind::TypeAlias(_)
            | DefinitionKind::Default(_)
            | DefinitionKind::Fixity(_)
            | DefinitionKind::Instantiation(_)
            | DefinitionKind::Directive(_)
            | DefinitionKind::End(_)
            | DefinitionKind::Constraint(_)
            | DefinitionKind::TerminationMeasure(_) => {}
        }
    }
}

fn collect_closing_brace_hints(file: &File, range: Range, hints: &mut Vec<InlayHint>) {
    let Some(parsed) = file.parsed() else {
        return;
    };

    for decl in &parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel {
            continue;
        }
        let start_pos = file.source.position_at(decl.span.start);
        let end_pos = file.source.position_at(decl.span.end);
        if end_pos.line - start_pos.line <= 5 {
            continue;
        }
        if end_pos < range.start || end_pos > range.end {
            continue;
        }
        hints.push(InlayHint {
            position: end_pos,
            label: format!(" // {}", decl.name).into(),
            kind: None,
            text_edits: None,
            tooltip: Some(InlayHintTooltip::String(format!("End of {}", decl.name))),
            padding_left: Some(true),
            padding_right: None,
            data: None,
        });
    }
}

pub(crate) fn inlay_hints_for_range<'a, I>(
    files: I,
    current_uri: &Url,
    current_file: &File,
    range: Range,
) -> Vec<InlayHint>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let all_files = files.into_iter().collect::<Vec<_>>();
    let begin = current_file.source.offset_at(&range.start);
    let end = current_file.source.offset_at(&range.end);
    let mut hints = Vec::new();

    if let Some(ast) = current_file.core_ast() {
        collect_core_inlay_hints(
            &all_files,
            current_uri,
            current_file,
            ast,
            begin,
            end,
            &mut hints,
        );
    }
    collect_closing_brace_hints(current_file, range, &mut hints);

    hints.sort_by(|lhs, rhs| {
        lhs.position
            .line
            .cmp(&rhs.position.line)
            .then(lhs.position.character.cmp(&rhs.position.character))
            .then(label_text(&lhs.label).cmp(label_text(&rhs.label)))
    });
    hints
}

pub(crate) fn resolve_inlay_hint(mut hint: InlayHint) -> InlayHint {
    if hint.tooltip.is_none() {
        if let Some(data) = hint.data.as_ref() {
            let kind = data
                .get("kind")
                .and_then(|value| value.as_str())
                .unwrap_or("");
            match kind {
                "parameter" => {
                    if let Some(param) = data.get("param").and_then(|value| value.as_str()) {
                        hint.tooltip = Some(InlayHintTooltip::String(format!(
                            "Parameter hint for `{param}`"
                        )));
                    }
                }
                "type" => {
                    if let Some(ty) = data.get("type").and_then(|value| value.as_str()) {
                        hint.tooltip =
                            Some(InlayHintTooltip::String(format!("Inferred type: `{ty}`")));
                    }
                }
                _ => {}
            }
        }
    }
    hint
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{InlayHintLabel, Position};

    fn full_range(file: &File) -> Range {
        Range::new(
            Position::new(0, 0),
            file.source.position_at(file.source.text().len()),
        )
    }

    fn hint_label(hint: &InlayHint) -> String {
        match &hint.label {
            InlayHintLabel::String(text) => text.clone(),
            InlayHintLabel::LabelParts(parts) => {
                parts.iter().map(|part| part.value.clone()).collect()
            }
        }
    }

    #[test]
    fn emits_parameter_hints_from_ast_calls() {
        let source = r#"
function add(x, y) = x + y
function main() = add(1, 2)
"#;
        let file = File::new(source.to_string());
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let hints = inlay_hints_for_range([(&uri, &file)], &uri, &file, full_range(&file));

        let labels = hints
            .iter()
            .filter(|hint| hint.kind == Some(InlayHintKind::PARAMETER))
            .map(hint_label)
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["x:", "y:"]);
    }

    #[test]
    fn emits_type_hints_from_ast_bindings() {
        let source = r#"
function main() = {
  let local = 1;
  local
}
let top = 0
let typed : int = 2
"#;
        let file = File::new(source.to_string());
        let uri = Url::parse("file:///tmp/test.sail").unwrap();
        let hints = inlay_hints_for_range([(&uri, &file)], &uri, &file, full_range(&file));

        let type_hints = hints
            .iter()
            .filter(|hint| hint.kind == Some(InlayHintKind::TYPE))
            .map(hint_label)
            .collect::<Vec<_>>();
        assert_eq!(type_hints, vec![": int", ": int"]);
    }

    #[test]
    fn resolves_type_hint_tooltip() {
        let hint = InlayHint {
            position: Position::new(0, 0),
            label: ": int".to_string().into(),
            kind: Some(InlayHintKind::TYPE),
            text_edits: None,
            tooltip: None,
            padding_left: None,
            padding_right: None,
            data: Some(serde_json::json!({
                "kind": "type",
                "type": "int",
            })),
        };

        let resolved = resolve_inlay_hint(hint);
        assert!(matches!(
            resolved.tooltip,
            Some(InlayHintTooltip::String(ref text)) if text == "Inferred type: `int`"
        ));
    }
}
