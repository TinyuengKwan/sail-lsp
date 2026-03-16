use crate::{
    core_ast::{
        BlockItem, Call, DefinitionKind as CoreDefinitionKind, Expr, FieldExpr, FieldPattern,
        NamedDefKind, Pattern, ScatteredClauseKind, SourceFile as CoreSourceFile, Spanned,
        VectorUpdate,
    },
    Span,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallAtOffset<'a> {
    pub callee: &'a str,
    pub callee_span: Span,
    pub open_span: Span,
    pub close_span: Span,
    pub arg_index: usize,
    pub args: &'a [Spanned<Expr>],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BindingValueAtSpan<'a> {
    pub explicit_ty: Option<Span>,
    pub value: &'a Spanned<Expr>,
}

fn span_contains(span: Span, offset: usize) -> bool {
    span.start <= offset && offset <= span.end
}

fn expr_symbol(expr: &Spanned<Expr>) -> Option<(&str, Span)> {
    match &expr.0 {
        Expr::Ident(name) => Some((name.as_str(), expr.1)),
        Expr::Field { field, .. } => Some((field.0.as_str(), field.1)),
        _ => None,
    }
}

fn maybe_update_call<'a>(
    best: &mut Option<(usize, CallAtOffset<'a>)>,
    expr_span: Span,
    call: &'a Call,
    offset: usize,
) {
    if !span_contains(expr_span, offset) {
        return;
    }
    let Some((callee, callee_span)) = expr_symbol(&call.callee) else {
        return;
    };
    let width = expr_span.end.saturating_sub(expr_span.start);
    let arg_index = call
        .arg_separator_spans
        .iter()
        .filter(|span| span.start < offset)
        .count();
    let candidate = CallAtOffset {
        callee,
        callee_span,
        open_span: call.open_span,
        close_span: call.close_span,
        arg_index,
        args: &call.args,
    };

    match best {
        Some((best_width, _)) if *best_width <= width => {}
        _ => *best = Some((width, candidate)),
    }
}

fn visit_expr<'a>(
    expr: &'a Spanned<Expr>,
    offset: usize,
    best: &mut Option<(usize, CallAtOffset<'a>)>,
) {
    if !span_contains(expr.1, offset) {
        return;
    }

    match &expr.0 {
        Expr::Attribute { expr, .. } => visit_expr(expr, offset, best),
        Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
            visit_expr(lhs, offset, best);
            visit_expr(rhs, offset, best);
        }
        Expr::Let { binding, body } => {
            visit_expr(&binding.value, offset, best);
            visit_expr(body, offset, best);
        }
        Expr::Var {
            target,
            value,
            body,
        } => {
            visit_expr(target, offset, best);
            visit_expr(value, offset, best);
            visit_expr(body, offset, best);
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Let(binding) => visit_expr(&binding.value, offset, best),
                    BlockItem::Var { target, value } => {
                        visit_expr(target, offset, best);
                        visit_expr(value, offset, best);
                    }
                    BlockItem::Expr(expr) => visit_expr(expr, offset, best),
                }
            }
        }
        Expr::Return(expr)
        | Expr::Throw(expr)
        | Expr::Exit(Some(expr))
        | Expr::Prefix { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Field { expr, .. } => visit_expr(expr, offset, best),
        Expr::Assert { condition, message } => {
            visit_expr(condition, offset, best);
            if let Some(message) = message {
                visit_expr(message, offset, best);
            }
        }
        Expr::Exit(None) => {}
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            visit_expr(cond, offset, best);
            visit_expr(then_branch, offset, best);
            if let Some(else_branch) = else_branch {
                visit_expr(else_branch, offset, best);
            }
        }
        Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
            visit_expr(scrutinee, offset, best);
            for case in cases {
                if let Some(guard) = &case.0.guard {
                    visit_expr(guard, offset, best);
                }
                visit_expr(&case.0.body, offset, best);
            }
        }
        Expr::Foreach(foreach) => visit_expr(&foreach.body, offset, best),
        Expr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                visit_expr(measure, offset, best);
            }
            visit_expr(body, offset, best);
            visit_expr(until, offset, best);
        }
        Expr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                visit_expr(measure, offset, best);
            }
            visit_expr(cond, offset, best);
            visit_expr(body, offset, best);
        }
        Expr::Call(call) => {
            maybe_update_call(best, expr.1, call, offset);
            visit_expr(&call.callee, offset, best);
            for arg in &call.args {
                visit_expr(arg, offset, best);
            }
        }
        Expr::Index { expr, index } => {
            visit_expr(expr, offset, best);
            visit_expr(index, offset, best);
        }
        Expr::Slice { expr, start, end } => {
            visit_expr(expr, offset, best);
            visit_expr(start, offset, best);
            visit_expr(end, offset, best);
        }
        Expr::VectorSlice {
            expr,
            start,
            length,
        } => {
            visit_expr(expr, offset, best);
            visit_expr(start, offset, best);
            visit_expr(length, offset, best);
        }
        Expr::Struct { fields, .. } => {
            for field in fields {
                if let FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr(target, offset, best);
                    visit_expr(value, offset, best);
                }
            }
        }
        Expr::Update { base, fields } => {
            visit_expr(base, offset, best);
            for field in fields {
                if let FieldExpr::Assignment { target, value } = &field.0 {
                    visit_expr(target, offset, best);
                    visit_expr(value, offset, best);
                }
            }
        }
        Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
            for item in items {
                visit_expr(item, offset, best);
            }
        }
        Expr::VectorUpdate { base, updates } => {
            visit_expr(base, offset, best);
            for update in updates {
                match &update.0 {
                    VectorUpdate::Assignment { target, value } => {
                        visit_expr(target, offset, best);
                        visit_expr(value, offset, best);
                    }
                    VectorUpdate::RangeAssignment { start, end, value } => {
                        visit_expr(start, offset, best);
                        visit_expr(end, offset, best);
                        visit_expr(value, offset, best);
                    }
                    VectorUpdate::Shorthand(_) => {}
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

pub fn find_call_at_offset_core(ast: &CoreSourceFile, offset: usize) -> Option<CallAtOffset<'_>> {
    let mut best = None;

    for (item, _) in &ast.defs {
        match &item.kind {
            CoreDefinitionKind::Callable(def) => {
                if let Some(rec_measure) = &def.rec_measure {
                    visit_expr(&rec_measure.0.body, offset, &mut best);
                }
                for clause in &def.clauses {
                    if let Some(guard) = &clause.0.guard {
                        visit_expr(guard, offset, &mut best);
                    }
                    if let Some(body) = &clause.0.body {
                        visit_expr(body, offset, &mut best);
                    }
                    if let Some(body) = &clause.0.mapping_body {
                        for arm in &body.arms {
                            visit_expr(&arm.0.lhs, offset, &mut best);
                            visit_expr(&arm.0.rhs, offset, &mut best);
                            if let Some(guard) = &arm.0.guard {
                                visit_expr(guard, offset, &mut best);
                            }
                        }
                    }
                }
                if let Some(body) = &def.body {
                    visit_expr(body, offset, &mut best);
                }
                if let Some(body) = &def.mapping_body {
                    for arm in &body.arms {
                        visit_expr(&arm.0.lhs, offset, &mut best);
                        visit_expr(&arm.0.rhs, offset, &mut best);
                        if let Some(guard) = &arm.0.guard {
                            visit_expr(guard, offset, &mut best);
                        }
                    }
                }
            }
            CoreDefinitionKind::Named(def) => {
                if let Some(value) = &def.value {
                    visit_expr(value, offset, &mut best);
                }
            }
            CoreDefinitionKind::Scattered(_)
            | CoreDefinitionKind::ScatteredClause(_)
            | CoreDefinitionKind::CallableSpec(_)
            | CoreDefinitionKind::TypeAlias(_)
            | CoreDefinitionKind::Default(_)
            | CoreDefinitionKind::Fixity(_)
            | CoreDefinitionKind::Instantiation(_)
            | CoreDefinitionKind::Directive(_)
            | CoreDefinitionKind::End(_)
            | CoreDefinitionKind::Constraint(_)
            | CoreDefinitionKind::TerminationMeasure(_) => {}
        }
    }

    best.map(|(_, call)| call)
}

fn pattern_binding_explicit_ty(
    pattern: &Spanned<Pattern>,
    name_span: Span,
) -> Option<Option<Span>> {
    match &pattern.0 {
        Pattern::Attribute { pattern: inner, .. } => pattern_binding_explicit_ty(inner, name_span),
        Pattern::Ident(_) if pattern.1 == name_span => Some(None),
        Pattern::Ident(_) => None,
        Pattern::Typed(inner, ty) | Pattern::AsType(inner, ty) => {
            pattern_binding_explicit_ty(inner, name_span)
                .map(|explicit_ty| explicit_ty.or(Some(ty.1)))
        }
        Pattern::AsBinding { pattern, binding } => {
            if binding.1 == name_span {
                Some(None)
            } else {
                pattern_binding_explicit_ty(pattern, name_span)
            }
        }
        Pattern::Tuple(items) | Pattern::List(items) | Pattern::Vector(items) => {
            for item in items {
                if let Some(explicit_ty) = pattern_binding_explicit_ty(item, name_span) {
                    return Some(explicit_ty);
                }
            }
            None
        }
        Pattern::App { args, .. } => {
            for arg in args {
                if let Some(explicit_ty) = pattern_binding_explicit_ty(arg, name_span) {
                    return Some(explicit_ty);
                }
            }
            None
        }
        Pattern::Struct { fields, .. } => {
            for field in fields {
                match &field.0 {
                    FieldPattern::Binding { pattern, .. } => {
                        if let Some(explicit_ty) = pattern_binding_explicit_ty(pattern, name_span) {
                            return Some(explicit_ty);
                        }
                    }
                    FieldPattern::Shorthand(name) if name.1 == name_span => {
                        return Some(None);
                    }
                    FieldPattern::Shorthand(_) | FieldPattern::Wild(_) => {}
                }
            }
            None
        }
        Pattern::Infix { lhs, rhs, .. } => pattern_binding_explicit_ty(lhs, name_span)
            .or_else(|| pattern_binding_explicit_ty(rhs, name_span)),
        Pattern::Wild
        | Pattern::Literal(_)
        | Pattern::TypeVar(_)
        | Pattern::Index { .. }
        | Pattern::RangeIndex { .. }
        | Pattern::Error(_) => None,
    }
}

fn target_binding_explicit_ty(target: &Spanned<Expr>, name_span: Span) -> Option<Option<Span>> {
    match &target.0 {
        Expr::Ident(_) if target.1 == name_span => Some(None),
        Expr::Cast { expr, ty } => target_binding_explicit_ty(expr, name_span)
            .map(|explicit_ty| explicit_ty.or(Some(ty.1))),
        _ => None,
    }
}

fn find_binding_value_in_expr<'a>(
    expr: &'a Spanned<Expr>,
    name_span: Span,
) -> Option<BindingValueAtSpan<'a>> {
    match &expr.0 {
        Expr::Attribute { expr, .. } => find_binding_value_in_expr(expr, name_span),
        Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
            find_binding_value_in_expr(lhs, name_span)
                .or_else(|| find_binding_value_in_expr(rhs, name_span))
        }
        Expr::Let { binding, body } => pattern_binding_explicit_ty(&binding.pattern, name_span)
            .map(|explicit_ty| BindingValueAtSpan {
                explicit_ty,
                value: &binding.value,
            })
            .or_else(|| {
                find_binding_value_in_expr(&binding.value, name_span)
                    .or_else(|| find_binding_value_in_expr(body, name_span))
            }),
        Expr::Var {
            target,
            value,
            body,
        } => target_binding_explicit_ty(target, name_span)
            .map(|explicit_ty| BindingValueAtSpan { explicit_ty, value })
            .or_else(|| {
                find_binding_value_in_expr(target, name_span)
                    .or_else(|| find_binding_value_in_expr(value, name_span))
                    .or_else(|| find_binding_value_in_expr(body, name_span))
            }),
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Let(binding) => {
                        if let Some(explicit_ty) =
                            pattern_binding_explicit_ty(&binding.pattern, name_span)
                        {
                            return Some(BindingValueAtSpan {
                                explicit_ty,
                                value: &binding.value,
                            });
                        }
                        if let Some(binding) = find_binding_value_in_expr(&binding.value, name_span)
                        {
                            return Some(binding);
                        }
                    }
                    BlockItem::Var { target, value } => {
                        if let Some(explicit_ty) = target_binding_explicit_ty(target, name_span) {
                            return Some(BindingValueAtSpan { explicit_ty, value });
                        }
                        if let Some(binding) = find_binding_value_in_expr(target, name_span) {
                            return Some(binding);
                        }
                        if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                            return Some(binding);
                        }
                    }
                    BlockItem::Expr(expr) => {
                        if let Some(binding) = find_binding_value_in_expr(expr, name_span) {
                            return Some(binding);
                        }
                    }
                }
            }
            None
        }
        Expr::Return(expr)
        | Expr::Throw(expr)
        | Expr::Exit(Some(expr))
        | Expr::Prefix { expr, .. }
        | Expr::Cast { expr, .. }
        | Expr::Field { expr, .. } => find_binding_value_in_expr(expr, name_span),
        Expr::Assert { condition, message } => find_binding_value_in_expr(condition, name_span)
            .or_else(|| {
                message
                    .as_ref()
                    .and_then(|expr| find_binding_value_in_expr(expr, name_span))
            }),
        Expr::Exit(None) => None,
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => find_binding_value_in_expr(cond, name_span)
            .or_else(|| find_binding_value_in_expr(then_branch, name_span))
            .or_else(|| {
                else_branch
                    .as_ref()
                    .and_then(|expr| find_binding_value_in_expr(expr, name_span))
            }),
        Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
            find_binding_value_in_expr(scrutinee, name_span).or_else(|| {
                for case in cases {
                    if let Some(explicit_ty) =
                        pattern_binding_explicit_ty(&case.0.pattern, name_span)
                    {
                        return Some(BindingValueAtSpan {
                            explicit_ty,
                            value: &case.0.body,
                        });
                    }
                    if let Some(guard) = &case.0.guard {
                        if let Some(binding) = find_binding_value_in_expr(guard, name_span) {
                            return Some(binding);
                        }
                    }
                    if let Some(binding) = find_binding_value_in_expr(&case.0.body, name_span) {
                        return Some(binding);
                    }
                }
                None
            })
        }
        Expr::Foreach(foreach) => find_binding_value_in_expr(&foreach.body, name_span),
        Expr::Repeat {
            measure,
            body,
            until,
        } => measure
            .as_ref()
            .and_then(|expr| find_binding_value_in_expr(expr, name_span))
            .or_else(|| find_binding_value_in_expr(body, name_span))
            .or_else(|| find_binding_value_in_expr(until, name_span)),
        Expr::While {
            measure,
            cond,
            body,
        } => measure
            .as_ref()
            .and_then(|expr| find_binding_value_in_expr(expr, name_span))
            .or_else(|| find_binding_value_in_expr(cond, name_span))
            .or_else(|| find_binding_value_in_expr(body, name_span)),
        Expr::Call(call) => find_binding_value_in_expr(&call.callee, name_span).or_else(|| {
            for arg in &call.args {
                if let Some(binding) = find_binding_value_in_expr(arg, name_span) {
                    return Some(binding);
                }
            }
            None
        }),
        Expr::Index { expr, index } => find_binding_value_in_expr(expr, name_span)
            .or_else(|| find_binding_value_in_expr(index, name_span)),
        Expr::Slice { expr, start, end } => find_binding_value_in_expr(expr, name_span)
            .or_else(|| find_binding_value_in_expr(start, name_span))
            .or_else(|| find_binding_value_in_expr(end, name_span)),
        Expr::VectorSlice {
            expr,
            start,
            length,
        } => find_binding_value_in_expr(expr, name_span)
            .or_else(|| find_binding_value_in_expr(start, name_span))
            .or_else(|| find_binding_value_in_expr(length, name_span)),
        Expr::Struct { fields, .. } => {
            for field in fields {
                if let FieldExpr::Assignment { target, value } = &field.0 {
                    if let Some(binding) = find_binding_value_in_expr(target, name_span) {
                        return Some(binding);
                    }
                    if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                        return Some(binding);
                    }
                }
            }
            None
        }
        Expr::Update { base, fields } => {
            find_binding_value_in_expr(base, name_span).or_else(|| {
                for field in fields {
                    if let FieldExpr::Assignment { target, value } = &field.0 {
                        if let Some(binding) = find_binding_value_in_expr(target, name_span) {
                            return Some(binding);
                        }
                        if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                            return Some(binding);
                        }
                    }
                }
                None
            })
        }
        Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
            for item in items {
                if let Some(binding) = find_binding_value_in_expr(item, name_span) {
                    return Some(binding);
                }
            }
            None
        }
        Expr::VectorUpdate { base, updates } => find_binding_value_in_expr(base, name_span)
            .or_else(|| {
                for update in updates {
                    match &update.0 {
                        VectorUpdate::Assignment { target, value } => {
                            if let Some(binding) = find_binding_value_in_expr(target, name_span) {
                                return Some(binding);
                            }
                            if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                                return Some(binding);
                            }
                        }
                        VectorUpdate::RangeAssignment { start, end, value } => {
                            if let Some(binding) = find_binding_value_in_expr(start, name_span) {
                                return Some(binding);
                            }
                            if let Some(binding) = find_binding_value_in_expr(end, name_span) {
                                return Some(binding);
                            }
                            if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                                return Some(binding);
                            }
                        }
                        VectorUpdate::Shorthand(_) => {}
                    }
                }
                None
            }),
        Expr::Literal(_)
        | Expr::Ident(_)
        | Expr::TypeVar(_)
        | Expr::Ref(_)
        | Expr::Config(_)
        | Expr::SizeOf(_)
        | Expr::Constraint(_)
        | Expr::Error(_) => None,
    }
}

