use crate::{
    core_ast::{
        BlockItem as AstBlockItem, Call as AstCall, CallableDefKind, CallableSpecKind,
        Expr as AstExpr, FieldExpr as AstFieldExpr, FieldPattern as AstFieldPattern,
        ForeachExpr as AstForeachExpr, Lexp as AstLexp, LoopMeasure as AstLoopMeasure,
        MappingBody as AstMappingBody, MatchCase as AstMatchCase,
        NamedDefDetail as AstNamedDefDetail, NamedDefKind, Pattern as AstPattern,
        ScatteredClauseKind, ScatteredKind, TerminationMeasureKind as AstTerminationMeasureKind,
        TypeExpr as AstTypeExpr, TypeParamSpec as AstTypeParamSpec,
        TypeParamTail as AstTypeParamTail, UnionPayload as AstUnionPayload,
    },
    core_ast::{
        CallableDefinition as CoreCallableDef, CallableSpecification as CoreCallableSpec,
        DefinitionKind as CoreDefinitionKind, NamedDefinition as CoreNamedDef,
        ScatteredClauseDefinition as CoreScatteredClauseDef,
        ScatteredDefinition as CoreScatteredDef, SourceFile as CoreSourceFile,
        TerminationMeasureDefinition as CoreTerminationMeasureDef,
        TypeAliasDefinition as CoreTypeAliasDef,
    },
    Span,
};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
use crate::ast::SourceFile;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    TopLevel,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Value,
    Mapping,
    Overload,
    Register,
    Parameter,
    Type,
    Struct,
    Union,
    Bitfield,
    Enum,
    EnumMember,
    Newtype,
    Let,
    Var,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclRole {
    Declaration,
    Definition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    pub role: DeclRole,
    pub scope: Scope,
    pub span: Span,
    pub is_scattered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedFile {
    pub decls: Vec<Decl>,
    pub type_aliases: Vec<TypeAlias>,
    pub call_sites: Vec<CallSite>,
    pub typed_bindings: Vec<TypedBinding>,
    pub callable_heads: Vec<CallableHead>,
    pub symbol_occurrences: Vec<SymbolOccurrence>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAlias {
    pub sub: String,
    pub sup: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub caller: Option<String>,
    pub callee: String,
    pub callee_span: Span,
    pub open_span: Span,
    pub close_span: Option<Span>,
    pub arg_separator_spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedBinding {
    pub name: String,
    pub name_span: Span,
    pub ty_span: Span,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableParam {
    pub span: Span,
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub ty_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableHead {
    pub name: String,
    pub kind: DeclKind,
    pub name_span: Span,
    pub label_span: Span,
    pub params: Vec<CallableParam>,
    pub return_type_span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolOccurrenceKind {
    Value,
    Type,
    TypeVar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolOccurrence {
    pub name: String,
    pub kind: SymbolOccurrenceKind,
    pub span: Span,
    pub scope: Option<Scope>,
    pub role: Option<DeclRole>,
    pub target_span: Option<Span>,
}

fn push_decl(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    scope: Scope,
    span: Span,
    is_scattered: bool,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope,
        span,
        is_scattered,
    });
}

fn decl_kind_for_scattered(kind: ScatteredKind) -> DeclKind {
    match kind {
        ScatteredKind::Function => DeclKind::Function,
        ScatteredKind::Mapping => DeclKind::Mapping,
        ScatteredKind::Union => DeclKind::Union,
        ScatteredKind::Enum => DeclKind::Enum,
    }
}

fn decl_kind_for_named(kind: NamedDefKind) -> DeclKind {
    match kind {
        NamedDefKind::Register => DeclKind::Register,
        NamedDefKind::Overload => DeclKind::Overload,
        NamedDefKind::Struct => DeclKind::Struct,
        NamedDefKind::Union => DeclKind::Union,
        NamedDefKind::Bitfield => DeclKind::Bitfield,
        NamedDefKind::Enum => DeclKind::Enum,
        NamedDefKind::Newtype => DeclKind::Newtype,
        NamedDefKind::Let => DeclKind::Let,
        NamedDefKind::Var => DeclKind::Var,
    }
}

fn callable_params_from_type_expr(ty: &(AstTypeExpr, Span)) -> Vec<CallableParam> {
    match &ty.0 {
        AstTypeExpr::Tuple(items) => items
            .iter()
            .map(|item| CallableParam {
                span: item.1,
                name: None,
                name_span: None,
                ty_span: Some(item.1),
            })
            .collect(),
        _ => vec![CallableParam {
            span: ty.1,
            name: None,
            name_span: None,
            ty_span: Some(ty.1),
        }],
    }
}

fn unwrapped_type_expr<'a>(ty: &'a (AstTypeExpr, Span)) -> &'a (AstTypeExpr, Span) {
    match &ty.0 {
        AstTypeExpr::Forall { body, .. } => unwrapped_type_expr(body),
        AstTypeExpr::Effect { ty, .. } => unwrapped_type_expr(ty),
        _ => ty,
    }
}

fn callable_head_from_core_spec(spec: &CoreCallableSpec, label_start: usize) -> CallableHead {
    callable_head_from_signature(
        &spec.name.0,
        match spec.kind {
            CallableSpecKind::Value => DeclKind::Value,
            CallableSpecKind::Mapping => DeclKind::Mapping,
        },
        label_start,
        spec.name.1,
        &spec.signature,
    )
}

fn callable_head_from_signature(
    name: &str,
    kind: DeclKind,
    label_start: usize,
    name_span: Span,
    signature: &(AstTypeExpr, Span),
) -> CallableHead {
    let signature = unwrapped_type_expr(signature);
    let (params, return_type_span) = match &signature.0 {
        AstTypeExpr::Arrow { params, ret, .. } => {
            let mut callable_params = Vec::new();
            for param in params {
                callable_params.extend(callable_params_from_type_expr(param));
            }
            (callable_params, Some(ret.1))
        }
        _ => (Vec::new(), Some(signature.1)),
    };

    CallableHead {
        name: name.to_string(),
        kind,
        name_span,
        label_span: Span::new(label_start, signature.1.end),
        params,
        return_type_span,
    }
}

fn callable_head_label_end_core(def: &CoreCallableDef) -> usize {
    let mut end = def.name.1.end;

    if let Some(signature) = &def.signature {
        end = end.max(signature.1.end);
    }
    if let Some(param) = def.params.last() {
        end = end.max(param.1.end);
    }
    if let Some(return_ty) = &def.return_ty {
        end = end.max(return_ty.1.end);
    }
    if let Some(clause) = def.clauses.first() {
        if let Some(pattern) = clause.0.patterns.last() {
            end = end.max(pattern.1.end);
        }
        if let Some(return_ty) = &clause.0.return_ty {
            end = end.max(return_ty.1.end);
        }
        if let Some(quantifier) = &clause.0.quantifier {
            if let Some(var) = quantifier.vars.last() {
                end = end.max(var.name.1.end);
            }
            if let Some(constraint) = &quantifier.constraint {
                end = end.max(constraint.1.end);
            }
        }
    }

    end
}

fn callable_param_from_pattern(pattern: &(AstPattern, Span)) -> CallableParam {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            let mut param = callable_param_from_pattern(inner);
            param.span = pattern.1;
            param
        }
        AstPattern::Typed(inner, ty) => {
            let mut param = callable_param_from_pattern(inner);
            param.span = pattern.1;
            param.ty_span = Some(ty.1);
            param
        }
        AstPattern::AsBinding { binding, .. } => CallableParam {
            span: pattern.1,
            name: Some(binding.0.clone()),
            name_span: Some(binding.1),
            ty_span: None,
        },
        AstPattern::Ident(name) => CallableParam {
            span: pattern.1,
            name: Some(name.clone()),
            name_span: Some(pattern.1),
            ty_span: None,
        },
        _ => CallableParam {
            span: pattern.1,
            name: None,
            name_span: None,
            ty_span: None,
        },
    }
}

fn collect_top_level_pattern_constants_core(ast: &CoreSourceFile) -> HashSet<String> {
    let mut constants = HashSet::new();

    for (item, _) in &ast.defs {
        match &item.kind {
            CoreDefinitionKind::Named(CoreNamedDef {
                kind: NamedDefKind::Enum,
                members,
                ..
            }) => {
                for member in members {
                    constants.insert(member.0.clone());
                }
            }
            CoreDefinitionKind::ScatteredClause(CoreScatteredClauseDef {
                kind: ScatteredClauseKind::Enum,
                member,
                ..
            }) => {
                constants.insert(member.0.clone());
            }
            _ => {}
        }
    }

    constants
}

fn pattern_ident_is_binding(name: &str, pattern_constants: &HashSet<String>) -> bool {
    !pattern_constants.contains(name)
}

fn collect_typed_bindings_from_pattern(
    pattern: &(AstPattern, Span),
    scope: Scope,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            collect_typed_bindings_from_pattern(inner, scope, pattern_constants, parsed);
        }
        AstPattern::Typed(inner, ty) => {
            if let AstPattern::Ident(name) = &inner.0 {
                if !pattern_ident_is_binding(name, pattern_constants) {
                    collect_typed_bindings_from_pattern(inner, scope, pattern_constants, parsed);
                    return;
                }
                parsed.typed_bindings.push(TypedBinding {
                    name: name.clone(),
                    name_span: inner.1,
                    ty_span: ty.1,
                    scope,
                });
            }
            collect_typed_bindings_from_pattern(inner, scope, pattern_constants, parsed);
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_typed_bindings_from_pattern(item, scope, pattern_constants, parsed);
            }
        }
        AstPattern::App { args, .. } => {
            for arg in args {
                collect_typed_bindings_from_pattern(arg, scope, pattern_constants, parsed);
            }
        }
        AstPattern::AsBinding { pattern, .. } => {
            collect_typed_bindings_from_pattern(pattern, scope, pattern_constants, parsed);
        }
        AstPattern::Struct { fields, .. } => {
            for field in fields {
                if let AstFieldPattern::Binding { pattern, .. } = &field.0 {
                    collect_typed_bindings_from_pattern(pattern, scope, pattern_constants, parsed);
                }
            }
        }
        AstPattern::Infix { lhs, rhs, .. } => {
            collect_typed_bindings_from_pattern(lhs, scope, pattern_constants, parsed);
            collect_typed_bindings_from_pattern(rhs, scope, pattern_constants, parsed);
        }
        AstPattern::AsType(inner, _) => {
            collect_typed_bindings_from_pattern(inner, scope, pattern_constants, parsed);
        }
        _ => {}
    }
}

