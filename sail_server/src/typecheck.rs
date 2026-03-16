use std::collections::{HashMap, HashSet};

use crate::diagnostics::reporting::{diagnostic_for_error, Error as ReportingError};
use crate::diagnostics::type_error::TypeError;
use crate::diagnostics::{Diagnostic, DiagnosticCode};
use crate::state::File;
use crate::symbols::collect_callable_signatures;
use sail_parser::{
    core_ast::{
        CallableClause, CallableDefKind, DefinitionKind, Expr, FieldPattern, MatchCase,
        NamedDefDetail, NamedDefKind, Pattern, SourceFile, Spanned, TypeExpr,
    },
    Literal, Span,
};

type SpanKey = (usize, usize);

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ty {
    Unknown,
    Text(String),
    Var(String),
    Tuple(Vec<Ty>),
    Function {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },
    App {
        name: String,
        args: Vec<TyArg>,
        text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum TyArg {
    Type(Box<Ty>),
    Value(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct TypeScheme {
    quantifiers: Vec<String>,
    params: Vec<Ty>,
    implicit_params: Vec<bool>,
    ret: Ty,
}

#[derive(Clone, Debug, Default)]
struct TopLevelEnv {
    functions: HashMap<String, Vec<TypeScheme>>,
    overloads: HashMap<String, Vec<String>>,
    values: HashMap<String, Ty>,
    records: HashMap<String, HashMap<String, Ty>>,
}

#[derive(Clone, Debug, Default)]
struct LocalEnv {
    scopes: Vec<HashMap<String, Ty>>,
    expected_return: Option<Ty>,
}

#[derive(Clone, Debug, Default)]
struct Subst {
    types: HashMap<String, Ty>,
    values: HashMap<String, String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct TypeCheckResult {
    diagnostics: Vec<Diagnostic>,
    expr_types: HashMap<SpanKey, String>,
    binding_types: HashMap<SpanKey, String>,
}

impl TypeCheckResult {
    pub(crate) fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub(crate) fn expr_type_text(&self, span: Span) -> Option<&str> {
        self.expr_types
            .get(&(span.start, span.end))
            .map(String::as_str)
    }

    pub(crate) fn binding_type_text(&self, span: Span) -> Option<&str> {
        self.binding_types
            .get(&(span.start, span.end))
            .map(String::as_str)
    }
}

impl Ty {
    fn text(&self) -> String {
        match self {
            Ty::Unknown => "_".to_string(),
            Ty::Text(text) => text.clone(),
            Ty::Var(name) => name.clone(),
            Ty::Tuple(items) => format!(
                "({})",
                items.iter().map(Ty::text).collect::<Vec<_>>().join(", ")
            ),
            Ty::Function { params, ret } => {
                let params = if params.len() == 1 {
                    params[0].text()
                } else {
                    format!(
                        "({})",
                        params.iter().map(Ty::text).collect::<Vec<_>>().join(", ")
                    )
                };
                format!("{params} -> {}", ret.text())
            }
            Ty::App { text, .. } => text.clone(),
        }
    }

    fn is_unknown(&self) -> bool {
        matches!(self, Ty::Unknown)
    }
}

impl LocalEnv {
    fn new(expected_return: Option<Ty>) -> Self {
        Self {
            scopes: vec![HashMap::new()],
            expected_return,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn define(&mut self, name: &str, ty: Ty) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.insert(name.to_string(), ty);
        }
    }

    fn lookup(&self, name: &str) -> Option<Ty> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }
}

fn span_text(source: &str, span: Span) -> String {
    source[span.start..span.end].trim().to_string()
}

fn type_arg_from_type_expr(source: &str, ty: &Spanned<TypeExpr>) -> TyArg {
    match &ty.0 {
        TypeExpr::Named(_)
        | TypeExpr::TypeVar(_)
        | TypeExpr::App { .. }
        | TypeExpr::Tuple(_)
        | TypeExpr::Arrow { .. }
        | TypeExpr::Register(_)
        | TypeExpr::Effect { .. }
        | TypeExpr::Forall { .. }
        | TypeExpr::Existential { .. } => TyArg::Type(Box::new(type_from_type_expr(source, ty))),
        _ => TyArg::Value(span_text(source, ty.1)),
    }
}

fn type_from_type_expr(source: &str, ty: &Spanned<TypeExpr>) -> Ty {
    match &ty.0 {
        TypeExpr::Named(name) => Ty::Text(name.clone()),
        TypeExpr::TypeVar(name) => Ty::Var(name.clone()),
        TypeExpr::Tuple(items) => Ty::Tuple(
            items
                .iter()
                .map(|item| type_from_type_expr(source, item))
                .collect(),
        ),
        TypeExpr::Arrow { params, ret, .. } => Ty::Function {
            params: params
                .iter()
                .map(|item| type_from_type_expr(source, item))
                .collect(),
            ret: Box::new(type_from_type_expr(source, ret)),
        },
        TypeExpr::App { callee, args } => Ty::App {
            name: callee.0.clone(),
            args: args
                .iter()
                .map(|arg| type_arg_from_type_expr(source, arg))
                .collect(),
            text: span_text(source, ty.1),
        },
        TypeExpr::Register(inner) => Ty::App {
            name: "register".to_string(),
            args: vec![TyArg::Type(Box::new(type_from_type_expr(source, inner)))],
            text: span_text(source, ty.1),
        },
        TypeExpr::Effect { ty: inner, .. } => type_from_type_expr(source, inner),
        TypeExpr::Forall { body, .. } => type_from_type_expr(source, body),
        TypeExpr::Existential { body, .. } => type_from_type_expr(source, body),
        _ => Ty::Text(span_text(source, ty.1)),
    }
}

fn scheme_from_type_expr(source: &str, ty: &Spanned<TypeExpr>) -> TypeScheme {
    let mut quantifiers = Vec::new();
    let mut current = ty;
    loop {
        match &current.0 {
            TypeExpr::Forall { vars, body, .. } => {
                quantifiers.extend(vars.iter().map(|var| var.name.0.clone()));
                current = body;
            }
            TypeExpr::Effect { ty, .. } => current = ty,
            _ => break,
        }
    }
    match type_from_type_expr(source, current) {
        Ty::Function { params, ret } => {
            let implicit_params = vec![false; params.len()];
            TypeScheme {
                quantifiers,
                params,
                implicit_params,
                ret: *ret,
            }
        }
        ret => TypeScheme {
            quantifiers,
            params: Vec::new(),
            implicit_params: Vec::new(),
            ret,
        },
    }
}

fn is_pattern_binding(name: &str, pattern_constants: &HashSet<String>) -> bool {
    !pattern_constants.contains(name)
}

impl TopLevelEnv {
    fn from_ast(source: &str, ast: &SourceFile) -> (Self, HashSet<String>) {
        let mut env = Self::default();
        let mut pattern_constants = HashSet::new();

        for (item, _) in &ast.defs {
            match &item.kind {
                DefinitionKind::CallableSpec(spec) => {
                    env.functions
                        .entry(spec.name.0.clone())
                        .or_default()
                        .push(scheme_from_type_expr(source, &spec.signature));
                }
                DefinitionKind::Callable(def) => {
                    if let Some(signature) = &def.signature {
                        env.functions
                            .entry(def.name.0.clone())
                            .or_default()
                            .push(scheme_from_type_expr(source, signature));
                    } else if !env.functions.contains_key(&def.name.0) {
                        if let Some(scheme) = scheme_from_callable_head(source, def) {
                            env.functions
                                .entry(def.name.0.clone())
                                .or_default()
                                .push(scheme);
                        }
                    }
                }
                DefinitionKind::Named(def) => match def.kind {
                    NamedDefKind::Overload => {
                        env.overloads.insert(
                            def.name.0.clone(),
                            def.members.iter().map(|member| member.0.clone()).collect(),
                        );
                    }
                    NamedDefKind::Struct => {
                        if let Some(NamedDefDetail::Struct { fields }) = &def.detail {
                            env.records.insert(
                                def.name.0.clone(),
                                fields
                                    .iter()
                                    .map(|field| {
                                        (
                                            field.0.name.0.clone(),
                                            type_from_type_expr(source, &field.0.ty),
                                        )
                                    })
                                    .collect(),
                            );
                        }
                    }
                    NamedDefKind::Enum => {
                        for member in &def.members {
                            pattern_constants.insert(member.0.clone());
                        }
                    }
                    NamedDefKind::Let | NamedDefKind::Var => {
                        if let Some(ty) = &def.ty {
                            env.values
                                .insert(def.name.0.clone(), type_from_type_expr(source, ty));
                        }
                    }
                    _ => {}
                },
                DefinitionKind::ScatteredClause(def)
                    if matches!(def.kind, sail_parser::ScatteredClauseKind::Enum) =>
                {
                    pattern_constants.insert(def.member.0.clone());
                }
                _ => {}
            }
        }

        (env, pattern_constants)
    }

    fn lookup_value(&self, locals: &LocalEnv, name: &str) -> Option<Ty> {
        locals
            .lookup(name)
            .or_else(|| self.values.get(name).cloned())
            .or_else(|| {
                let schemes = self.functions.get(name)?;
                if schemes.len() == 1 {
                    Some(Ty::Function {
                        params: schemes[0].params.clone(),
                        ret: Box::new(schemes[0].ret.clone()),
                    })
                } else {
                    None
                }
            })
    }

    fn lookup_functions(&self, name: &str) -> Vec<TypeScheme> {
        if let Some(members) = self.overloads.get(name) {
            let mut out = Vec::new();
            for member in members {
                if let Some(schemes) = self.functions.get(member) {
                    out.extend(schemes.iter().cloned());
                }
            }
            out
        } else {
            self.functions.get(name).cloned().unwrap_or_default()
        }
    }
}

fn scheme_from_callable_head(
    source: &str,
    def: &sail_parser::core_ast::CallableDefinition,
) -> Option<TypeScheme> {
    if def.params.is_empty() && def.return_ty.is_none() {
        return None;
    }

    let params = def
        .params
        .iter()
        .map(|param| pattern_annotation_type(source, param).unwrap_or(Ty::Unknown))
        .collect::<Vec<_>>();
    let ret = def
        .return_ty
        .as_ref()
        .map(|ty| type_from_type_expr(source, ty))
        .unwrap_or(Ty::Unknown);

    Some(TypeScheme {
        quantifiers: Vec::new(),
        params,
        implicit_params: vec![false; def.params.len()],
        ret,
    })
}

fn apply_callable_signature_metadata(file: &File, env: &mut TopLevelEnv) {
    let mut best_signatures =
        HashMap::<(String, usize), (usize, Vec<bool>, Vec<Ty>, Option<String>)>::new();

    for signature in collect_callable_signatures(file) {
        let implicit_params = signature
            .params
            .iter()
            .map(|param| param.is_implicit)
            .collect::<Vec<_>>();
        let signature_params = signature
            .params
            .iter()
            .map(|param| {
                param
                    .name
                    .split_once(':')
                    .map(|(_, ty)| Ty::Text(ty.trim().to_string()))
                    .unwrap_or(Ty::Unknown)
            })
            .collect::<Vec<_>>();
        let score = signature
            .params
            .iter()
            .filter(|param| param.is_implicit || param.name.contains(':'))
            .count()
            + usize::from(signature.return_type.is_some());
        let key = (signature.name.clone(), signature.params.len());
        match best_signatures.get(&key) {
            Some((best_score, _, _, _)) if *best_score >= score => {}
            _ => {
                best_signatures.insert(
                    key,
                    (
                        score,
                        implicit_params,
                        signature_params,
                        signature.return_type,
                    ),
                );
            }
        }
    }

    for ((name, _), (_, implicit_params, signature_params, return_type)) in best_signatures {
        let Some(schemes) = env.functions.get_mut(&name) else {
            continue;
        };
        let mut matched = false;
        if let Some(scheme) = schemes.iter_mut().find(|scheme| {
            scheme.params.len() == implicit_params.len()
                || matches!(
                    scheme.params.as_slice(),
                    [Ty::Tuple(items)] if items.len() == implicit_params.len()
                )
        }) {
            if let [Ty::Tuple(items)] = scheme.params.as_slice() {
                if items.len() == implicit_params.len() {
                    scheme.params = items.clone();
                }
            }
            scheme.implicit_params = implicit_params.clone();
            if scheme.ret.is_unknown() {
                if let Some(ret) = return_type.as_deref() {
                    scheme.ret = Ty::Text(ret.to_string());
                }
            }
            matched = true;
        }

        if !matched && schemes.len() == 1 {
            let scheme = &mut schemes[0];
            scheme.params = signature_params;
            scheme.implicit_params = implicit_params;
            if scheme.ret.is_unknown() {
                if let Some(ret) = return_type.as_deref() {
                    scheme.ret = Ty::Text(ret.to_string());
                }
            }
        }
    }
}

fn pattern_annotation_type(source: &str, pattern: &Spanned<Pattern>) -> Option<Ty> {
    match &pattern.0 {
        Pattern::Typed(_, ty) | Pattern::AsType(_, ty) => Some(type_from_type_expr(source, ty)),
        Pattern::Attribute { pattern, .. } => pattern_annotation_type(source, pattern),
        Pattern::AsBinding { pattern, .. } => pattern_annotation_type(source, pattern),
        _ => None,
    }
}

fn infer_literal_type(literal: &Literal) -> Ty {
    match literal {
        Literal::Bool(_) => Ty::Text("bool".to_string()),
        Literal::Unit => Ty::Text("unit".to_string()),
        Literal::Number(text) => {
            if text.contains('.') {
                Ty::Text("real".to_string())
            } else {
                Ty::Text("int".to_string())
            }
        }
        Literal::Binary(text) => Ty::App {
            name: "bits".to_string(),
            args: vec![TyArg::Value(
                text.trim_start_matches("0b")
                    .chars()
                    .filter(|ch| *ch != '_')
                    .count()
                    .to_string(),
            )],
            text: format!(
                "bits({})",
                text.trim_start_matches("0b")
                    .chars()
                    .filter(|ch| *ch != '_')
                    .count()
            ),
        },
        Literal::Hex(text) => Ty::App {
            name: "bits".to_string(),
            args: vec![TyArg::Value(
                (text
                    .trim_start_matches("0x")
                    .chars()
                    .filter(|ch| *ch != '_')
                    .count()
                    * 4)
                .to_string(),
            )],
            text: format!(
                "bits({})",
                text.trim_start_matches("0x")
                    .chars()
                    .filter(|ch| *ch != '_')
                    .count()
                    * 4
            ),
        },
        Literal::String(_) => Ty::Text("string".to_string()),
        Literal::BitZero | Literal::BitOne => Ty::Text("bit".to_string()),
        Literal::Undefined => Ty::Unknown,
    }
}

fn unify_value(expected: &str, actual: &str, subst: &mut Subst) -> bool {
    if expected.starts_with('\'') {
        match subst.values.get(expected) {
            Some(bound) => bound == actual,
            None => {
                subst
                    .values
                    .insert(expected.to_string(), actual.to_string());
                true
            }
        }
    } else {
        expected == actual
    }
}

fn apply_subst(ty: &Ty, subst: &Subst) -> Ty {
    match ty {
        Ty::Unknown => Ty::Unknown,
        Ty::Text(text) => Ty::Text(text.clone()),
        Ty::Var(name) => subst
            .types
            .get(name)
            .cloned()
            .unwrap_or_else(|| Ty::Var(name.clone())),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(|item| apply_subst(item, subst)).collect()),
        Ty::Function { params, ret } => Ty::Function {
            params: params
                .iter()
                .map(|param| apply_subst(param, subst))
                .collect(),
            ret: Box::new(apply_subst(ret, subst)),
        },
        Ty::App { name, args, text } => Ty::App {
            name: name.clone(),
            args: args
                .iter()
                .map(|arg| match arg {
                    TyArg::Type(ty) => TyArg::Type(Box::new(apply_subst(ty, subst))),
                    TyArg::Value(value) => TyArg::Value(
                        subst
                            .values
                            .get(value)
                            .cloned()
                            .unwrap_or_else(|| value.clone()),
                    ),
                })
                .collect(),
            text: text.clone(),
        },
    }
}

fn unify(expected: &Ty, actual: &Ty, subst: &mut Subst) -> bool {
    match expected {
        Ty::Unknown => true,
        Ty::Var(name) => match subst.types.get(name).cloned() {
            Some(bound) => unify(&bound, actual, subst),
            None => {
                subst.types.insert(name.clone(), actual.clone());
                true
            }
        },
        Ty::Text(expected) => expected == &actual.text(),
        Ty::Tuple(expected_items) => match actual {
            Ty::Tuple(actual_items) if expected_items.len() == actual_items.len() => expected_items
                .iter()
                .zip(actual_items.iter())
                .all(|(expected, actual)| unify(expected, actual, subst)),
            _ => false,
        },
        Ty::Function {
            params: expected_params,
            ret: expected_ret,
        } => match actual {
            Ty::Function {
                params: actual_params,
                ret: actual_ret,
            } if expected_params.len() == actual_params.len() => {
                expected_params
                    .iter()
                    .zip(actual_params.iter())
                    .all(|(expected, actual)| unify(expected, actual, subst))
                    && unify(expected_ret, actual_ret, subst)
            }
            _ => false,
        },
        Ty::App {
            name: expected_name,
            args: expected_args,
            ..
        } => match actual {
            Ty::App {
                name: actual_name,
                args: actual_args,
                ..
            } if expected_name == actual_name && expected_args.len() == actual_args.len() => {
                expected_args
                    .iter()
                    .zip(actual_args.iter())
                    .all(|(expected, actual)| match (expected, actual) {
                        (TyArg::Type(expected), TyArg::Type(actual)) => {
                            unify(expected, actual, subst)
                        }
                        (TyArg::Value(expected), TyArg::Value(actual)) => {
                            unify_value(expected, actual, subst)
                        }
                        (TyArg::Value(expected), TyArg::Type(actual)) => {
                            unify_value(expected, &actual.text(), subst)
                        }
                        (TyArg::Type(expected), TyArg::Value(actual)) => {
                            unify(expected, &Ty::Text(actual.clone()), subst)
                        }
                    })
            }
            _ => false,
        },
    }
}

struct Checker<'a> {
    file: &'a File,
    source: &'a str,
    env: TopLevelEnv,
    pattern_constants: HashSet<String>,
    diagnostics: Vec<Diagnostic>,
    expr_types: HashMap<SpanKey, String>,
    binding_types: HashMap<SpanKey, String>,
    seen_errors: HashSet<(DiagnosticCode, usize, usize, String)>,
}

impl<'a> Checker<'a> {
    fn new(file: &'a File, env: TopLevelEnv, pattern_constants: HashSet<String>) -> Self {
        Self {
            file,
            source: file.source.text(),
            env,
            pattern_constants,
            diagnostics: Vec::new(),
            expr_types: HashMap::new(),
            binding_types: HashMap::new(),
            seen_errors: HashSet::new(),
        }
    }

    fn finish(self) -> TypeCheckResult {
        TypeCheckResult {
            diagnostics: self.diagnostics,
            expr_types: self.expr_types,
            binding_types: self.binding_types,
        }
    }

    fn push_error(&mut self, code: DiagnosticCode, span: Span, error: TypeError) {
        let key = (code.clone(), span.start, span.end, format!("{error:?}"));
        if !self.seen_errors.insert(key) {
            return;
        }
        self.diagnostics.push(diagnostic_for_error(
            self.file,
            code,
            ReportingError::Type { span, error },
        ));
    }

    fn record_expr_type(&mut self, span: Span, ty: &Ty) {
        if !ty.is_unknown() {
            self.expr_types.insert((span.start, span.end), ty.text());
        }
    }

    fn record_binding_type(&mut self, span: Span, ty: &Ty) {
        if !ty.is_unknown() {
            self.binding_types.insert((span.start, span.end), ty.text());
        }
    }

    fn check_source_file(mut self, ast: &SourceFile) -> TypeCheckResult {
        for (item, _) in &ast.defs {
            match &item.kind {
                DefinitionKind::Callable(def) => self.check_callable_definition(def),
                DefinitionKind::Named(def)
                    if matches!(def.kind, NamedDefKind::Let | NamedDefKind::Var) =>
                {
                    self.check_named_binding(def);
                }
                _ => {}
            }
        }
        self.finish()
    }

    fn check_named_binding(&mut self, def: &sail_parser::core_ast::NamedDefinition) {
        let Some(value) = &def.value else {
            return;
        };
        let mut locals = LocalEnv::new(
            def.ty
                .as_ref()
                .map(|ty| type_from_type_expr(self.source, ty)),
        );
        let value_ty = self.infer_expr(value, &mut locals);
        if let Some(expected) = &def.ty {
            let expected_ty = type_from_type_expr(self.source, expected);
            let mut subst = Subst::default();
            if !unify(&expected_ty, &value_ty, &mut subst) {
                self.push_error(
                    DiagnosticCode::TypeError,
                    value.1,
                    TypeError::Subtype {
                        lhs: value_ty.text(),
                        rhs: expected_ty.text(),
                        constraint: None,
                    },
                );
            } else {
                self.record_binding_type(def.name.1, &apply_subst(&expected_ty, &subst));
            }
        } else {
            self.record_binding_type(def.name.1, &value_ty);
        }
    }

    fn check_callable_definition(&mut self, def: &sail_parser::core_ast::CallableDefinition) {
        if !matches!(
            def.kind,
            CallableDefKind::Function | CallableDefKind::FunctionClause
        ) {
            return;
        }

        let expected_scheme = self
            .env
            .functions
            .get(&def.name.0)
            .and_then(|schemes| schemes.first().cloned())
            .or_else(|| {
                def.signature
                    .as_ref()
                    .map(|ty| scheme_from_type_expr(self.source, ty))
            })
            .or_else(|| scheme_from_callable_head(self.source, def));

        if def.clauses.is_empty() {
            let mut locals =
                LocalEnv::new(expected_scheme.as_ref().map(|scheme| scheme.ret.clone()));
            if let Some(scheme) = &expected_scheme {
                for (param, expected_ty) in def.params.iter().zip(scheme.params.iter()) {
                    self.bind_pattern(param, Some(expected_ty.clone()), &mut locals);
                }
            } else {
                for param in &def.params {
                    self.bind_pattern(param, None, &mut locals);
                }
            }
            if let Some(body) = &def.body {
                let body_ty = self.infer_expr(body, &mut locals);
                if let Some(expected) = expected_scheme.as_ref().map(|scheme| scheme.ret.clone()) {
                    let mut subst = Subst::default();
                    if !unify(&expected, &body_ty, &mut subst) {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            body.1,
                            TypeError::Subtype {
                                lhs: body_ty.text(),
                                rhs: expected.text(),
                                constraint: None,
                            },
                        );
                    }
                }
            }
            return;
        }

        for clause in &def.clauses {
            self.check_callable_clause(clause, expected_scheme.as_ref());
        }
    }

    fn check_callable_clause(
        &mut self,
        clause: &Spanned<CallableClause>,
        expected_scheme: Option<&TypeScheme>,
    ) {
        let mut locals = LocalEnv::new(expected_scheme.map(|scheme| scheme.ret.clone()));
        if let Some(scheme) = expected_scheme {
            for (pattern, expected_ty) in clause.0.patterns.iter().zip(scheme.params.iter()) {
                self.bind_pattern(pattern, Some(expected_ty.clone()), &mut locals);
            }
        } else {
            for pattern in &clause.0.patterns {
                self.bind_pattern(pattern, None, &mut locals);
            }
        }
        if let Some(guard) = &clause.0.guard {
            let guard_ty = self.infer_expr(guard, &mut locals);
            let mut subst = Subst::default();
            if !unify(&Ty::Text("bool".to_string()), &guard_ty, &mut subst) {
                self.push_error(
                    DiagnosticCode::TypeError,
                    guard.1,
                    TypeError::Subtype {
                        lhs: guard_ty.text(),
                        rhs: "bool".to_string(),
                        constraint: None,
                    },
                );
            }
        }
        if let Some(body) = &clause.0.body {
            let body_ty = self.infer_expr(body, &mut locals);
            if let Some(expected) = expected_scheme.map(|scheme| scheme.ret.clone()) {
                let mut subst = Subst::default();
                if !unify(&expected, &body_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        body.1,
                        TypeError::Subtype {
                            lhs: body_ty.text(),
                            rhs: expected.text(),
                            constraint: None,
                        },
                    );
                }
            }
        }
    }

    fn bind_pattern(
        &mut self,
        pattern: &Spanned<Pattern>,
        expected_ty: Option<Ty>,
        locals: &mut LocalEnv,
    ) {
        match &pattern.0 {
            Pattern::Attribute { pattern, .. } => self.bind_pattern(pattern, expected_ty, locals),
            Pattern::Ident(name) => {
                if is_pattern_binding(name, &self.pattern_constants) {
                    let ty = expected_ty.unwrap_or(Ty::Unknown);
                    locals.define(name, ty.clone());
                    self.record_binding_type(pattern.1, &ty);
                }
            }
            Pattern::Typed(inner, ty) | Pattern::AsType(inner, ty) => {
                let annotated = type_from_type_expr(self.source, ty);
                if let Some(expected) = expected_ty {
                    let mut subst = Subst::default();
                    if !unify(&annotated, &expected, &mut subst) {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            ty.1,
                            TypeError::Subtype {
                                lhs: expected.text(),
                                rhs: annotated.text(),
                                constraint: None,
                            },
                        );
                    }
                }
                self.bind_pattern(inner, Some(annotated), locals);
            }
            Pattern::AsBinding {
                pattern: inner,
                binding,
            } => {
                self.bind_pattern(inner, expected_ty.clone(), locals);
                let ty = expected_ty.unwrap_or(Ty::Unknown);
                locals.define(&binding.0, ty.clone());
                self.record_binding_type(binding.1, &ty);
            }
            Pattern::Tuple(items) => {
                let tuple_items = match expected_ty {
                    Some(Ty::Tuple(expected_items)) if expected_items.len() == items.len() => {
                        Some(expected_items)
                    }
                    _ => None,
                };
                for (index, item) in items.iter().enumerate() {
                    self.bind_pattern(
                        item,
                        tuple_items
                            .as_ref()
                            .and_then(|items| items.get(index).cloned()),
                        locals,
                    );
                }
            }
            Pattern::Struct { fields, name } => {
                let record_name = name.as_ref().map(|name| name.0.clone()).or_else(|| {
                    match expected_ty.as_ref() {
                        Some(Ty::Text(name)) => Some(name.clone()),
                        Some(Ty::App { name, .. }) => Some(name.clone()),
                        _ => None,
                    }
                });
                let record_fields = record_name
                    .as_ref()
                    .and_then(|name| self.env.records.get(name))
                    .cloned();
                for field in fields {
                    if let FieldPattern::Binding { name, pattern } = &field.0 {
                        let expected = record_fields
                            .as_ref()
                            .and_then(|fields| fields.get(&name.0))
                            .cloned();
                        self.bind_pattern(pattern, expected, locals);
                    }
                }
            }
            Pattern::List(items) | Pattern::Vector(items) => {
                for item in items {
                    self.bind_pattern(item, None, locals);
                }
            }
            Pattern::App { args, .. } => {
                for arg in args {
                    self.bind_pattern(arg, None, locals);
                }
            }
            Pattern::Infix { lhs, rhs, .. } => {
                self.bind_pattern(lhs, None, locals);
                self.bind_pattern(rhs, None, locals);
            }
            Pattern::Wild
            | Pattern::Literal(_)
            | Pattern::TypeVar(_)
            | Pattern::Index { .. }
            | Pattern::RangeIndex { .. }
            | Pattern::Error(_) => {}
        }
    }

    fn infer_expr(&mut self, expr: &Spanned<Expr>, locals: &mut LocalEnv) -> Ty {
        let ty = match &expr.0 {
            Expr::Attribute { expr, .. } => self.infer_expr(expr, locals),
            Expr::Assign { lhs: _, rhs } => self.infer_expr(rhs, locals),
            Expr::Let { binding, body } => {
                let value_ty = self.infer_expr(&binding.value, locals);
                locals.push_scope();
                self.bind_pattern(&binding.pattern, Some(value_ty), locals);
                let body_ty = self.infer_expr(body, locals);
                locals.pop_scope();
                body_ty
            }
            Expr::Var {
                target,
                value,
                body,
            } => {
                let value_ty = self.infer_expr(value, locals);
                locals.push_scope();
                if let Expr::Ident(name) = &target.0 {
                    locals.define(name, value_ty.clone());
                    self.record_binding_type(target.1, &value_ty);
                }
                let body_ty = self.infer_expr(body, locals);
                locals.pop_scope();
                body_ty
            }
            Expr::Block(items) => {
                locals.push_scope();
                let mut last_ty = Ty::Text("unit".to_string());
                for item in items {
                    last_ty = match &item.0 {
                        sail_parser::BlockItem::Let(binding) => {
                            let value_ty = self.infer_expr(&binding.value, locals);
                            self.bind_pattern(&binding.pattern, Some(value_ty), locals);
                            Ty::Text("unit".to_string())
                        }
                        sail_parser::BlockItem::Var { target, value } => {
                            let value_ty = self.infer_expr(value, locals);
                            if let Expr::Ident(name) = &target.0 {
                                locals.define(name, value_ty.clone());
                                self.record_binding_type(target.1, &value_ty);
                            }
                            Ty::Text("unit".to_string())
                        }
                        sail_parser::BlockItem::Expr(expr) => self.infer_expr(expr, locals),
                    };
                }
                locals.pop_scope();
                last_ty
            }
            Expr::Return(expr) => {
                let value_ty = self.infer_expr(expr, locals);
                if let Some(expected) = &locals.expected_return {
                    let mut subst = Subst::default();
                    if !unify(expected, &value_ty, &mut subst) {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            expr.1,
                            TypeError::Subtype {
                                lhs: value_ty.text(),
                                rhs: expected.text(),
                                constraint: None,
                            },
                        );
                    }
                }
                value_ty
            }
            Expr::Throw(expr) => self.infer_expr(expr, locals),
            Expr::Assert { condition, message } => {
                let cond_ty = self.infer_expr(condition, locals);
                let mut subst = Subst::default();
                if !unify(&Ty::Text("bool".to_string()), &cond_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        condition.1,
                        TypeError::Subtype {
                            lhs: cond_ty.text(),
                            rhs: "bool".to_string(),
                            constraint: None,
                        },
                    );
                }
                if let Some(message) = message {
                    self.infer_expr(message, locals);
                }
                Ty::Text("unit".to_string())
            }
            Expr::Exit(expr) => expr
                .as_ref()
                .map(|expr| self.infer_expr(expr, locals))
                .unwrap_or(Ty::Text("unit".to_string())),
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                let cond_ty = self.infer_expr(cond, locals);
                let mut subst = Subst::default();
                if !unify(&Ty::Text("bool".to_string()), &cond_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        cond.1,
                        TypeError::Subtype {
                            lhs: cond_ty.text(),
                            rhs: "bool".to_string(),
                            constraint: None,
                        },
                    );
                }
                let then_ty = self.infer_expr(then_branch, locals);
                let else_ty = else_branch
                    .as_ref()
                    .map(|branch| self.infer_expr(branch, locals))
                    .unwrap_or_else(|| Ty::Text("unit".to_string()));
                let mut subst = Subst::default();
                if unify(&then_ty, &else_ty, &mut subst) {
                    apply_subst(&then_ty, &subst)
                } else {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        expr.1,
                        TypeError::Subtype {
                            lhs: else_ty.text(),
                            rhs: then_ty.text(),
                            constraint: None,
                        },
                    );
                    Ty::Unknown
                }
            }
            Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
                let scrutinee_ty = self.infer_expr(scrutinee, locals);
                self.infer_match_cases(scrutinee_ty, cases, locals)
            }
            Expr::Foreach(foreach) => {
                self.infer_expr(&foreach.start, locals);
                self.infer_expr(&foreach.end, locals);
                if let Some(step) = &foreach.step {
                    self.infer_expr(step, locals);
                }
                self.infer_expr(&foreach.body, locals);
                Ty::Text("unit".to_string())
            }
            Expr::Repeat {
                measure,
                body,
                until,
            } => {
                if let Some(measure) = measure {
                    self.infer_expr(measure, locals);
                }
                self.infer_expr(body, locals);
                let until_ty = self.infer_expr(until, locals);
                let mut subst = Subst::default();
                if !unify(&Ty::Text("bool".to_string()), &until_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        until.1,
                        TypeError::Subtype {
                            lhs: until_ty.text(),
                            rhs: "bool".to_string(),
                            constraint: None,
                        },
                    );
                }
                Ty::Text("unit".to_string())
            }
            Expr::While {
                measure,
                cond,
                body,
            } => {
                if let Some(measure) = measure {
                    self.infer_expr(measure, locals);
                }
                let cond_ty = self.infer_expr(cond, locals);
                let mut subst = Subst::default();
                if !unify(&Ty::Text("bool".to_string()), &cond_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        cond.1,
                        TypeError::Subtype {
                            lhs: cond_ty.text(),
                            rhs: "bool".to_string(),
                            constraint: None,
                        },
                    );
                }
                self.infer_expr(body, locals);
                Ty::Text("unit".to_string())
            }
            Expr::Infix { lhs, op, rhs } => {
                self.infer_infix(expr.1, lhs, op.0.as_str(), rhs, locals)
            }
            Expr::Prefix { op, expr: inner } => {
                let inner_ty = self.infer_expr(inner, locals);
                match op.0.as_str() {
                    "not" => {
                        let mut subst = Subst::default();
                        if !unify(&Ty::Text("bool".to_string()), &inner_ty, &mut subst) {
                            self.push_error(
                                DiagnosticCode::TypeError,
                                inner.1,
                                TypeError::Subtype {
                                    lhs: inner_ty.text(),
                                    rhs: "bool".to_string(),
                                    constraint: None,
                                },
                            );
                        }
                        Ty::Text("bool".to_string())
                    }
                    "-" => inner_ty,
                    _ => Ty::Unknown,
                }
            }
            Expr::Cast { ty, .. } => type_from_type_expr(self.source, ty),
            Expr::Config(_) => Ty::Unknown,
            Expr::Literal(literal) => infer_literal_type(literal),
            Expr::Ident(name) => {
                if let Some(ty) = self.env.lookup_value(locals, name) {
                    ty
                } else if self.env.lookup_functions(name).is_empty() {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        expr.1,
                        TypeError::UnboundId {
                            id: name.clone(),
                            locals: locals
                                .scopes
                                .iter()
                                .flat_map(|scope| scope.keys().cloned())
                                .collect(),
                            have_function: false,
                        },
                    );
                    Ty::Unknown
                } else {
                    Ty::Unknown
                }
            }
            Expr::TypeVar(name) => Ty::Var(name.clone()),
            Expr::Ref(name) => self
                .env
                .lookup_value(locals, &name.0)
                .unwrap_or(Ty::Unknown),
            Expr::Call(call) => self.infer_call(call, locals),
            Expr::Field {
                expr: inner, field, ..
            } => {
                let base_ty = self.infer_expr(inner, locals);
                match base_ty {
                    Ty::Text(name) if self.env.records.contains_key(&name) => self
                        .env
                        .records
                        .get(&name)
                        .and_then(|fields| fields.get(&field.0))
                        .cloned()
                        .unwrap_or(Ty::Unknown),
                    Ty::App { name, .. } if self.env.records.contains_key(&name) => self
                        .env
                        .records
                        .get(&name)
                        .and_then(|fields| fields.get(&field.0))
                        .cloned()
                        .unwrap_or(Ty::Unknown),
                    _ => Ty::Unknown,
                }
            }
            Expr::SizeOf(_) => Ty::Text("int".to_string()),
            Expr::Constraint(_) => Ty::Text("bool".to_string()),
            Expr::Index { expr: inner, .. } => match self.infer_expr(inner, locals) {
                Ty::App { name, args, .. } if name == "vector" || name == "list" => args
                    .first()
                    .and_then(|arg| match arg {
                        TyArg::Type(ty) => Some((**ty).clone()),
                        TyArg::Value(_) => None,
                    })
                    .unwrap_or(Ty::Unknown),
                Ty::App { name, .. } if name == "bits" => Ty::Text("bit".to_string()),
                _ => Ty::Unknown,
            },
            Expr::Slice { expr: inner, .. } | Expr::VectorSlice { expr: inner, .. } => {
                self.infer_expr(inner, locals)
            }
            Expr::Struct { name, .. } => name
                .as_ref()
                .map(|name| Ty::Text(name.0.clone()))
                .unwrap_or(Ty::Unknown),
            Expr::Update { base, .. } => self.infer_expr(base, locals),
            Expr::List(items) => infer_collection_type(self, items, locals, "list"),
            Expr::Vector(items) => infer_collection_type(self, items, locals, "vector"),
            Expr::VectorUpdate { base, .. } => self.infer_expr(base, locals),
            Expr::Tuple(items) => Ty::Tuple(
                items
                    .iter()
                    .map(|item| self.infer_expr(item, locals))
                    .collect(),
            ),
            Expr::Error(_) => Ty::Unknown,
        };
        self.record_expr_type(expr.1, &ty);
        ty
    }

    fn infer_infix(
        &mut self,
        span: Span,
        lhs: &Spanned<Expr>,
        op: &str,
        rhs: &Spanned<Expr>,
        locals: &mut LocalEnv,
    ) -> Ty {
        let lhs_ty = self.infer_expr(lhs, locals);
        let rhs_ty = self.infer_expr(rhs, locals);
        match op {
            "&&" | "||" => {
                for (side_ty, side_span) in [(&lhs_ty, lhs.1), (&rhs_ty, rhs.1)] {
                    let mut subst = Subst::default();
                    if !unify(&Ty::Text("bool".to_string()), side_ty, &mut subst) {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            side_span,
                            TypeError::Subtype {
                                lhs: side_ty.text(),
                                rhs: "bool".to_string(),
                                constraint: None,
                            },
                        );
                    }
                }
                Ty::Text("bool".to_string())
            }
            "==" | "!=" | "<" | ">" | "<=" | ">=" => {
                let mut subst = Subst::default();
                if !unify(&lhs_ty, &rhs_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        span,
                        TypeError::Subtype {
                            lhs: rhs_ty.text(),
                            rhs: lhs_ty.text(),
                            constraint: None,
                        },
                    );
                }
                Ty::Text("bool".to_string())
            }
            "+" | "-" | "*" | "/" => {
                let numeric = if lhs_ty.text() == "real" || rhs_ty.text() == "real" {
                    Ty::Text("real".to_string())
                } else {
                    Ty::Text("int".to_string())
                };
                for (side_ty, side_span) in [(&lhs_ty, lhs.1), (&rhs_ty, rhs.1)] {
                    let mut subst = Subst::default();
                    if !unify(&numeric, side_ty, &mut subst)
                        && !unify(&Ty::Text("int".to_string()), side_ty, &mut Subst::default())
                    {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            side_span,
                            TypeError::Subtype {
                                lhs: side_ty.text(),
                                rhs: numeric.text(),
                                constraint: None,
                            },
                        );
                    }
                }
                numeric
            }
            _ => Ty::Unknown,
        }
    }

    fn infer_match_cases(
        &mut self,
        scrutinee_ty: Ty,
        cases: &[Spanned<MatchCase>],
        locals: &mut LocalEnv,
    ) -> Ty {
        let mut case_ty = None;
        for case in cases {
            locals.push_scope();
            self.bind_pattern(&case.0.pattern, Some(scrutinee_ty.clone()), locals);
            if let Some(guard) = &case.0.guard {
                let guard_ty = self.infer_expr(guard, locals);
                let mut subst = Subst::default();
                if !unify(&Ty::Text("bool".to_string()), &guard_ty, &mut subst) {
                    self.push_error(
                        DiagnosticCode::TypeError,
                        guard.1,
                        TypeError::Subtype {
                            lhs: guard_ty.text(),
                            rhs: "bool".to_string(),
                            constraint: None,
                        },
                    );
                }
            }
            let body_ty = self.infer_expr(&case.0.body, locals);
            locals.pop_scope();
            match &case_ty {
                None => case_ty = Some(body_ty),
                Some(prev_ty) => {
                    let mut subst = Subst::default();
                    if !unify(prev_ty, &body_ty, &mut subst) {
                        self.push_error(
                            DiagnosticCode::TypeError,
                            case.0.body.1,
                            TypeError::Subtype {
                                lhs: body_ty.text(),
                                rhs: prev_ty.text(),
                                constraint: None,
                            },
                        );
                    }
                }
            }
        }
        case_ty.unwrap_or(Ty::Unknown)
    }

    fn infer_call(&mut self, call: &sail_parser::Call, locals: &mut LocalEnv) -> Ty {
        let arg_types = call
            .args
            .iter()
            .map(|arg| self.infer_expr(arg, locals))
            .collect::<Vec<_>>();

        let Some(callee_name) = expr_symbol(&call.callee) else {
            self.infer_expr(&call.callee, locals);
            return Ty::Unknown;
        };
        let candidates = self.env.lookup_functions(callee_name);

        if candidates.is_empty() {
            self.push_error(
                DiagnosticCode::TypeError,
                call.callee.1,
                TypeError::NoFunctionType {
                    id: callee_name.to_string(),
                    functions: self.env.functions.keys().cloned().collect(),
                },
            );
            return Ty::Unknown;
        }

        let mut count_mismatch: Option<(usize, usize)> = None;
        let mut candidate_errors = Vec::new();
        for candidate in candidates {
            let required = candidate
                .implicit_params
                .iter()
                .filter(|is_implicit| !**is_implicit)
                .count();
            let total = candidate.params.len();
            if arg_types.len() < required || arg_types.len() > total {
                count_mismatch = Some(match count_mismatch {
                    Some((prev_required, prev_total)) => {
                        (prev_required.min(required), prev_total.max(total))
                    }
                    None => (required, total),
                });
                candidate_errors.push((
                    callee_name.to_string(),
                    call.callee.1,
                    Box::new(TypeError::other(format!(
                        "Expected {}{} arguments, found {}",
                        required,
                        if required == total {
                            String::new()
                        } else {
                            format!("-{total}")
                        },
                        arg_types.len()
                    ))),
                ));
                continue;
            }

            let expected_params = if arg_types.len() == total {
                candidate.params.iter().collect::<Vec<_>>()
            } else {
                candidate
                    .params
                    .iter()
                    .zip(candidate.implicit_params.iter())
                    .filter_map(|(param, is_implicit)| (!is_implicit).then_some(param))
                    .collect::<Vec<_>>()
            };
            let mut subst = Subst::default();
            let mut ok = true;
            for (index, (expected, actual)) in
                expected_params.iter().zip(arg_types.iter()).enumerate()
            {
                if !unify(expected, actual, &mut subst) {
                    ok = false;
                    candidate_errors.push((
                        callee_name.to_string(),
                        call.args[index].1,
                        Box::new(TypeError::FunctionArg {
                            span: call.args[index].1,
                            ty: expected.text(),
                            error: Box::new(TypeError::Subtype {
                                lhs: actual.text(),
                                rhs: expected.text(),
                                constraint: None,
                            }),
                        }),
                    ));
                    break;
                }
            }
            if ok {
                return apply_subst(&candidate.ret, &subst);
            }
        }

        if let Some((expected, actual)) = count_mismatch {
            self.push_error(
                DiagnosticCode::MismatchedArgCount,
                call.callee.1,
                TypeError::other(if expected == actual {
                    format!("Expected {} arguments, found {}", actual, arg_types.len())
                } else {
                    format!(
                        "Expected {}-{} arguments, found {}",
                        expected,
                        actual,
                        arg_types.len()
                    )
                }),
            );
        } else {
            self.push_error(
                DiagnosticCode::TypeError,
                call.callee.1,
                TypeError::NoOverloading {
                    id: callee_name.to_string(),
                    errors: candidate_errors,
                },
            );
        }
        Ty::Unknown
    }
}

