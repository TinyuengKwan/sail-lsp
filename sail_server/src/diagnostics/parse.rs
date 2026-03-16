use std::collections::HashSet;

use chumsky::error::Rich;

use crate::diagnostics::error_format::Message;
use crate::diagnostics::reporting::{
    diagnostic_for_error, Error as ReportingError, WarningEmitter,
};
use crate::diagnostics::{Diagnostic, DiagnosticCode};
use crate::state::File;
use sail_parser::{
    core_ast::{
        CallableDefinition, CallableSpecification, ConstraintDefinition, DefaultDefinition,
        DefinitionKind, DirectiveDefinition, EndDefinition, FixityDefinition,
        InstantiationDefinition, NamedDefinition, ScatteredClauseDefinition, ScatteredDefinition,
        SourceFile, TerminationMeasureDefinition, TypeAliasDefinition,
    },
    AttributeData, BitfieldField, BlockItem, Call, CallableClause, CallableQuantifier,
    EnumFunction, EnumMember, Expr, ExternSpec, FieldExpr, FieldPattern, LoopMeasure, MappingArm,
    MappingBody, MatchCase, NamedDefDetail, Pattern, RecMeasure, Span, TerminationMeasureKind,
    Token, TypeExpr, TypeParamSpec, UnionPayload, UnionVariant, VectorUpdate,
};
pub(crate) fn compute_parse_diagnostics(
    file: &File,
    lex_errors: &[Rich<'_, char, sail_parser::Span>],
) -> Vec<Diagnostic> {
    let mut collector = ParseDiagnosticCollector::new(file);
    collector.collect_lex_errors(lex_errors);
    if let Some(tokens) = file.tokens.as_deref() {
        collector.collect_bracket_diagnostics(tokens);
    }
    if let Some(ast) = file.core_ast() {
        collector.collect_upstream_warnings(ast);
    }
    collector.finish()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BracketKind {
    Round,
    Square,
    Curly,
    CurlyBar,
    SquareBar,
}

impl BracketKind {
    fn expected_closer(self) -> &'static str {
        match self {
            BracketKind::Round => ")",
            BracketKind::Square => "]",
            BracketKind::Curly => "}",
            BracketKind::CurlyBar => "|}",
            BracketKind::SquareBar => "|]",
        }
    }

    fn closer_text(self) -> &'static str {
        self.expected_closer()
    }
}

fn open_bracket_kind(token: &Token) -> Option<BracketKind> {
    match token {
        Token::LeftBracket => Some(BracketKind::Round),
        Token::LeftSquareBracket => Some(BracketKind::Square),
        Token::LeftCurlyBracket => Some(BracketKind::Curly),
        Token::LeftCurlyBar => Some(BracketKind::CurlyBar),
        Token::LeftSquareBar => Some(BracketKind::SquareBar),
        _ => None,
    }
}

fn close_bracket_kind(token: &Token) -> Option<BracketKind> {
    match token {
        Token::RightBracket => Some(BracketKind::Round),
        Token::RightSquareBracket => Some(BracketKind::Square),
        Token::RightCurlyBracket => Some(BracketKind::Curly),
        Token::RightCurlyBar => Some(BracketKind::CurlyBar),
        Token::RightSquareBar => Some(BracketKind::SquareBar),
        _ => None,
    }
}

struct ParseDiagnosticCollector<'a> {
    file: &'a File,
    diagnostics: Vec<Diagnostic>,
    seen_errors: HashSet<(DiagnosticCode, usize, usize)>,
    warnings: WarningEmitter,
}

impl<'a> ParseDiagnosticCollector<'a> {
    fn new(file: &'a File) -> Self {
        Self {
            file,
            diagnostics: Vec::new(),
            seen_errors: HashSet::new(),
            warnings: WarningEmitter::new(),
        }
    }

    fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    fn emit_error(&mut self, code: DiagnosticCode, span: Span, error: ReportingError) {
        if !self
            .seen_errors
            .insert((code.clone(), span.start, span.end))
        {
            return;
        }
        self.diagnostics
            .push(diagnostic_for_error(self.file, code, error));
    }