fn collect_decl_names_from_pattern(
    pattern: &(AstPattern, Span),
    kind: DeclKind,
    scope: Scope,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            collect_decl_names_from_pattern(inner, kind, scope, pattern_constants, parsed);
        }
        AstPattern::Ident(name) => {
            if !pattern_ident_is_binding(name, pattern_constants) {
                return;
            }
            push_decl(
                parsed,
                kind,
                DeclRole::Definition,
                name,
                scope,
                pattern.1,
                false,
            );
        }
        AstPattern::Typed(inner, _) | AstPattern::AsType(inner, _) => {
            collect_decl_names_from_pattern(inner, kind, scope, pattern_constants, parsed);
        }
        AstPattern::AsBinding { pattern, binding } => {
            collect_decl_names_from_pattern(pattern, kind, scope, pattern_constants, parsed);
            push_decl(
                parsed,
                kind,
                DeclRole::Definition,
                &binding.0,
                scope,
                binding.1,
                false,
            );
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_decl_names_from_pattern(item, kind, scope, pattern_constants, parsed);
            }
        }
        AstPattern::App { args, .. } => {
            for arg in args {
                collect_decl_names_from_pattern(arg, kind, scope, pattern_constants, parsed);
            }
        }
        AstPattern::Struct { fields, .. } => {
            for field in fields {
                match &field.0 {
                    AstFieldPattern::Binding { pattern, .. } => {
                        collect_decl_names_from_pattern(
                            pattern,
                            kind,
                            scope,
                            pattern_constants,
                            parsed,
                        );
                    }
                    AstFieldPattern::Shorthand(name) => {
                        push_decl(
                            parsed,
                            kind,
                            DeclRole::Definition,
                            &name.0,
                            scope,
                            name.1,
                            false,
                        );
                    }
                    AstFieldPattern::Wild(_) => {}
                }
            }
        }
        AstPattern::Infix { lhs, rhs, .. } => {
            collect_decl_names_from_pattern(lhs, kind, scope, pattern_constants, parsed);
            collect_decl_names_from_pattern(rhs, kind, scope, pattern_constants, parsed);
        }
        _ => {}
    }
}

fn callee_name_and_span(expr: &(AstExpr, Span)) -> Option<(String, Span)> {
    match &expr.0 {
        AstExpr::Ident(name) => Some((name.clone(), expr.1)),
        AstExpr::Field { field, .. } => Some((field.0.clone(), field.1)),
        _ => None,
    }
}

fn var_target_name(expr: &(AstLexp, Span)) -> Option<(String, Span)> {
    match &expr.0 {
        AstLexp::Id(name) => Some((name.clone(), expr.1)),
        AstLexp::Attribute { lexp, .. } | AstLexp::Typed { lexp, .. } => var_target_name(lexp),
        _ => None,
    }
}

fn typed_var_target(expr: &(AstLexp, Span)) -> Option<(String, Span, Span)> {
    match &expr.0 {
        AstLexp::Typed { lexp, ty } => var_target_name(lexp).map(|(name, span)| (name, span, ty.1)),
        _ => None,
    }
}

fn collect_lexp_metadata(
    lexp: &(AstLexp, Span),
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    match &lexp.0 {
        AstLexp::Attribute { lexp, .. } | AstLexp::Typed { lexp, .. } => {
            collect_lexp_metadata(lexp, caller, pattern_constants, parsed)
        }
        AstLexp::Deref(expr) => collect_expr_metadata(expr, caller, pattern_constants, parsed),
        AstLexp::Call(call) => collect_call_metadata(call, caller, pattern_constants, parsed),
        AstLexp::Field { lexp, .. } => {
            collect_lexp_metadata(lexp, caller, pattern_constants, parsed)
        }
        AstLexp::Vector { lexp, index } => {
            collect_lexp_metadata(lexp, caller, pattern_constants, parsed);
            collect_expr_metadata(index, caller, pattern_constants, parsed);
        }
        AstLexp::VectorRange { lexp, start, end } => {
            collect_lexp_metadata(lexp, caller, pattern_constants, parsed);
            collect_expr_metadata(start, caller, pattern_constants, parsed);
            collect_expr_metadata(end, caller, pattern_constants, parsed);
        }
        AstLexp::VectorConcat(items) | AstLexp::Tuple(items) => {
            for item in items {
                collect_lexp_metadata(item, caller, pattern_constants, parsed);
            }
        }
        AstLexp::Id(_) | AstLexp::Error(_) => {}
    }
}

fn collect_field_expr_metadata(
    field: &(AstFieldExpr, Span),
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    if let AstFieldExpr::Assignment { target, value } = &field.0 {
        collect_expr_metadata(target, caller, pattern_constants, parsed);
        collect_expr_metadata(value, caller, pattern_constants, parsed);
    }
}

fn collect_case_metadata(
    case: &(AstMatchCase, Span),
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    collect_decl_names_from_pattern(
        &case.0.pattern,
        DeclKind::Let,
        Scope::Local,
        pattern_constants,
        parsed,
    );
    collect_typed_bindings_from_pattern(&case.0.pattern, Scope::Local, pattern_constants, parsed);
    if let Some(guard) = &case.0.guard {
        collect_expr_metadata(guard, caller, pattern_constants, parsed);
    }
    collect_expr_metadata(&case.0.body, caller, pattern_constants, parsed);
}

fn collect_call_metadata(
    call: &AstCall,
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    if let Some((callee, callee_span)) = callee_name_and_span(&call.callee) {
        parsed.call_sites.push(CallSite {
            caller: caller.map(|name| name.to_string()),
            callee,
            callee_span,
            open_span: call.open_span,
            close_span: Some(call.close_span),
            arg_separator_spans: call.arg_separator_spans.clone(),
        });
    }
    collect_expr_metadata(&call.callee, caller, pattern_constants, parsed);
    for arg in &call.args {
        collect_expr_metadata(arg, caller, pattern_constants, parsed);
    }
}