pub fn find_binding_value_at_span_core(
    ast: &CoreSourceFile,
    name_span: Span,
) -> Option<BindingValueAtSpan<'_>> {
    for (item, _) in &ast.defs {
        match &item.kind {
            CoreDefinitionKind::Callable(def) => {
                if let Some(rec_measure) = &def.rec_measure {
                    if let Some(binding) =
                        find_binding_value_in_expr(&rec_measure.0.body, name_span)
                    {
                        return Some(binding);
                    }
                }
                for clause in &def.clauses {
                    if let Some(guard) = &clause.0.guard {
                        if let Some(binding) = find_binding_value_in_expr(guard, name_span) {
                            return Some(binding);
                        }
                    }
                    if let Some(body) = &clause.0.body {
                        if let Some(binding) = find_binding_value_in_expr(body, name_span) {
                            return Some(binding);
                        }
                    }
                    if let Some(body) = &clause.0.mapping_body {
                        for arm in &body.arms {
                            if let Some(binding) = find_binding_value_in_expr(&arm.0.lhs, name_span)
                            {
                                return Some(binding);
                            }
                            if let Some(binding) = find_binding_value_in_expr(&arm.0.rhs, name_span)
                            {
                                return Some(binding);
                            }
                            if let Some(guard) = &arm.0.guard {
                                if let Some(binding) = find_binding_value_in_expr(guard, name_span)
                                {
                                    return Some(binding);
                                }
                            }
                        }
                    }
                }
                if let Some(body) = &def.body {
                    if let Some(binding) = find_binding_value_in_expr(body, name_span) {
                        return Some(binding);
                    }
                }
                if let Some(body) = &def.mapping_body {
                    for arm in &body.arms {
                        if let Some(binding) = find_binding_value_in_expr(&arm.0.lhs, name_span) {
                            return Some(binding);
                        }
                        if let Some(binding) = find_binding_value_in_expr(&arm.0.rhs, name_span) {
                            return Some(binding);
                        }
                        if let Some(guard) = &arm.0.guard {
                            if let Some(binding) = find_binding_value_in_expr(guard, name_span) {
                                return Some(binding);
                            }
                        }
                    }
                }
            }
            CoreDefinitionKind::Named(def) => {
                if matches!(def.kind, NamedDefKind::Let | NamedDefKind::Var)
                    && def.name.1 == name_span
                {
                    if let Some(value) = &def.value {
                        return Some(BindingValueAtSpan {
                            explicit_ty: def.ty.as_ref().map(|ty| ty.1),
                            value,
                        });
                    }
                }
                if let Some(value) = &def.value {
                    if let Some(binding) = find_binding_value_in_expr(value, name_span) {
                        return Some(binding);
                    }
                }
            }
            CoreDefinitionKind::Scattered(_)
            | CoreDefinitionKind::ScatteredClause(_)
            | CoreDefinitionKind::CallableSpec(_)
            | CoreDefinitionKind::TypeAlias(_)
            | CoreDefinitionKind::Default(_)
            | CoreDefinitionKind::Fixity(_)
            | CoreDefinitionKind::Instantiation(_)
            | CoreDefinitionKind::Directive(_)
            | CoreDefinitionKind::End(_)
            | CoreDefinitionKind::Constraint(_)
            | CoreDefinitionKind::TerminationMeasure(_) => {}
        }
    }
    None
}