    fn collect_lex_errors(&mut self, lex_errors: &[Rich<'_, char, Span>]) {
        for error in lex_errors {
            self.emit_error(
                DiagnosticCode::LexicalError,
                *error.span(),
                ReportingError::Lex {
                    span: *error.span(),
                    message: error.to_string(),
                },
            );
        }
    }

    fn collect_bracket_diagnostics(&mut self, tokens: &[(Token, Span)]) {
        let mut stack: Vec<(BracketKind, Span)> = Vec::new();

        for (token, span) in tokens {
            if let Some(kind) = open_bracket_kind(token) {
                stack.push((kind, *span));
                continue;
            }

            let Some(close_kind) = close_bracket_kind(token) else {
                continue;
            };

            let mut matched = false;
            while let Some((open_kind, _)) = stack.last().copied() {
                if open_kind == close_kind {
                    stack.pop();
                    matched = true;
                    break;
                }

                self.emit_error(
                    DiagnosticCode::SyntaxError,
                    Span::new(span.start, span.start),
                    ReportingError::Syntax {
                        span: Span::new(span.start, span.start),
                        message: format!("expected '{}'", open_kind.expected_closer()),
                    },
                );
                stack.pop();
            }

            if !matched {
                self.emit_error(
                    DiagnosticCode::SyntaxError,
                    *span,
                    ReportingError::Syntax {
                        span: *span,
                        message: format!("unexpected '{}'", close_kind.closer_text()),
                    },
                );
            }
        }

        let eof = tokens
            .last()
            .map(|(_, span)| Span::new(span.end, span.end))
            .unwrap_or_else(|| Span::new(0, 0));
        for (open_kind, _) in stack {
            self.emit_error(
                DiagnosticCode::SyntaxError,
                eof,
                ReportingError::Syntax {
                    span: eof,
                    message: format!("expected '{}'", open_kind.expected_closer()),
                },
            );
        }
    }

    fn collect_upstream_warnings(&mut self, ast: &SourceFile) {
        for (item, _) in &ast.defs {
            self.collect_item_warnings(&item.kind);
        }
    }

    fn collect_item_warnings(&mut self, item: &DefinitionKind) {
        match item {
            DefinitionKind::Scattered(ScatteredDefinition {
                params, signature, ..
            }) => {
                if let Some(params) = params {
                    self.collect_type_param_spec_warnings(&params.0);
                }
                if let Some(signature) = signature {
                    self.collect_type_warnings(signature);
                }
            }
            DefinitionKind::ScatteredClause(ScatteredClauseDefinition { ty, .. }) => {
                if let Some(ty) = ty {
                    self.collect_type_warnings(ty);
                }
            }
            DefinitionKind::CallableSpec(spec) => self.collect_callable_spec_warnings(spec),
            DefinitionKind::Callable(def) => self.collect_callable_def_warnings(def),
            DefinitionKind::TypeAlias(alias) => self.collect_type_alias_warnings(alias),
            DefinitionKind::Named(def) => self.collect_named_def_warnings(def),
            DefinitionKind::Constraint(ConstraintDefinition { ty, .. }) => {
                self.collect_type_warnings(ty)
            }
            DefinitionKind::TerminationMeasure(def) => {
                self.collect_termination_measure_warnings(def)
            }
            DefinitionKind::Default(DefaultDefinition { .. })
            | DefinitionKind::Fixity(FixityDefinition { .. })
            | DefinitionKind::Instantiation(InstantiationDefinition { .. })
            | DefinitionKind::Directive(DirectiveDefinition { .. })
            | DefinitionKind::End(EndDefinition { .. }) => {}
        }
    }

    fn collect_callable_spec_warnings(&mut self, spec: &CallableSpecification) {
        if let Some(externs) = &spec.externs {
            match &externs.0 {
                ExternSpec::String { purity: None, .. }
                | ExternSpec::Bindings { purity: None, .. } => self.warnings.warn(
                    self.file,
                    &mut self.diagnostics,
                    DiagnosticCode::MissingExternPurity,
                    "Deprecated",
                    externs.1,
                    Message::line(
                        "All external bindings should be marked as either pure or impure",
                    ),
                ),
                _ => {}
            }
        }
        self.collect_type_warnings(&spec.signature);
    }

    fn collect_callable_def_warnings(&mut self, def: &CallableDefinition) {
        if let Some(signature) = &def.signature {
            self.collect_type_warnings(signature);
        }
        if let Some(measure) = &def.rec_measure {
            self.collect_rec_measure_warnings(&measure.0);
        }
        for clause in &def.clauses {
            self.collect_callable_clause_warnings(&clause.0);
        }
        for param in &def.params {
            self.collect_pattern_warnings(param);
        }
        if let Some(return_ty) = &def.return_ty {
            self.collect_type_warnings(return_ty);
        }
        if let Some(body) = &def.body {
            self.collect_expr_warnings(body);
        }
        if let Some(body) = &def.mapping_body {
            self.collect_mapping_body_warnings(body);
        }
    }

    fn collect_callable_clause_warnings(&mut self, clause: &CallableClause) {
        for pattern in &clause.patterns {
            self.collect_pattern_warnings(pattern);
        }
        if let Some(quantifier) = &clause.quantifier {
            self.collect_callable_quantifier_warnings(quantifier);
        }
        if let Some(guard) = &clause.guard {
            self.collect_expr_warnings(guard);
        }
        if let Some(return_ty) = &clause.return_ty {
            self.collect_type_warnings(return_ty);
        }
        if let Some(body) = &clause.body {
            self.collect_expr_warnings(body);
        }
        if let Some(body) = &clause.mapping_body {
            self.collect_mapping_body_warnings(body);
        }
    }

    fn collect_callable_quantifier_warnings(&mut self, quantifier: &CallableQuantifier) {
        if let Some(constraint) = &quantifier.constraint {
            self.collect_type_warnings(constraint);
        }
    }

    fn collect_rec_measure_warnings(&mut self, measure: &RecMeasure) {
        self.collect_pattern_warnings(&measure.pattern);
        self.collect_expr_warnings(&measure.body);
    }

    fn collect_type_alias_warnings(&mut self, alias: &TypeAliasDefinition) {
        if let Some(params) = &alias.params {
            self.collect_type_param_spec_warnings(&params.0);
        }
        if let Some(target) = &alias.target {
            self.collect_type_warnings(target);
        }
    }

    fn collect_named_def_warnings(&mut self, def: &NamedDefinition) {
        if let Some(params) = &def.params {
            self.collect_type_param_spec_warnings(&params.0);
        }
        if let Some(ty) = &def.ty {
            self.collect_type_warnings(ty);
        }
        if let Some(detail) = &def.detail {
            self.collect_named_detail_warnings(detail);
        }
        if let Some(value) = &def.value {
            self.collect_expr_warnings(value);
        }
    }

    fn collect_named_detail_warnings(&mut self, detail: &NamedDefDetail) {
        match detail {
            NamedDefDetail::Enum { members, functions } => {
                for member in members {
                    self.collect_enum_member_warnings(&member.0);
                }
                for function in functions {
                    self.collect_enum_function_warnings(&function.0);
                }
            }
            NamedDefDetail::Struct { fields } => {
                for field in fields {
                    self.collect_type_warnings(&field.0.ty);
                }
            }
            NamedDefDetail::Union { variants } => {
                for variant in variants {
                    self.collect_union_variant_warnings(&variant.0);
                }
            }
            NamedDefDetail::Bitfield { fields } => {
                for field in fields {
                    self.collect_bitfield_field_warnings(&field.0);
                }
            }
        }
    }

    fn collect_enum_member_warnings(&mut self, member: &EnumMember) {
        if let Some(value) = &member.value {
            self.collect_expr_warnings(value);
        }
    }

    fn collect_enum_function_warnings(&mut self, function: &EnumFunction) {
        self.collect_type_warnings(&function.ty);
    }

    fn collect_union_variant_warnings(&mut self, variant: &UnionVariant) {
        match &variant.payload {
            UnionPayload::Type(ty) => self.collect_type_warnings(ty),
            UnionPayload::Struct { fields } => {
                for field in fields {
                    self.collect_type_warnings(&field.0.ty);
                }
            }
        }
    }

    fn collect_bitfield_field_warnings(&mut self, field: &BitfieldField) {
        self.collect_type_warnings(&field.high);
        if let Some(low) = &field.low {
            self.collect_type_warnings(low);
        }
    }

    fn collect_termination_measure_warnings(&mut self, def: &TerminationMeasureDefinition) {
        match &def.kind {
            TerminationMeasureKind::Function { pattern, body } => {
                self.collect_pattern_warnings(pattern);
                self.collect_expr_warnings(body);
            }
            TerminationMeasureKind::Loop { measures } => {
                for measure in measures {
                    match &measure.0 {
                        LoopMeasure::Until(expr)
                        | LoopMeasure::Repeat(expr)
                        | LoopMeasure::While(expr) => self.collect_expr_warnings(expr),
                    }
                }
            }
        }
    }

    fn collect_type_param_spec_warnings(&mut self, spec: &TypeParamSpec) {
        if let Some(tail) = &spec.tail {
            match tail {
                sail_parser::TypeParamTail::Type(ty)
                | sail_parser::TypeParamTail::Constraint(ty) => self.collect_type_warnings(ty),
            }
        }
    }

    fn collect_pattern_warnings(&mut self, pattern: &(Pattern, Span)) {
        match &pattern.0 {
            Pattern::Attribute { pattern, .. } => self.collect_pattern_warnings(pattern),
            Pattern::Typed(inner, ty) | Pattern::AsType(inner, ty) => {
                self.collect_pattern_warnings(inner);
                self.collect_type_warnings(ty);
            }
            Pattern::Tuple(items) | Pattern::List(items) | Pattern::Vector(items) => {
                for item in items {
                    self.collect_pattern_warnings(item);
                }
            }
            Pattern::App { args, .. } => {
                for arg in args {
                    self.collect_pattern_warnings(arg);
                }
            }
            Pattern::Index { index, .. } => self.collect_type_warnings(index),
            Pattern::RangeIndex { start, end, .. } => {
                self.collect_type_warnings(start);
                self.collect_type_warnings(end);
            }
            Pattern::Struct { fields, .. } => {
                for field in fields {
                    self.collect_field_pattern_warnings(&field.0);
                }
            }
            Pattern::Infix { lhs, rhs, .. } => {
                self.collect_pattern_warnings(lhs);
                self.collect_pattern_warnings(rhs);
            }
            Pattern::AsBinding { pattern, .. } => self.collect_pattern_warnings(pattern),
            Pattern::Wild
            | Pattern::Literal(_)
            | Pattern::Ident(_)
            | Pattern::TypeVar(_)
            | Pattern::Error(_) => {}
        }
    }

    fn collect_field_pattern_warnings(&mut self, pattern: &FieldPattern) {
        match pattern {
            FieldPattern::Binding { pattern, .. } => self.collect_pattern_warnings(pattern),
            FieldPattern::Shorthand(_) | FieldPattern::Wild(_) => {}
        }
    }

    fn collect_expr_warnings(&mut self, expr: &(Expr, Span)) {
        match &expr.0 {
            Expr::Attribute { expr, attr } => {
                self.collect_attribute_warnings(&attr.0.parsed_data);
                self.collect_expr_warnings(expr);
            }
            Expr::Assign { lhs, rhs } | Expr::Infix { lhs, rhs, .. } => {
                self.collect_expr_warnings(lhs);
                self.collect_expr_warnings(rhs);
            }
            Expr::Let { binding, body } => {
                self.collect_pattern_warnings(&binding.pattern);
                self.collect_expr_warnings(&binding.value);
                self.collect_expr_warnings(body);
            }
            Expr::Var {
                target,
                value,
                body,
            } => {
                self.collect_expr_warnings(target);
                self.collect_expr_warnings(value);
                self.collect_expr_warnings(body);
            }
            Expr::Block(items) => {
                for item in items {
                    self.collect_block_item_warnings(&item.0);
                }
            }
            Expr::Return(expr)
            | Expr::Throw(expr)
            | Expr::Prefix { expr, .. }
            | Expr::Field { expr, .. } => self.collect_expr_warnings(expr),
            Expr::Assert { condition, message } => {
                self.collect_expr_warnings(condition);
                if let Some(message) = message {
                    self.collect_expr_warnings(message);
                }
            }
            Expr::Exit(expr) => {
                if let Some(expr) = expr {
                    self.collect_expr_warnings(expr);
                }
            }
            Expr::If {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_expr_warnings(cond);
                self.collect_expr_warnings(then_branch);
                if let Some(else_branch) = else_branch {
                    self.collect_expr_warnings(else_branch);
                }
            }
            Expr::Match { scrutinee, cases } | Expr::Try { scrutinee, cases } => {
                self.collect_expr_warnings(scrutinee);
                for case in cases {
                    self.collect_match_case_warnings(&case.0);
                }
            }
            Expr::Foreach(foreach) => {
                self.collect_expr_warnings(&foreach.start);
                self.collect_expr_warnings(&foreach.end);
                if let Some(step) = &foreach.step {
                    self.collect_expr_warnings(step);
                }
                if let Some(ty) = &foreach.ty {
                    self.collect_type_warnings(ty);
                }
                self.collect_expr_warnings(&foreach.body);
            }
            Expr::Repeat {
                measure,
                body,
                until,
            } => {
                if let Some(measure) = measure {
                    self.collect_expr_warnings(measure);
                }
                self.collect_expr_warnings(body);
                self.collect_expr_warnings(until);
            }
            Expr::While {
                measure,
                cond,
                body,
            } => {
                if let Some(measure) = measure {
                    self.collect_expr_warnings(measure);
                }
                self.collect_expr_warnings(cond);
                self.collect_expr_warnings(body);
            }
            Expr::Cast { expr: inner, ty } => {
                self.warnings.warn(
                    self.file,
                    &mut self.diagnostics,
                    DiagnosticCode::DeprecatedCastAnnotation,
                    "Deprecated",
                    expr.1,
                    Message::line(
                        "Cast annotations are deprecated. They will be removed in a future version of the language.",
                    ),
                );
                self.collect_expr_warnings(inner);
                self.collect_type_warnings(ty);
            }
            Expr::Call(Call { callee, args, .. }) => {
                self.collect_expr_warnings(callee);
                for arg in args {
                    self.collect_expr_warnings(arg);
                }
            }
            Expr::SizeOf(ty) | Expr::Constraint(ty) => self.collect_type_warnings(ty),
            Expr::Index { expr, index } => {
                self.collect_expr_warnings(expr);
                self.collect_expr_warnings(index);
            }
            Expr::Slice { expr, start, end } => {
                self.collect_expr_warnings(expr);
                self.collect_expr_warnings(start);
                self.collect_expr_warnings(end);
            }
            Expr::VectorSlice {
                expr,
                start,
                length,
            } => {
                self.collect_expr_warnings(expr);
                self.collect_expr_warnings(start);
                self.collect_expr_warnings(length);
            }
            Expr::Struct { fields, .. } => {
                for field in fields {
                    self.collect_field_expr_warnings(&field.0);
                }
            }
            Expr::Update { base, fields } => {
                self.collect_expr_warnings(base);
                for field in fields {
                    self.collect_field_expr_warnings(&field.0);
                }
            }
            Expr::List(items) | Expr::Vector(items) | Expr::Tuple(items) => {
                for item in items {
                    self.collect_expr_warnings(item);
                }
            }
            Expr::VectorUpdate { base, updates } => {
                self.collect_expr_warnings(base);
                for update in updates {
                    self.collect_vector_update_warnings(&update.0);
                }
            }
            Expr::Config(_)
            | Expr::Literal(_)
            | Expr::Ident(_)
            | Expr::TypeVar(_)
            | Expr::Ref(_)
            | Expr::Error(_) => {}
        }
    }

    fn collect_block_item_warnings(&mut self, item: &BlockItem) {
        match item {
            BlockItem::Let(binding) => {
                self.collect_pattern_warnings(&binding.pattern);
                self.collect_expr_warnings(&binding.value);
            }
            BlockItem::Var { target, value } => {
                self.collect_expr_warnings(target);
                self.collect_expr_warnings(value);
            }
            BlockItem::Expr(expr) => self.collect_expr_warnings(expr),
        }
    }

    fn collect_match_case_warnings(&mut self, case: &MatchCase) {
        self.collect_pattern_warnings(&case.pattern);
        if let Some(guard) = &case.guard {
            self.collect_expr_warnings(guard);
        }
        self.collect_expr_warnings(&case.body);
    }

    fn collect_mapping_body_warnings(&mut self, body: &MappingBody) {
        for arm in &body.arms {
            self.collect_mapping_arm_warnings(&arm.0);
        }
    }

    fn collect_mapping_arm_warnings(&mut self, arm: &MappingArm) {
        self.collect_expr_warnings(&arm.lhs);
        self.collect_expr_warnings(&arm.rhs);
        if let Some(pattern) = &arm.lhs_pattern {
            self.collect_pattern_warnings(pattern);
        }
        if let Some(pattern) = &arm.rhs_pattern {
            self.collect_pattern_warnings(pattern);
        }
        if let Some(guard) = &arm.guard {
            self.collect_expr_warnings(guard);
        }
    }

    fn collect_field_expr_warnings(&mut self, field: &FieldExpr) {
        match field {
            FieldExpr::Assignment { target, value } => {
                self.collect_expr_warnings(target);
                self.collect_expr_warnings(value);
            }
            FieldExpr::Shorthand(_) => {}
        }
    }

    fn collect_vector_update_warnings(&mut self, update: &VectorUpdate) {
        match update {
            VectorUpdate::Assignment { target, value } => {
                self.collect_expr_warnings(target);
                self.collect_expr_warnings(value);
            }
            VectorUpdate::RangeAssignment { start, end, value } => {
                self.collect_expr_warnings(start);
                self.collect_expr_warnings(end);
                self.collect_expr_warnings(value);
            }
            VectorUpdate::Shorthand(_) => {}
        }
    }

    fn collect_type_warnings(&mut self, ty: &(TypeExpr, Span)) {
        match &ty.0 {
            TypeExpr::Forall {
                constraint, body, ..
            } => {
                if let Some(constraint) = constraint {
                    self.collect_type_warnings(constraint);
                }
                self.collect_type_warnings(body);
            }
            TypeExpr::Existential {
                constraint, body, ..
            } => {
                if let Some(constraint) = constraint {
                    self.collect_type_warnings(constraint);
                }
                self.collect_type_warnings(body);
            }
            TypeExpr::Effect { ty: inner, .. } => {
                self.warnings.warn(
                    self.file,
                    &mut self.diagnostics,
                    DiagnosticCode::DeprecatedEffectAnnotation,
                    "Deprecated",
                    ty.1,
                    Message::line(
                        "Explicit effect annotations are deprecated. They are no longer used and can be removed.",
                    ),
                );
                self.collect_type_warnings(inner);
            }
            TypeExpr::App { args, .. } | TypeExpr::Set(args) | TypeExpr::Tuple(args) => {
                for arg in args {
                    self.collect_type_warnings(arg);
                }
            }
            TypeExpr::Register(inner) | TypeExpr::Prefix { ty: inner, .. } => {
                self.collect_type_warnings(inner);
            }
            TypeExpr::Infix { lhs, rhs, .. } => {
                self.collect_type_warnings(lhs);
                self.collect_type_warnings(rhs);
            }
            TypeExpr::Conditional {
                cond,
                then_ty,
                else_ty,
            } => {
                self.collect_type_warnings(cond);
                self.collect_type_warnings(then_ty);
                self.collect_type_warnings(else_ty);
            }
            TypeExpr::Arrow { params, ret, .. } => {
                for param in params {
                    self.collect_type_warnings(param);
                }
                self.collect_type_warnings(ret);
            }
            TypeExpr::Wild
            | TypeExpr::Named(_)
            | TypeExpr::TypeVar(_)
            | TypeExpr::Literal(_)
            | TypeExpr::Dec
            | TypeExpr::Inc
            | TypeExpr::Config(_)
            | TypeExpr::Error(_) => {}
        }
    }

    fn collect_attribute_warnings(&mut self, data: &Option<(AttributeData, Span)>) {
        let Some((data, _)) = data else {
            return;
        };
        self.collect_attribute_data_warnings(data);
    }

    fn collect_attribute_data_warnings(&mut self, data: &AttributeData) {
        match data {
            AttributeData::Object(entries) => {
                for entry in entries {
                    self.collect_attribute_data_warnings(&entry.0.value.0);
                }
            }
            AttributeData::Array(items) => {
                for item in items {
                    self.collect_attribute_data_warnings(&item.0);
                }
            }
            AttributeData::Number(_)
            | AttributeData::String(_)
            | AttributeData::Ident(_)
            | AttributeData::Bool(_) => {}
        }
    }
}