fn collect_mapping_body_metadata(
    body: &AstMappingBody,
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    for arm in &body.arms {
        if let Some(pattern) = &arm.0.lhs_pattern {
            collect_decl_names_from_pattern(
                pattern,
                DeclKind::Let,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(pattern, Scope::Local, pattern_constants, parsed);
        }
        if let Some(pattern) = &arm.0.rhs_pattern {
            collect_decl_names_from_pattern(
                pattern,
                DeclKind::Let,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(pattern, Scope::Local, pattern_constants, parsed);
        }
        collect_expr_metadata(&arm.0.lhs, caller, pattern_constants, parsed);
        collect_expr_metadata(&arm.0.rhs, caller, pattern_constants, parsed);
        if let Some(guard) = &arm.0.guard {
            collect_expr_metadata(guard, caller, pattern_constants, parsed);
        }
    }
}

fn collect_core_callable_clause_metadata(
    def: &CoreCallableDef,
    caller: &str,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    if let Some(rec_measure) = &def.rec_measure {
        collect_decl_names_from_pattern(
            &rec_measure.0.pattern,
            DeclKind::Let,
            Scope::Local,
            pattern_constants,
            parsed,
        );
        collect_typed_bindings_from_pattern(
            &rec_measure.0.pattern,
            Scope::Local,
            pattern_constants,
            parsed,
        );
        collect_expr_metadata(&rec_measure.0.body, Some(caller), pattern_constants, parsed);
    }

    if def.clauses.is_empty() {
        for param in &def.params {
            collect_decl_names_from_pattern(
                param,
                DeclKind::Parameter,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(param, Scope::Local, pattern_constants, parsed);
        }
        if let Some(body) = &def.body {
            collect_expr_metadata(body, Some(caller), pattern_constants, parsed);
        }
        if let Some(mapping_body) = &def.mapping_body {
            collect_mapping_body_metadata(mapping_body, Some(caller), pattern_constants, parsed);
        }
        return;
    }

    for clause in &def.clauses {
        for pattern in &clause.0.patterns {
            collect_decl_names_from_pattern(
                pattern,
                DeclKind::Parameter,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(pattern, Scope::Local, pattern_constants, parsed);
        }
        if let Some(guard) = &clause.0.guard {
            collect_expr_metadata(guard, Some(caller), pattern_constants, parsed);
        }
        if let Some(body) = &clause.0.body {
            collect_expr_metadata(body, Some(caller), pattern_constants, parsed);
        }
        if let Some(mapping_body) = &clause.0.mapping_body {
            collect_mapping_body_metadata(mapping_body, Some(caller), pattern_constants, parsed);
        }
    }
}

fn collect_termination_measure_metadata(
    kind: &AstTerminationMeasureKind,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    match kind {
        AstTerminationMeasureKind::Function { pattern, body } => {
            collect_decl_names_from_pattern(
                pattern,
                DeclKind::Let,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(pattern, Scope::Local, pattern_constants, parsed);
            collect_expr_metadata(body, None, pattern_constants, parsed);
        }
        AstTerminationMeasureKind::Loop { measures } => {
            for measure in measures {
                match &measure.0 {
                    AstLoopMeasure::Until(expr)
                    | AstLoopMeasure::Repeat(expr)
                    | AstLoopMeasure::While(expr) => {
                        collect_expr_metadata(expr, None, pattern_constants, parsed)
                    }
                }
            }
        }
    }
}

fn collect_expr_metadata(
    expr: &(AstExpr, Span),
    caller: Option<&str>,
    pattern_constants: &HashSet<String>,
    parsed: &mut ParsedFile,
) {
    match &expr.0 {
        AstExpr::Attribute { expr, .. } => {
            collect_expr_metadata(expr, caller, pattern_constants, parsed)
        }
        AstExpr::Assign { lhs, rhs } => {
            collect_lexp_metadata(lhs, caller, pattern_constants, parsed);
            collect_expr_metadata(rhs, caller, pattern_constants, parsed);
        }
        AstExpr::Infix { lhs, rhs, .. } => {
            collect_expr_metadata(lhs, caller, pattern_constants, parsed);
            collect_expr_metadata(rhs, caller, pattern_constants, parsed);
        }
        AstExpr::Let { binding, body } => {
            collect_decl_names_from_pattern(
                &binding.pattern,
                DeclKind::Let,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_typed_bindings_from_pattern(
                &binding.pattern,
                Scope::Local,
                pattern_constants,
                parsed,
            );
            collect_expr_metadata(&binding.value, caller, pattern_constants, parsed);
            collect_expr_metadata(body, caller, pattern_constants, parsed);
        }
        AstExpr::Var {
            target,
            value,
            body,
        } => {
            if let Some((name, span)) = var_target_name(target) {
                push_decl(
                    parsed,
                    DeclKind::Var,
                    DeclRole::Definition,
                    &name,
                    Scope::Local,
                    span,
                    false,
                );
            }
            if let Some((name, span, ty_span)) = typed_var_target(target) {
                parsed.typed_bindings.push(TypedBinding {
                    name,
                    name_span: span,
                    ty_span,
                    scope: Scope::Local,
                });
            }
            collect_lexp_metadata(target, caller, pattern_constants, parsed);
            collect_expr_metadata(value, caller, pattern_constants, parsed);
            collect_expr_metadata(body, caller, pattern_constants, parsed);
        }
        AstExpr::Block(items) => {
            for item in items {
                match &item.0 {
                    AstBlockItem::Let(binding) => {
                        collect_decl_names_from_pattern(
                            &binding.pattern,
                            DeclKind::Let,
                            Scope::Local,
                            pattern_constants,
                            parsed,
                        );
                        collect_typed_bindings_from_pattern(
                            &binding.pattern,
                            Scope::Local,
                            pattern_constants,
                            parsed,
                        );
                        collect_expr_metadata(&binding.value, caller, pattern_constants, parsed);
                    }
                    AstBlockItem::Var { target, value } => {
                        if let Some((name, span)) = var_target_name(target) {
                            push_decl(
                                parsed,
                                DeclKind::Var,
                                DeclRole::Definition,
                                &name,
                                Scope::Local,
                                span,
                                false,
                            );
                        }
                        if let Some((name, span, ty_span)) = typed_var_target(target) {
                            parsed.typed_bindings.push(TypedBinding {
                                name,
                                name_span: span,
                                ty_span,
                                scope: Scope::Local,
                            });
                        }
                        collect_lexp_metadata(target, caller, pattern_constants, parsed);
                        collect_expr_metadata(value, caller, pattern_constants, parsed);
                    }
                    AstBlockItem::Expr(expr) => {
                        collect_expr_metadata(expr, caller, pattern_constants, parsed)
                    }
                }
            }
        }
        AstExpr::Return(expr)
        | AstExpr::Throw(expr)
        | AstExpr::Exit(Some(expr))
        | AstExpr::Prefix { expr, .. }
        | AstExpr::Cast { expr, .. }
        | AstExpr::Field { expr, .. } => {
            collect_expr_metadata(expr, caller, pattern_constants, parsed)
        }
        AstExpr::Assert { condition, message } => {
            collect_expr_metadata(condition, caller, pattern_constants, parsed);
            if let Some(message) = message {
                collect_expr_metadata(message, caller, pattern_constants, parsed);
            }
        }
        AstExpr::Exit(None) => {}
        AstExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_metadata(cond, caller, pattern_constants, parsed);
            collect_expr_metadata(then_branch, caller, pattern_constants, parsed);
            if let Some(else_branch) = else_branch {
                collect_expr_metadata(else_branch, caller, pattern_constants, parsed);
            }
        }
        AstExpr::Match { scrutinee, cases } | AstExpr::Try { scrutinee, cases } => {
            collect_expr_metadata(scrutinee, caller, pattern_constants, parsed);
            for case in cases {
                collect_case_metadata(case, caller, pattern_constants, parsed);
            }
        }
        AstExpr::Foreach(AstForeachExpr { body, .. }) => {
            collect_expr_metadata(body, caller, pattern_constants, parsed);
        }
        AstExpr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                collect_expr_metadata(measure, caller, pattern_constants, parsed);
            }
            collect_expr_metadata(body, caller, pattern_constants, parsed);
            collect_expr_metadata(until, caller, pattern_constants, parsed);
        }
        AstExpr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                collect_expr_metadata(measure, caller, pattern_constants, parsed);
            }
            collect_expr_metadata(cond, caller, pattern_constants, parsed);
            collect_expr_metadata(body, caller, pattern_constants, parsed);
        }
        AstExpr::Call(call) => collect_call_metadata(call, caller, pattern_constants, parsed),
        AstExpr::Struct { fields, .. } => {
            for field in fields {
                collect_field_expr_metadata(field, caller, pattern_constants, parsed);
            }
        }
        AstExpr::Update { base, fields } => {
            collect_expr_metadata(base, caller, pattern_constants, parsed);
            for field in fields {
                collect_field_expr_metadata(field, caller, pattern_constants, parsed);
            }
        }
        AstExpr::List(items) | AstExpr::Vector(items) | AstExpr::Tuple(items) => {
            for item in items {
                collect_expr_metadata(item, caller, pattern_constants, parsed);
            }
        }
        AstExpr::Literal(_)
        | AstExpr::Ident(_)
        | AstExpr::TypeVar(_)
        | AstExpr::Ref(_)
        | AstExpr::Config(_)
        | AstExpr::SizeOf(_)
        | AstExpr::Constraint(_)
        | AstExpr::Error(_) => {}
    }
}

fn decl_occurrence_kind(kind: DeclKind) -> Option<SymbolOccurrenceKind> {
    match kind {
        DeclKind::Function
        | DeclKind::Value
        | DeclKind::Mapping
        | DeclKind::Overload
        | DeclKind::Register
        | DeclKind::Parameter
        | DeclKind::EnumMember
        | DeclKind::Let
        | DeclKind::Var => Some(SymbolOccurrenceKind::Value),
        DeclKind::Type
        | DeclKind::Struct
        | DeclKind::Union
        | DeclKind::Bitfield
        | DeclKind::Enum
        | DeclKind::Newtype => Some(SymbolOccurrenceKind::Type),
    }
}

struct SymbolOccurrenceCollector {
    top_level_values: HashSet<String>,
    top_level_types: HashSet<String>,
    pattern_constants: HashSet<String>,
    occurrences: Vec<SymbolOccurrence>,
    value_scopes: Vec<HashMap<String, Span>>,
    type_var_scopes: Vec<HashMap<String, Span>>,
}

impl SymbolOccurrenceCollector {
    fn new(parsed: &ParsedFile) -> Self {
        let mut top_level_values = HashSet::new();
        let mut top_level_types = HashSet::new();
        let mut pattern_constants = HashSet::new();
        let mut occurrences = Vec::new();

        for decl in &parsed.decls {
            let Some(kind) = decl_occurrence_kind(decl.kind) else {
                continue;
            };
            if decl.scope == Scope::TopLevel {
                if decl.kind == DeclKind::EnumMember {
                    pattern_constants.insert(decl.name.clone());
                }
                match kind {
                    SymbolOccurrenceKind::Value => {
                        top_level_values.insert(decl.name.clone());
                    }
                    SymbolOccurrenceKind::Type => {
                        top_level_types.insert(decl.name.clone());
                    }
                    SymbolOccurrenceKind::TypeVar => {}
                }
                occurrences.push(SymbolOccurrence {
                    name: decl.name.clone(),
                    kind,
                    span: decl.span,
                    scope: Some(Scope::TopLevel),
                    role: Some(decl.role),
                    target_span: None,
                });
            }
        }

        Self {
            top_level_values,
            top_level_types,
            pattern_constants,
            occurrences,
            value_scopes: Vec::new(),
            type_var_scopes: Vec::new(),
        }
    }

    fn is_pattern_constant(&self, name: &str) -> bool {
        self.pattern_constants.contains(name)
    }

    fn push_value_scope(&mut self) {
        self.value_scopes.push(HashMap::new());
    }

    fn pop_value_scope(&mut self) {
        self.value_scopes.pop();
    }

    fn push_type_var_scope(&mut self) {
        self.type_var_scopes.push(HashMap::new());
    }

    fn pop_type_var_scope(&mut self) {
        self.type_var_scopes.pop();
    }

    fn define_value(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.value_scopes.last_mut() {
            scope.insert(name.to_string(), span);
        }
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Value,
            span,
            scope: Some(Scope::Local),
            role: Some(DeclRole::Definition),
            target_span: Some(span),
        });
    }

    fn define_type_var(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.type_var_scopes.last_mut() {
            scope.insert(name.to_string(), span);
        }
        self.occurrences.push(SymbolOccurrence {
            name: format!("'{name}"),
            kind: SymbolOccurrenceKind::TypeVar,
            span,
            scope: Some(Scope::Local),
            role: Some(DeclRole::Definition),
            target_span: Some(span),
        });
    }

    fn resolve_value(&self, name: &str) -> (Option<Scope>, Option<Span>) {
        for scope in self.value_scopes.iter().rev() {
            if let Some(span) = scope.get(name).copied() {
                return (Some(Scope::Local), Some(span));
            }
        }
        if self.top_level_values.contains(name) {
            (Some(Scope::TopLevel), None)
        } else {
            (None, None)
        }
    }

    fn resolve_type_var(&self, name: &str) -> Option<Span> {
        for scope in self.type_var_scopes.iter().rev() {
            if let Some(span) = scope.get(name).copied() {
                return Some(span);
            }
        }
        None
    }

    fn record_value_ref(&mut self, name: &str, span: Span) {
        let (scope, target_span) = self.resolve_value(name);
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Value,
            span,
            scope,
            role: None,
            target_span,
        });
    }

    fn record_type_ref(&mut self, name: &str, span: Span) {
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Type,
            span,
            scope: self
                .top_level_types
                .contains(name)
                .then_some(Scope::TopLevel),
            role: None,
            target_span: None,
        });
    }

    fn record_type_var_ref(&mut self, name: &str, span: Span) {
        let target_span = self.resolve_type_var(name);
        self.occurrences.push(SymbolOccurrence {
            name: format!("'{name}"),
            kind: SymbolOccurrenceKind::TypeVar,
            span,
            scope: target_span.map(|_| Scope::Local),
            role: None,
            target_span,
        });
    }
}