pub fn find_top_level_item_span_core(ast: &CoreSourceFile, name_span: Span) -> Option<Span> {
    ast.defs
        .iter()
        .find_map(|(item, item_span)| match &item.kind {
            CoreDefinitionKind::Scattered(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::ScatteredClause(def)
                if def.member.1 == name_span || def.name.1 == name_span =>
            {
                Some(*item_span)
            }
            CoreDefinitionKind::CallableSpec(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::Callable(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::TypeAlias(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::Named(def)
                if def.name.1 == name_span
                    || def.members.iter().any(|member| member.1 == name_span) =>
            {
                Some(*item_span)
            }
            CoreDefinitionKind::Default(def) if def.kind.1 == name_span => Some(*item_span),
            CoreDefinitionKind::Fixity(def) if def.operator.1 == name_span => Some(*item_span),
            CoreDefinitionKind::Instantiation(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::Directive(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::End(def) if def.name.1 == name_span => Some(*item_span),
            CoreDefinitionKind::TerminationMeasure(def) if def.name.1 == name_span => {
                Some(*item_span)
            }
            _ => None,
        })
}

pub fn find_named_members_core(
    ast: &CoreSourceFile,
    kind: NamedDefKind,
    name_span: Span,
) -> Option<&[Spanned<String>]> {
    ast.defs.iter().find_map(|(item, _)| match &item.kind {
        CoreDefinitionKind::Named(def) if def.kind == kind && def.name.1 == name_span => {
            Some(def.members.as_slice())
        }
        _ => None,
    })
}

pub fn find_enum_name_for_member_core<'a>(
    ast: &'a CoreSourceFile,
    member: &str,
) -> Option<&'a str> {
    ast.defs.iter().find_map(|(item, _)| match &item.kind {
        CoreDefinitionKind::Named(def)
            if def.kind == NamedDefKind::Enum
                && def.members.iter().any(|candidate| candidate.0 == member) =>
        {
            Some(def.name.0.as_str())
        }
        CoreDefinitionKind::ScatteredClause(def)
            if def.kind == ScatteredClauseKind::Enum && def.member.0 == member =>
        {
            Some(def.name.0.as_str())
        }
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chumsky::Parser;

    #[test]
    fn finds_smallest_nested_call_at_offset() {
        let source = "function foo(x) = outer(inner(x), x)";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let offset = source.find("inner").unwrap() + 2;

        let call = find_call_at_offset_core(&ast, offset).expect("call");
        assert_eq!(call.callee, "inner");
        assert_eq!(call.args.len(), 1);
    }

    #[test]
    fn finds_smallest_nested_call_at_offset_in_core_ast() {
        let source = "function foo(x) = outer(inner(x), x)";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let core_ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let offset = source.find("inner").unwrap() + 2;

        let call = find_call_at_offset_core(&core_ast, offset).expect("call");
        assert_eq!(call.callee, "inner");
        assert_eq!(call.args.len(), 1);
    }

    #[test]
    fn finds_call_in_top_level_let_initializer() {
        let source = "let result = outer(inner(1))";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let offset = source.find("inner").unwrap() + 2;

        let call = find_call_at_offset_core(&ast, offset).expect("call");
        assert_eq!(call.callee, "inner");
        assert_eq!(call.args.len(), 1);
    }

    #[test]
    fn finds_named_members_and_item_span() {
        let source = "overload foo = {bar, baz}\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let name_span = match &ast.defs[0].0.kind {
            CoreDefinitionKind::Named(def) => def.name.1,
            _ => panic!("expected named def"),
        };

        let item_span = find_top_level_item_span_core(&ast, name_span).expect("item span");
        let members =
            find_named_members_core(&ast, NamedDefKind::Overload, name_span).expect("members");

        assert_eq!(
            &source[item_span.start..item_span.end],
            "overload foo = {bar, baz}"
        );
        assert_eq!(
            members
                .iter()
                .map(|member| member.0.as_str())
                .collect::<Vec<_>>(),
            vec!["bar", "baz"]
        );
    }

    #[test]
    fn finds_struct_members_via_named_members_query() {
        let source = "struct S = { field1 : int, field2 : bits(3) }\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let name_span = match &ast.defs[0].0.kind {
            CoreDefinitionKind::Named(def) => def.name.1,
            _ => panic!("expected named def"),
        };

        let members =
            find_named_members_core(&ast, NamedDefKind::Struct, name_span).expect("members");
        assert_eq!(
            members
                .iter()
                .map(|member| member.0.as_str())
                .collect::<Vec<_>>(),
            vec!["field1", "field2"]
        );
    }

    #[test]
    fn finds_enum_name_for_member() {
        let source = "enum color = { Red, Green, Blue }\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();

        assert_eq!(find_enum_name_for_member_core(&ast, "Green"), Some("color"));
        assert_eq!(find_enum_name_for_member_core(&ast, "Purple"), None);
    }

    #[test]
    fn finds_item_metadata_in_core_ast() {
        let source = "overload foo = {bar, baz}\nenum color = { Red, Green, Blue }\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let core_ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let overload_name_span = match &core_ast.defs[0].0.kind {
            CoreDefinitionKind::Named(def) => def.name.1,
            _ => panic!("expected named def"),
        };

        let item_span =
            find_top_level_item_span_core(&core_ast, overload_name_span).expect("item span");
        let members =
            find_named_members_core(&core_ast, NamedDefKind::Overload, overload_name_span)
                .expect("members");

        assert_eq!(
            &source[item_span.start..item_span.end],
            "overload foo = {bar, baz}"
        );
        assert_eq!(
            members
                .iter()
                .map(|member| member.0.as_str())
                .collect::<Vec<_>>(),
            vec!["bar", "baz"]
        );
        assert_eq!(
            find_enum_name_for_member_core(&core_ast, "Green"),
            Some("color")
        );
        assert_eq!(find_enum_name_for_member_core(&core_ast, "Purple"), None);
    }

    #[test]
    fn finds_binding_value_for_local_let() {
        let source = "function foo() = { let x = bar(1); x }\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let start = source.find("x =").unwrap();
        let name_span = Span::new(start, start + 1);

        let binding = find_binding_value_at_span_core(&ast, name_span).expect("binding");
        assert_eq!(binding.explicit_ty, None);
        assert!(matches!(binding.value.0, Expr::Call(_)));
    }

    #[test]
    fn finds_binding_value_for_local_let_in_core_ast() {
        let source = "function foo() = { let x = bar(1); x }\n";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let core_ast = crate::parse_core_source(&tokens).into_result().unwrap();
        let start = source.find("x =").unwrap();
        let name_span = Span::new(start, start + 1);

        let binding = find_binding_value_at_span_core(&core_ast, name_span).expect("binding");
        assert_eq!(binding.explicit_ty, None);
        assert!(matches!(binding.value.0, Expr::Call(_)));
    }
}