fn infer_collection_type(
    checker: &mut Checker<'_>,
    items: &[Spanned<Expr>],
    locals: &mut LocalEnv,
    name: &str,
) -> Ty {
    let mut item_ty = None;
    for item in items {
        let ty = checker.infer_expr(item, locals);
        if let Some(prev) = &item_ty {
            let mut subst = Subst::default();
            if !unify(prev, &ty, &mut subst) {
                return Ty::Unknown;
            }
        } else {
            item_ty = Some(ty);
        }
    }
    let elem = item_ty.unwrap_or(Ty::Unknown);
    Ty::App {
        name: name.to_string(),
        args: vec![TyArg::Type(Box::new(elem.clone()))],
        text: format!("{name}({})", elem.text()),
    }
}

fn expr_symbol(expr: &Spanned<Expr>) -> Option<&str> {
    match &expr.0 {
        Expr::Ident(name) => Some(name.as_str()),
        Expr::Field { field, .. } => Some(field.0.as_str()),
        _ => None,
    }
}

fn build_env_from_files<'a, I>(files: I) -> TopLevelEnv
where
    I: IntoIterator<Item = &'a File>,
{
    let mut env = TopLevelEnv::default();
    for file in files {
        let Some(ast) = file.core_ast() else {
            continue;
        };
        let (mut file_env, _) = TopLevelEnv::from_ast(file.source.text(), ast);
        apply_callable_signature_metadata(file, &mut file_env);
        for (name, schemes) in file_env.functions {
            env.functions.entry(name).or_default().extend(schemes);
        }
        env.overloads.extend(file_env.overloads);
        env.values.extend(file_env.values);
        env.records.extend(file_env.records);
    }
    env
}

pub(crate) fn check_file(file: &File) -> Option<TypeCheckResult> {
    let ast = file.core_ast()?;
    let (mut env, pattern_constants) = TopLevelEnv::from_ast(file.source.text(), ast);
    apply_callable_signature_metadata(file, &mut env);
    Some(Checker::new(file, env, pattern_constants).check_source_file(ast))
}

pub(crate) fn infer_expr_type_text_in_files<'a, I>(
    files: I,
    current_file: &File,
    expr: &Spanned<Expr>,
) -> Option<String>
where
    I: IntoIterator<Item = &'a File>,
{
    if let Some(result) = current_file.type_check() {
        if let Some(ty) = result.expr_type_text(expr.1) {
            return Some(ty.to_string());
        }
    }

    let env = build_env_from_files(files);
    let pattern_constants = current_file
        .core_ast()
        .map(|ast| TopLevelEnv::from_ast(current_file.source.text(), ast).1)
        .unwrap_or_default();
    let mut checker = Checker::new(current_file, env, pattern_constants);
    let mut locals = LocalEnv::new(None);
    let ty = checker.infer_expr(expr, &mut locals);
    if ty.is_unknown() {
        None
    } else {
        Some(ty.text())
    }
}