fn collect_pattern_symbol_occurrences(
    pattern: &(AstPattern, Span),
    collector: &mut SymbolOccurrenceCollector,
    bindings: &mut Vec<(String, Span)>,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern, .. } => {
            collect_pattern_symbol_occurrences(pattern, collector, bindings);
        }
        AstPattern::Wild | AstPattern::Literal(_) | AstPattern::Error(_) => {}
        AstPattern::Ident(name) => {
            if collector.is_pattern_constant(name) {
                collector.record_value_ref(name, pattern.1);
            } else {
                bindings.push((name.clone(), pattern.1));
            }
        }
        AstPattern::TypeVar(name) => collector.record_type_var_ref(name, pattern.1),
        AstPattern::Typed(inner, ty) | AstPattern::AsType(inner, ty) => {
            collect_pattern_symbol_occurrences(inner, collector, bindings);
            collect_type_symbol_occurrences(ty, collector);
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_pattern_symbol_occurrences(item, collector, bindings);
            }
        }
        AstPattern::App { callee, args } => {
            collector.record_value_ref(&callee.0, callee.1);
            for arg in args {
                collect_pattern_symbol_occurrences(arg, collector, bindings);
            }
        }
        AstPattern::Index { name, index } => {
            collector.record_value_ref(&name.0, name.1);
            collect_type_symbol_occurrences(index, collector);
        }
        AstPattern::RangeIndex { name, start, end } => {
            collector.record_value_ref(&name.0, name.1);
            collect_type_symbol_occurrences(start, collector);
            collect_type_symbol_occurrences(end, collector);
        }
        AstPattern::Struct { name, fields } => {
            if let Some(name) = name {
                collector.record_type_ref(&name.0, name.1);
            }
            for field in fields {
                match &field.0 {
                    AstFieldPattern::Binding { pattern, .. } => {
                        collect_pattern_symbol_occurrences(pattern, collector, bindings);
                    }
                    AstFieldPattern::Shorthand(name) => bindings.push((name.0.clone(), name.1)),
                    AstFieldPattern::Wild(_) => {}
                }
            }
        }
        AstPattern::Infix { lhs, op, rhs } => {
            collect_pattern_symbol_occurrences(lhs, collector, bindings);
            collector.record_value_ref(&op.0, op.1);
            collect_pattern_symbol_occurrences(rhs, collector, bindings);
        }
        AstPattern::AsBinding { pattern, binding } => {
            collect_pattern_symbol_occurrences(pattern, collector, bindings);
            bindings.push((binding.0.clone(), binding.1));
        }
    }
}

fn visit_var_target_symbol_occurrences(
    target: &(AstLexp, Span),
    collector: &mut SymbolOccurrenceCollector,
) -> Option<(String, Span)> {
    match &target.0 {
        AstLexp::Id(name) => Some((name.clone(), target.1)),
        AstLexp::Attribute { lexp, .. } => visit_var_target_symbol_occurrences(lexp, collector),
        AstLexp::Typed { lexp, ty } => {
            collect_type_symbol_occurrences(ty, collector);
            visit_var_target_symbol_occurrences(lexp, collector)
        }
        _ => {
            collect_lexp_symbol_occurrences(target, collector);
            None
        }
    }
}

fn collect_lexp_symbol_occurrences(
    lexp: &(AstLexp, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &lexp.0 {
        AstLexp::Attribute { lexp, .. } => collect_lexp_symbol_occurrences(lexp, collector),
        AstLexp::Typed { lexp, ty } => {
            collect_type_symbol_occurrences(ty, collector);
            collect_lexp_symbol_occurrences(lexp, collector);
        }
        AstLexp::Id(name) => collector.record_value_ref(name, lexp.1),
        AstLexp::Deref(expr) => collect_expr_symbol_occurrences(expr, collector),
        AstLexp::Call(call) => {
            collect_expr_symbol_occurrences(&call.callee, collector);
            for arg in &call.args {
                collect_expr_symbol_occurrences(arg, collector);
            }
        }
        AstLexp::Field { lexp, .. } => {
            collect_lexp_symbol_occurrences(lexp, collector);
        }
        AstLexp::Vector { lexp, index } => {
            collect_lexp_symbol_occurrences(lexp, collector);
            collect_expr_symbol_occurrences(index, collector);
        }
        AstLexp::VectorRange { lexp, start, end } => {
            collect_lexp_symbol_occurrences(lexp, collector);
            collect_expr_symbol_occurrences(start, collector);
            collect_expr_symbol_occurrences(end, collector);
        }
        AstLexp::VectorConcat(items) | AstLexp::Tuple(items) => {
            for item in items {
                collect_lexp_symbol_occurrences(item, collector);
            }
        }
        AstLexp::Error(_) => {}
    }
}

fn collect_type_param_defs(params: &AstTypeParamSpec, collector: &mut SymbolOccurrenceCollector) {
    for param in &params.params {
        collector.define_type_var(&param.name.0, param.name.1);
    }
    if let Some(tail) = &params.tail {
        match tail {
            AstTypeParamTail::Type(ty) | AstTypeParamTail::Constraint(ty) => {
                collect_type_symbol_occurrences(ty, collector);
            }
        }
    }
}

fn collect_type_symbol_occurrences(
    ty: &(AstTypeExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &ty.0 {
        AstTypeExpr::Wild
        | AstTypeExpr::Literal(_)
        | AstTypeExpr::Dec
        | AstTypeExpr::Inc
        | AstTypeExpr::Config(_)
        | AstTypeExpr::Error(_) => {}
        AstTypeExpr::Named(name) => collector.record_type_ref(name, ty.1),
        AstTypeExpr::TypeVar(name) => collector.record_type_var_ref(name, ty.1),
        AstTypeExpr::Forall {
            vars,
            constraint,
            body,
        } => {
            collector.push_type_var_scope();
            for var in vars {
                collector.define_type_var(&var.name.0, var.name.1);
            }
            if let Some(constraint) = constraint {
                collect_type_symbol_occurrences(constraint, collector);
            }
            collect_type_symbol_occurrences(body, collector);
            collector.pop_type_var_scope();
        }
        AstTypeExpr::Existential {
            binder,
            constraint,
            body,
        } => {
            collector.push_type_var_scope();
            collector.define_type_var(&binder.name.0, binder.name.1);
            if let Some(constraint) = constraint {
                collect_type_symbol_occurrences(constraint, collector);
            }
            collect_type_symbol_occurrences(body, collector);
            collector.pop_type_var_scope();
        }
        AstTypeExpr::Effect { ty, .. } => collect_type_symbol_occurrences(ty, collector),
        AstTypeExpr::App { callee, args } => {
            collector.record_type_ref(&callee.0, callee.1);
            for arg in args {
                collect_type_symbol_occurrences(arg, collector);
            }
        }
        AstTypeExpr::Tuple(items) | AstTypeExpr::Set(items) => {
            for item in items {
                collect_type_symbol_occurrences(item, collector);
            }
        }
        AstTypeExpr::Register(inner) | AstTypeExpr::Prefix { ty: inner, .. } => {
            collect_type_symbol_occurrences(inner, collector);
        }
        AstTypeExpr::Infix { lhs, op, rhs } => {
            collect_type_symbol_occurrences(lhs, collector);
            collector.record_type_ref(&op.0, op.1);
            collect_type_symbol_occurrences(rhs, collector);
        }
        AstTypeExpr::Conditional {
            cond,
            then_ty,
            else_ty,
        } => {
            collect_type_symbol_occurrences(cond, collector);
            collect_type_symbol_occurrences(then_ty, collector);
            collect_type_symbol_occurrences(else_ty, collector);
        }
        AstTypeExpr::Arrow { params, ret, .. } => {
            for param in params {
                collect_type_symbol_occurrences(param, collector);
            }
            collect_type_symbol_occurrences(ret, collector);
        }
    }
}

fn collect_field_expr_symbol_occurrences(
    field: &(AstFieldExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &field.0 {
        AstFieldExpr::Assignment { target, value } => {
            collect_expr_symbol_occurrences(target, collector);
            collect_expr_symbol_occurrences(value, collector);
        }
        AstFieldExpr::Shorthand(name) => collector.record_value_ref(&name.0, name.1),
    }
}

fn collect_case_symbol_occurrences(
    case: &(AstMatchCase, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    let mut bindings = Vec::new();
    collect_pattern_symbol_occurrences(&case.0.pattern, collector, &mut bindings);
    collector.push_value_scope();
    for (name, span) in bindings {
        collector.define_value(&name, span);
    }
    if let Some(guard) = &case.0.guard {
        collect_expr_symbol_occurrences(guard, collector);
    }
    collect_expr_symbol_occurrences(&case.0.body, collector);
    collector.pop_value_scope();
}

fn collect_mapping_body_symbol_occurrences(
    body: &AstMappingBody,
    collector: &mut SymbolOccurrenceCollector,
) {
    for arm in &body.arms {
        let mut bindings = Vec::new();
        if let Some(pattern) = &arm.0.lhs_pattern {
            collect_pattern_symbol_occurrences(pattern, collector, &mut bindings);
        }
        if let Some(pattern) = &arm.0.rhs_pattern {
            collect_pattern_symbol_occurrences(pattern, collector, &mut bindings);
        }
        collector.push_value_scope();
        for (name, span) in bindings {
            collector.define_value(&name, span);
        }
        if let Some(guard) = &arm.0.guard {
            collect_expr_symbol_occurrences(guard, collector);
        }
        collect_expr_symbol_occurrences(&arm.0.lhs, collector);
        collect_expr_symbol_occurrences(&arm.0.rhs, collector);
        collector.pop_value_scope();
    }
}

fn collect_expr_symbol_occurrences(
    expr: &(AstExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &expr.0 {
        AstExpr::Attribute { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Assign { lhs, rhs } => {
            collect_lexp_symbol_occurrences(lhs, collector);
            collect_expr_symbol_occurrences(rhs, collector);
        }
        AstExpr::Let { binding, body } => {
            collect_expr_symbol_occurrences(&binding.value, collector);
            let mut bindings = Vec::new();
            collect_pattern_symbol_occurrences(&binding.pattern, collector, &mut bindings);
            collector.push_value_scope();
            for (name, span) in bindings {
                collector.define_value(&name, span);
            }
            collect_expr_symbol_occurrences(body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Var {
            target,
            value,
            body,
        } => {
            let binding = visit_var_target_symbol_occurrences(target, collector);
            collect_expr_symbol_occurrences(value, collector);
            collector.push_value_scope();
            if let Some((name, span)) = binding {
                collector.define_value(&name, span);
            }
            collect_expr_symbol_occurrences(body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Block(items) => {
            collector.push_value_scope();
            for item in items {
                match &item.0 {
                    AstBlockItem::Let(binding) => {
                        collect_expr_symbol_occurrences(&binding.value, collector);
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(
                            &binding.pattern,
                            collector,
                            &mut bindings,
                        );
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    AstBlockItem::Var { target, value } => {
                        let binding = visit_var_target_symbol_occurrences(target, collector);
                        collect_expr_symbol_occurrences(value, collector);
                        if let Some((name, span)) = binding {
                            collector.define_value(&name, span);
                        }
                    }
                    AstBlockItem::Expr(expr) => collect_expr_symbol_occurrences(expr, collector),
                }
            }
            collector.pop_value_scope();
        }
        AstExpr::Return(expr)
        | AstExpr::Throw(expr)
        | AstExpr::Exit(Some(expr))
        | AstExpr::Prefix { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Assert { condition, message } => {
            collect_expr_symbol_occurrences(condition, collector);
            if let Some(message) = message {
                collect_expr_symbol_occurrences(message, collector);
            }
        }
        AstExpr::Cast { expr, ty } => {
            collect_expr_symbol_occurrences(expr, collector);
            collect_type_symbol_occurrences(ty, collector);
        }
        AstExpr::Field { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Exit(None) => {}
        AstExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_symbol_occurrences(cond, collector);
            collect_expr_symbol_occurrences(then_branch, collector);
            if let Some(else_branch) = else_branch {
                collect_expr_symbol_occurrences(else_branch, collector);
            }
        }
        AstExpr::Match { scrutinee, cases } | AstExpr::Try { scrutinee, cases } => {
            collect_expr_symbol_occurrences(scrutinee, collector);
            for case in cases {
                collect_case_symbol_occurrences(case, collector);
            }
        }
        AstExpr::Foreach(foreach) => {
            collect_expr_symbol_occurrences(&foreach.start, collector);
            collect_expr_symbol_occurrences(&foreach.end, collector);
            if let Some(step) = &foreach.step {
                collect_expr_symbol_occurrences(step, collector);
            }
            if let Some(ty) = &foreach.ty {
                collect_type_symbol_occurrences(ty, collector);
            }
            collector.push_value_scope();
            collector.define_value(&foreach.iterator.0, foreach.iterator.1);
            collect_expr_symbol_occurrences(&foreach.body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                collect_expr_symbol_occurrences(measure, collector);
            }
            collect_expr_symbol_occurrences(body, collector);
            collect_expr_symbol_occurrences(until, collector);
        }
        AstExpr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                collect_expr_symbol_occurrences(measure, collector);
            }
            collect_expr_symbol_occurrences(cond, collector);
            collect_expr_symbol_occurrences(body, collector);
        }
        AstExpr::Infix { lhs, op, rhs } => {
            collect_expr_symbol_occurrences(lhs, collector);
            collector.record_value_ref(&op.0, op.1);
            collect_expr_symbol_occurrences(rhs, collector);
        }
        AstExpr::Config(_) | AstExpr::Literal(_) | AstExpr::Error(_) => {}
        AstExpr::Ident(name) => collector.record_value_ref(name, expr.1),
        AstExpr::TypeVar(name) => collector.record_type_var_ref(name, expr.1),
        AstExpr::Ref(name) => collector.record_value_ref(&name.0, name.1),
        AstExpr::Call(call) => {
            collect_expr_symbol_occurrences(&call.callee, collector);
            for arg in &call.args {
                collect_expr_symbol_occurrences(arg, collector);
            }
        }
        AstExpr::SizeOf(ty) | AstExpr::Constraint(ty) => {
            collect_type_symbol_occurrences(ty, collector);
        }
        AstExpr::Struct { name, fields } => {
            if let Some(name) = name {
                collector.record_type_ref(&name.0, name.1);
            }
            for field in fields {
                collect_field_expr_symbol_occurrences(field, collector);
            }
        }
        AstExpr::Update { base, fields } => {
            collect_expr_symbol_occurrences(base, collector);
            for field in fields {
                collect_field_expr_symbol_occurrences(field, collector);
            }
        }
        AstExpr::List(items) | AstExpr::Vector(items) | AstExpr::Tuple(items) => {
            for item in items {
                collect_expr_symbol_occurrences(item, collector);
            }
        }
    }
}

fn collect_named_detail_symbol_occurrences(
    detail: &AstNamedDefDetail,
    collector: &mut SymbolOccurrenceCollector,
) {
    match detail {
        AstNamedDefDetail::Enum { members, functions } => {
            for member in members {
                if let Some(value) = &member.0.value {
                    collect_expr_symbol_occurrences(value, collector);
                }
            }
            for function in functions {
                collect_type_symbol_occurrences(&function.0.ty, collector);
            }
        }
        AstNamedDefDetail::Struct { fields } => {
            for field in fields {
                collect_type_symbol_occurrences(&field.0.ty, collector);
            }
        }
        AstNamedDefDetail::Union { variants } => {
            for variant in variants {
                match &variant.0.payload {
                    AstUnionPayload::Type(ty) => collect_type_symbol_occurrences(ty, collector),
                    AstUnionPayload::Struct { fields } => {
                        for field in fields {
                            collect_type_symbol_occurrences(&field.0.ty, collector);
                        }
                    }
                }
            }
        }
        AstNamedDefDetail::Bitfield { fields } => {
            for field in fields {
                collect_type_symbol_occurrences(&field.0.high, collector);
                if let Some(low) = &field.0.low {
                    collect_type_symbol_occurrences(low, collector);
                }
            }
        }
    }
}

fn collect_symbol_occurrences_from_core_ast(
    ast: &CoreSourceFile,
    parsed: &ParsedFile,
) -> Vec<SymbolOccurrence> {
    let mut collector = SymbolOccurrenceCollector::new(parsed);

    for (item, _) in &ast.defs {
        match &item.kind {
            CoreDefinitionKind::Scattered(def) => {
                if let Some(params) = &def.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                    if let Some(signature) = &def.signature {
                        collect_type_symbol_occurrences(signature, &mut collector);
                    }
                    collector.pop_type_var_scope();
                } else if let Some(signature) = &def.signature {
                    collect_type_symbol_occurrences(signature, &mut collector);
                }
            }
            CoreDefinitionKind::ScatteredClause(def) => {
                if let Some(ty) = &def.ty {
                    collect_type_symbol_occurrences(ty, &mut collector);
                }
            }
            CoreDefinitionKind::CallableSpec(spec) => {
                collect_type_symbol_occurrences(&spec.signature, &mut collector);
            }
            CoreDefinitionKind::Callable(def) => {
                if let Some(rec_measure) = &def.rec_measure {
                    let mut bindings = Vec::new();
                    collect_pattern_symbol_occurrences(
                        &rec_measure.0.pattern,
                        &mut collector,
                        &mut bindings,
                    );
                    collector.push_value_scope();
                    for (name, span) in bindings {
                        collector.define_value(&name, span);
                    }
                    collect_expr_symbol_occurrences(&rec_measure.0.body, &mut collector);
                    collector.pop_value_scope();
                }

                if def.clauses.is_empty() {
                    collector.push_value_scope();
                    if let Some(signature) = &def.signature {
                        collect_type_symbol_occurrences(signature, &mut collector);
                    }
                    for param in &def.params {
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(param, &mut collector, &mut bindings);
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    if let Some(return_ty) = &def.return_ty {
                        collect_type_symbol_occurrences(return_ty, &mut collector);
                    }
                    if let Some(body) = &def.body {
                        collect_expr_symbol_occurrences(body, &mut collector);
                    }
                    if let Some(mapping_body) = &def.mapping_body {
                        collect_mapping_body_symbol_occurrences(mapping_body, &mut collector);
                    }
                    collector.pop_value_scope();
                    continue;
                }

                for clause in &def.clauses {
                    collector.push_value_scope();
                    let mut pushed_type_scope = false;
                    if let Some(quantifier) = &clause.0.quantifier {
                        collector.push_type_var_scope();
                        pushed_type_scope = true;
                        for var in &quantifier.vars {
                            collector.define_type_var(&var.name.0, var.name.1);
                        }
                        if let Some(constraint) = &quantifier.constraint {
                            collect_type_symbol_occurrences(constraint, &mut collector);
                        }
                    }
                    for pattern in &clause.0.patterns {
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(pattern, &mut collector, &mut bindings);
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    if let Some(return_ty) = &clause.0.return_ty {
                        collect_type_symbol_occurrences(return_ty, &mut collector);
                    }
                    if let Some(guard) = &clause.0.guard {
                        collect_expr_symbol_occurrences(guard, &mut collector);
                    }
                    if let Some(body) = &clause.0.body {
                        collect_expr_symbol_occurrences(body, &mut collector);
                    }
                    if let Some(mapping_body) = &clause.0.mapping_body {
                        collect_mapping_body_symbol_occurrences(mapping_body, &mut collector);
                    }
                    if pushed_type_scope {
                        collector.pop_type_var_scope();
                    }
                    collector.pop_value_scope();
                }
            }
            CoreDefinitionKind::TypeAlias(alias) => {
                if let Some(params) = &alias.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                    if let Some(target) = &alias.target {
                        collect_type_symbol_occurrences(target, &mut collector);
                    }
                    collector.pop_type_var_scope();
                } else if let Some(target) = &alias.target {
                    collect_type_symbol_occurrences(target, &mut collector);
                }
            }
            CoreDefinitionKind::Named(def) => {
                let has_type_params = def.params.is_some();
                if let Some(params) = &def.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                }
                if let Some(ty) = &def.ty {
                    collect_type_symbol_occurrences(ty, &mut collector);
                }
                if let Some(detail) = &def.detail {
                    collect_named_detail_symbol_occurrences(detail, &mut collector);
                }
                if matches!(def.kind, NamedDefKind::Overload) {
                    for member in &def.members {
                        collector.record_value_ref(&member.0, member.1);
                    }
                }
                if let Some(value) = &def.value {
                    collect_expr_symbol_occurrences(value, &mut collector);
                }
                if has_type_params {
                    collector.pop_type_var_scope();
                }
            }
            CoreDefinitionKind::Default(_)
            | CoreDefinitionKind::Fixity(_)
            | CoreDefinitionKind::Instantiation(_)
            | CoreDefinitionKind::Directive(_)
            | CoreDefinitionKind::End(_) => {}
            CoreDefinitionKind::Constraint(def) => {
                collect_type_symbol_occurrences(&def.ty, &mut collector);
            }
            CoreDefinitionKind::TerminationMeasure(def) => match &def.kind {
                AstTerminationMeasureKind::Function { pattern, body } => {
                    let mut bindings = Vec::new();
                    collect_pattern_symbol_occurrences(pattern, &mut collector, &mut bindings);
                    collector.push_value_scope();
                    for (name, span) in bindings {
                        collector.define_value(&name, span);
                    }
                    collect_expr_symbol_occurrences(body, &mut collector);
                    collector.pop_value_scope();
                }
                AstTerminationMeasureKind::Loop { measures } => {
                    for measure in measures {
                        match &measure.0 {
                            AstLoopMeasure::Until(expr)
                            | AstLoopMeasure::Repeat(expr)
                            | AstLoopMeasure::While(expr) => {
                                collect_expr_symbol_occurrences(expr, &mut collector)
                            }
                        }
                    }
                }
            },
        }
    }

    collector.occurrences
}

fn core_alias_target_name(alias: &CoreTypeAliasDef) -> Option<String> {
    match &unwrapped_type_expr(alias.target.as_ref()?).0 {
        AstTypeExpr::Named(name) => Some(name.clone()),
        AstTypeExpr::App { callee, .. } => Some(callee.0.clone()),
        _ => None,
    }
}

impl ParsedFile {
    #[cfg(test)]
    fn from_ast(ast: &SourceFile) -> Self {
        Self::from_core_ast(&crate::core_ast::SourceFile::from(ast))
    }

    pub fn from_core_ast(ast: &CoreSourceFile) -> Self {
        let mut parsed = ParsedFile::default();
        let pattern_constants = collect_top_level_pattern_constants_core(ast);

        for (item, item_span) in &ast.defs {
            match &item.kind {
                CoreDefinitionKind::Scattered(CoreScatteredDef { kind, name, .. }) => {
                    push_decl(
                        &mut parsed,
                        decl_kind_for_scattered(*kind),
                        DeclRole::Declaration,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        true,
                    );
                }
                CoreDefinitionKind::ScatteredClause(CoreScatteredClauseDef {
                    kind: ScatteredClauseKind::Enum,
                    member,
                    ..
                }) => {
                    push_decl(
                        &mut parsed,
                        DeclKind::EnumMember,
                        DeclRole::Definition,
                        &member.0,
                        Scope::TopLevel,
                        member.1,
                        true,
                    );
                }
                CoreDefinitionKind::ScatteredClause(_) => {}
                CoreDefinitionKind::CallableSpec(spec) => {
                    let kind = match spec.kind {
                        CallableSpecKind::Value => DeclKind::Value,
                        CallableSpecKind::Mapping => DeclKind::Mapping,
                    };
                    push_decl(
                        &mut parsed,
                        kind,
                        DeclRole::Declaration,
                        &spec.name.0,
                        Scope::TopLevel,
                        spec.name.1,
                        false,
                    );
                    parsed
                        .callable_heads
                        .push(callable_head_from_core_spec(spec, item_span.start));
                }
                CoreDefinitionKind::Callable(def) => {
                    let CoreCallableDef {
                        kind,
                        name,
                        signature,
                        params,
                        return_ty,
                        ..
                    } = def;
                    let (decl_kind, is_scattered) = match kind {
                        CallableDefKind::Function => (DeclKind::Function, false),
                        CallableDefKind::FunctionClause => (DeclKind::Function, true),
                        CallableDefKind::Mapping => (DeclKind::Mapping, false),
                        CallableDefKind::MappingClause => (DeclKind::Mapping, true),
                    };
                    push_decl(
                        &mut parsed,
                        decl_kind,
                        DeclRole::Definition,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        is_scattered,
                    );
                    parsed.callable_heads.push(CallableHead {
                        name: name.0.clone(),
                        kind: decl_kind,
                        name_span: name.1,
                        label_span: Span::new(item_span.start, callable_head_label_end_core(def)),
                        params: params.iter().map(callable_param_from_pattern).collect(),
                        return_type_span: return_ty.as_ref().map(|ty| ty.1),
                    });
                    if matches!(decl_kind, DeclKind::Mapping) {
                        if let Some(signature) = signature {
                            parsed.callable_heads.pop();
                            parsed.callable_heads.push(callable_head_from_signature(
                                &name.0,
                                decl_kind,
                                item_span.start,
                                name.1,
                                signature,
                            ));
                        }
                    }
                    collect_core_callable_clause_metadata(
                        def,
                        &name.0,
                        &pattern_constants,
                        &mut parsed,
                    );
                }
                CoreDefinitionKind::TypeAlias(alias) => {
                    push_decl(
                        &mut parsed,
                        DeclKind::Type,
                        DeclRole::Definition,
                        &alias.name.0,
                        Scope::TopLevel,
                        alias.name.1,
                        false,
                    );
                    if let Some(target) = core_alias_target_name(alias) {
                        parsed.type_aliases.push(TypeAlias {
                            sub: alias.name.0.clone(),
                            sup: target,
                            span: alias.name.1,
                        });
                    }
                }
                CoreDefinitionKind::Named(CoreNamedDef {
                    kind,
                    name,
                    members,
                    ty,
                    value,
                    ..
                }) => {
                    let decl_kind = decl_kind_for_named(*kind);
                    push_decl(
                        &mut parsed,
                        decl_kind,
                        DeclRole::Definition,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        false,
                    );
                    if *kind == NamedDefKind::Enum {
                        for member in members {
                            push_decl(
                                &mut parsed,
                                DeclKind::EnumMember,
                                DeclRole::Definition,
                                &member.0,
                                Scope::TopLevel,
                                member.1,
                                false,
                            );
                        }
                    }
                    if matches!(kind, NamedDefKind::Let | NamedDefKind::Var) {
                        if let Some(ty) = ty {
                            parsed.typed_bindings.push(TypedBinding {
                                name: name.0.clone(),
                                name_span: name.1,
                                ty_span: ty.1,
                                scope: Scope::TopLevel,
                            });
                        }
                    }
                    if let Some(value) = value {
                        collect_expr_metadata(value, None, &pattern_constants, &mut parsed);
                    }
                }
                CoreDefinitionKind::Default(_)
                | CoreDefinitionKind::Fixity(_)
                | CoreDefinitionKind::Instantiation(_)
                | CoreDefinitionKind::Directive(_)
                | CoreDefinitionKind::End(_)
                | CoreDefinitionKind::Constraint(_) => {}
                CoreDefinitionKind::TerminationMeasure(CoreTerminationMeasureDef {
                    kind, ..
                }) => {
                    collect_termination_measure_metadata(kind, &pattern_constants, &mut parsed);
                }
            }
        }

        parsed.symbol_occurrences = collect_symbol_occurrences_from_core_ast(ast, &parsed);
        parsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chumsky::Parser;

    #[test]
    fn parses_top_level_and_local_bindings_with_scope() {
        let source = r#"
function foo() = {
  let x = 1;
}
let y = 2
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);
        assert!(parsed.decls.iter().any(|d| d.name == "foo"
            && d.kind == DeclKind::Function
            && d.role == DeclRole::Definition
            && d.scope == Scope::TopLevel));
        assert!(parsed.decls.iter().any(|d| d.name == "x"
            && d.kind == DeclKind::Let
            && d.role == DeclRole::Definition
            && d.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|d| d.name == "y"
            && d.kind == DeclKind::Let
            && d.role == DeclRole::Definition
            && d.scope == Scope::TopLevel));
    }

    #[test]
    fn parses_enum_members() {
        let source = "enum color = Red | Green | Blue";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);
        assert!(parsed
            .decls
            .iter()
            .any(|d| d.name == "color" && d.kind == DeclKind::Enum));
        assert!(parsed
            .decls
            .iter()
            .any(|d| d.name == "Red" && d.kind == DeclKind::EnumMember));
    }

    #[test]
    fn parses_type_alias_edges() {
        let source = "type child = parent\ntype plain = bits";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);
        assert!(parsed
            .type_aliases
            .iter()
            .any(|a| a.sub == "child" && a.sup == "parent"));
    }

    #[test]
    fn parses_scattered_and_newtype_declarations() {
        let source = r#"
scattered union my_u
scattered enum my_e
newtype register_index = Mk_index : bits(5)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);
        assert!(parsed.decls.iter().any(|d| d.name == "my_u"
            && d.kind == DeclKind::Union
            && d.role == DeclRole::Declaration));
        assert!(parsed.decls.iter().any(|d| d.name == "my_e"
            && d.kind == DeclKind::Enum
            && d.role == DeclRole::Declaration));
        assert!(parsed.decls.iter().any(|d| d.name == "register_index"
            && d.kind == DeclKind::Newtype
            && d.role == DeclRole::Definition));
    }

    #[test]
    fn parses_scattered_function_head_and_clause_roles() {
        let source = r#"
scattered function foo
function clause foo(x) = x
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo" && d.kind == DeclKind::Function && d.role == DeclRole::Declaration
        }));
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo" && d.kind == DeclKind::Function && d.role == DeclRole::Definition
        }));
    }

    #[test]
    fn derives_top_level_metadata_from_ast() {
        let source = r#"
val foo : (int, int) -> int
function foo(x : bits(32), y : int) -> int = x
type child = parent
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo"
                && d.kind == DeclKind::Value
                && d.role == DeclRole::Declaration
                && d.scope == Scope::TopLevel
        }));
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo"
                && d.kind == DeclKind::Function
                && d.role == DeclRole::Definition
                && d.scope == Scope::TopLevel
        }));
        assert!(parsed
            .type_aliases
            .iter()
            .any(|alias| alias.sub == "child" && alias.sup == "parent"));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "x"));
        assert!(parsed
            .callable_heads
            .iter()
            .any(|head| head.name == "foo" && head.kind == DeclKind::Function));
    }

    #[test]
    fn derives_callable_heads_from_quantified_effectful_specs() {
        let source = r#"
val write_ram : forall 'n, 0 < 'n <= max_mem_access . (write_kind, xlenbits, atom('n), bits(8 * 'n), mem_meta) -> bool effect {wmv, wmvt}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let head = parsed
            .callable_heads
            .iter()
            .find(|head| head.name == "write_ram")
            .expect("callable head");
        assert_eq!(head.kind, DeclKind::Value);
        assert_eq!(head.params.len(), 5, "{head:#?}");
        assert!(head.return_type_span.is_some());
    }

    #[test]
    fn derives_calls_from_termination_measure_defs() {
        let source = r#"
termination_measure helper x = call(x)
termination_measure loop until done(x), while guard(x)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.call_sites.iter().any(|call| call.callee == "call"));
        assert!(parsed.call_sites.iter().any(|call| call.callee == "done"));
        assert!(parsed.call_sites.iter().any(|call| call.callee == "guard"));
    }

    #[test]
    fn derives_enum_members_from_ast() {
        let source = r#"
enum color = Red | Green
scattered enum extension
enum clause extension = Ext_M
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "Red" && decl.kind == DeclKind::EnumMember));
        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "Green" && decl.kind == DeclKind::EnumMember));
        assert!(parsed.decls.iter().any(|decl| decl.name == "Ext_M"
            && decl.kind == DeclKind::EnumMember
            && decl.is_scattered));
    }

    #[test]
    fn derives_as_pattern_bindings_from_ast() {
        let source = r#"
function match_cons_as(ys) = match ys {
  x :: xs as zs => zs,
  [||] => ys
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|decl| decl.name == "x"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|decl| decl.name == "xs"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|decl| decl.name == "zs"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
    }

    #[test]
    fn derives_body_metadata_from_ast() {
        let source = r#"
function foo(x : int) = {
  let y : bits(8) = bar(x);
  var z = baz(y);
  match z {
    Some(v) => qux(v),
    None() => y
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|decl| {
            decl.name == "y"
                && decl.kind == DeclKind::Let
                && decl.scope == Scope::Local
                && decl.role == DeclRole::Definition
        }));
        assert!(parsed.decls.iter().any(|decl| {
            decl.name == "z"
                && decl.kind == DeclKind::Var
                && decl.scope == Scope::Local
                && decl.role == DeclRole::Definition
        }));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "x" && binding.scope == Scope::Local));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "y" && binding.scope == Scope::Local));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "bar" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "baz" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "qux" }));
    }

    #[test]
    fn derives_top_level_initializer_calls_from_ast() {
        let source = r#"
register r : int = mk_reg(init())
let result : int = wrap(read_reg(r))
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "mk_reg" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "init" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "wrap" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "read_reg" }));
    }

    #[test]
    fn derives_mapping_body_calls_from_ast() {
        let source = r#"mapping clause assembly = use_bits(0x12) <-> "ok""#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.call_sites.iter().any(|call| {
            call.caller.as_deref() == Some("assembly") && call.callee == "use_bits"
        }));
    }

    #[test]
    fn derives_mapping_arm_calls_from_structured_ast() {
        let source = r#"
mapping encdec_reg : regidx <-> bits(5) = {
  forwards Regidx(r) => zero_extend(r),
  backwards r if allow_reg(r) => decode_reg(r)
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let callees = parsed
            .call_sites
            .iter()
            .filter(|call| call.caller.as_deref() == Some("encdec_reg"))
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();
        assert!(callees.contains(&"zero_extend"));
        assert!(callees.contains(&"allow_reg"));
        assert!(callees.contains(&"decode_reg"));
    }

    #[test]
    fn derives_metadata_from_function_and_clauses_and_rec_measures() {
        let source = r#"
function { xs => dec(xs) } foo forall 'n. (x if guard(x)) -> bits('n) = body(x)
and foo y = other(y)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::full_parser::parse_source(&tokens)
            .into_result()
            .unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let callees = parsed
            .call_sites
            .iter()
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();
        assert!(callees.contains(&"dec"));
        assert!(callees.contains(&"guard"));
        assert!(callees.contains(&"body"));
        assert!(callees.contains(&"other"));
        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "x" && decl.kind == DeclKind::Parameter));
    }
}
