use std::collections::{HashMap, HashSet};

use crate::diagnostics::reporting::{
    diagnostic_for_error, diagnostic_for_warning, unnecessary_warning, Error as ReportingError,
    Message,
};
use crate::diagnostics::type_error::TypeError;
use crate::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use crate::state::File;
use sail_parser::{
    core_ast::{DefinitionKind as CoreDefinitionKind, SourceFile as CoreSourceFile},
    BlockItem as AstBlockItem, Expr as AstExpr, FieldExpr as AstFieldExpr,
    FieldPattern as AstFieldPattern, Lexp as AstLexp, MatchCase as AstMatchCase,
    Pattern as AstPattern,
};

#[derive(Debug)]
struct BindingRecord {
    name: String,
    span: sail_parser::Span,
    used: bool,
    warn_unused: bool,
    mutable: bool,
    modified: bool,
}

#[derive(Default)]
struct BindingTracker {
    bindings: Vec<BindingRecord>,
    scopes: Vec<Vec<usize>>,
    by_name: HashMap<String, Vec<usize>>,
}

impl BindingTracker {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self, file: &File, diagnostics: &mut Vec<Diagnostic>) {
        let Some(ids) = self.scopes.pop() else {
            return;
        };

        for idx in ids.into_iter().rev() {
            let binding = &self.bindings[idx];
            let name = binding.name.clone();
            let span = binding.span;
            let used = binding.used;
            let warn_unused = binding.warn_unused;

            let remove_entry = if let Some(stack) = self.by_name.get_mut(&name) {
                debug_assert_eq!(stack.pop(), Some(idx));
                stack.is_empty()
            } else {
                false
            };
            if remove_entry {
                self.by_name.remove(&name);
            }

            if warn_unused && !used && !name.starts_with('_') {
                diagnostics.push(unused_binding_diagnostic(file, span, &name));
            }

            if warn_unused && binding.mutable && !binding.modified && !name.starts_with('_') {
                diagnostics.push(unmodified_mutable_diagnostic(file, span, &name));
            }
        }
    }

    fn define_binding(
        &mut self,
        name: &str,
        span: sail_parser::Span,
        warn_unused: bool,
        mutable: bool,
    ) {
        let idx = self.bindings.len();
        self.bindings.push(BindingRecord {
            name: name.to_string(),
            span,
            used: false,
            warn_unused,
            mutable,
            modified: false,
        });
        self.scopes.last_mut().expect("binding scope").push(idx);
        self.by_name.entry(name.to_string()).or_default().push(idx);
    }

    fn mark_used(&mut self, name: &str) {
        let Some(idx) = self
            .by_name
            .get(name)
            .and_then(|stack| stack.last())
            .copied()
        else {
            return;
        };
        if let Some(binding) = self.bindings.get_mut(idx) {
            binding.used = true;
        }
    }

    fn is_mutable(&self, name: &str) -> bool {
        self.by_name
            .get(name)
            .and_then(|stack| stack.last())
            .and_then(|&idx| self.bindings.get(idx))
            .is_some_and(|b| b.mutable)
    }

    fn mark_modified(&mut self, name: &str) {
        let Some(idx) = self
            .by_name
            .get(name)
            .and_then(|stack| stack.last())
            .copied()
        else {
            return;
        };
        if let Some(binding) = self.bindings.get_mut(idx) {
            binding.modified = true;
        }
    }
}

struct AstSemanticAnalyzer<'a> {
    file: &'a File,
    diagnostics: Vec<Diagnostic>,
    pattern_constants: HashSet<String>,
    union_constructors: HashSet<String>,
    registers: HashSet<String>,
    tracker: BindingTracker,
}

impl<'a> AstSemanticAnalyzer<'a> {
    fn new(file: &'a File) -> Self {
        Self {
            file,
            diagnostics: Vec::new(),
            pattern_constants: file
                .parsed()
                .map(|parsed| {
                    parsed
                        .decls
                        .iter()
                        .filter(|decl| {
                            decl.scope == sail_parser::Scope::TopLevel
                                && decl.kind == sail_parser::DeclKind::EnumMember
                        })
                        .map(|decl| decl.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            union_constructors: file
                .core_ast()
                .map(|ast| {
                    let mut ctors = HashSet::new();
                    for (item, _) in &ast.defs {
                        if let CoreDefinitionKind::Named(def) = &item.kind {
                            if let Some(sail_parser::NamedDefDetail::Union { variants, .. }) =
                                &def.detail
                            {
                                for variant in variants {
                                    ctors.insert(variant.0.name.0.clone());
                                }
                            }
                        }
                    }
                    ctors
                })
                .unwrap_or_default(),
            registers: file
                .parsed()
                .map(|parsed| {
                    parsed
                        .decls
                        .iter()
                        .filter(|decl| {
                            decl.scope == sail_parser::Scope::TopLevel
                                && decl.kind == sail_parser::DeclKind::Register
                        })
                        .map(|decl| decl.name.clone())
                        .collect()
                })
                .unwrap_or_default(),
            tracker: BindingTracker::default(),
        }
    }

    fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    fn analyze_source_file(&mut self, ast: &CoreSourceFile) {
        for (item, _) in &ast.defs {
            match &item.kind {
                CoreDefinitionKind::Callable(def) => {
                    self.tracker.push_scope();
                    if let Some(rec_measure) = &def.rec_measure {
                        self.define_pattern_bindings(&rec_measure.0.pattern, false);
                        self.analyze_expr(&rec_measure.0.body);
                    }
                    if def.clauses.is_empty() {
                        for param in &def.params {
                            self.define_pattern_bindings(param, false);
                        }
                        if let Some(body) = &def.body {
                            self.analyze_expr(body);
                        }
                        if let Some(body) = &def.mapping_body {
                            for arm in &body.arms {
                                if let Some(guard) = &arm.0.guard {
                                    self.analyze_expr(guard);
                                }
                                self.analyze_expr(&arm.0.lhs);
                                self.analyze_expr(&arm.0.rhs);
                            }
                        }
                    } else {
                        for clause in &def.clauses {
                            self.tracker.push_scope();
                            for pattern in &clause.0.patterns {
                                self.define_pattern_bindings(pattern, false);
                            }
                            if let Some(guard) = &clause.0.guard {
                                self.analyze_expr(guard);
                            }
                            if let Some(body) = &clause.0.body {
                                self.analyze_expr(body);
                            }
                            if let Some(body) = &clause.0.mapping_body {
                                for arm in &body.arms {
                                    if let Some(guard) = &arm.0.guard {
                                        self.analyze_expr(guard);
                                    }
                                    self.analyze_expr(&arm.0.lhs);
                                    self.analyze_expr(&arm.0.rhs);
                                }
                            }
                            self.tracker.pop_scope(self.file, &mut self.diagnostics);
                        }
                    }
                    self.tracker.pop_scope(self.file, &mut self.diagnostics);
                }
                CoreDefinitionKind::Named(def) => {
                    let Some(value) = &def.value else {
                        continue;
                    };
                    self.tracker.push_scope();
                    self.analyze_expr(value);
                    self.tracker.pop_scope(self.file, &mut self.diagnostics);
                }
                CoreDefinitionKind::Directive(_) => {}
                CoreDefinitionKind::TerminationMeasure(def) => {
                    self.tracker.push_scope();
                    match &def.kind {
                        sail_parser::TerminationMeasureKind::Function { pattern, body } => {
                            self.define_pattern_bindings(pattern, false);
                            self.analyze_expr(body);
                        }
                        sail_parser::TerminationMeasureKind::Loop { measures } => {
                            for measure in measures {
                                match &measure.0 {
                                    sail_parser::LoopMeasure::Until(expr)
                                    | sail_parser::LoopMeasure::Repeat(expr)
                                    | sail_parser::LoopMeasure::While(expr) => {
                                        self.analyze_expr(expr);
                                    }
                                }
                            }
                        }
                    }
                    self.tracker.pop_scope(self.file, &mut self.diagnostics);
                }
                _ => {}
            }
        }
    }

    fn analyze_expr(&mut self, expr: &(AstExpr, sail_parser::Span)) -> bool {
        match &expr.0 {
            AstExpr::Attribute { expr, .. }
            | AstExpr::Prefix { expr, .. }
            | AstExpr::Cast { expr, .. }
            | AstExpr::Field { expr, .. } => self.analyze_expr(expr),
            AstExpr::Assign { lhs, rhs } => self.analyze_lexp(lhs) || self.analyze_expr(rhs),
            AstExpr::Infix { lhs, rhs, .. } => self.analyze_expr(lhs) || self.analyze_expr(rhs),
            AstExpr::Let { binding, body } => self.analyze_let_expr(binding, body),
            AstExpr::Var {
                target,
                value,
                body,
            } => self.analyze_var_expr(target, value, body),
            AstExpr::Block(items) => self.analyze_block(items),
            AstExpr::Return(expr) | AstExpr::Throw(expr) => {
                self.analyze_expr(expr);
                true
            }
            AstExpr::Assert { condition, message } => {
                let mut terminates = self.analyze_expr(condition);
                if let Some(message) = message {
                    terminates |= self.analyze_expr(message);
                }
                terminates
            }
            AstExpr::Exit(expr) => {
                if let Some(expr) = expr {
                    self.analyze_expr(expr);
                }
                true
            }
            AstExpr::If {
                cond,
                then_branch,
                else_branch,
            } => self.analyze_if(cond, then_branch, else_branch.as_deref()),
            AstExpr::Match { scrutinee, cases } | AstExpr::Try { scrutinee, cases } => {
                self.analyze_match_like(scrutinee, cases)
            }
            AstExpr::Foreach(foreach) => {
                self.analyze_expr(&foreach.body);
                false
            }
            AstExpr::Repeat {
                measure,
                body,
                until,
            } => self.analyze_repeat(measure.as_deref(), body, until),
            AstExpr::While {
                measure,
                cond,
                body,
            } => self.analyze_while(measure.as_deref(), cond, body),
            AstExpr::Ident(name) => {
                self.tracker.mark_used(name);
                false
            }
            AstExpr::Ref(name) => {
                self.tracker.mark_used(&name.0);
                false
            }
            AstExpr::Call(call) => {
                let mut terminates = self.analyze_expr(&call.callee);
                for arg in &call.args {
                    terminates |= self.analyze_expr(arg);
                }
                terminates
            }
            AstExpr::Struct { fields, .. } => {
                let mut terminates = false;
                for field in fields {
                    terminates |= self.analyze_field_expr(field);
                }
                terminates
            }
            AstExpr::Update { base, fields } => {
                let mut terminates = self.analyze_expr(base);
                for field in fields {
                    terminates |= self.analyze_field_expr(field);
                }
                terminates
            }
            AstExpr::List(items) | AstExpr::Vector(items) | AstExpr::Tuple(items) => {
                let mut terminates = false;
                for item in items {
                    terminates |= self.analyze_expr(item);
                }
                terminates
            }
            AstExpr::Literal(_)
            | AstExpr::TypeVar(_)
            | AstExpr::Config(_)
            | AstExpr::SizeOf(_)
            | AstExpr::Constraint(_)
            | AstExpr::Error(_) => false,
        }
    }

    fn analyze_block(&mut self, items: &[(AstBlockItem, sail_parser::Span)]) -> bool {
        self.tracker.push_scope();

        let mut terminates = false;
        let mut unreachable_start = None;
        let mut unreachable_end = None;

        for item in items {
            if terminates {
                unreachable_start.get_or_insert(item.1.start);
                unreachable_end = Some(item.1.end);
                continue;
            }

            terminates = match &item.0 {
                AstBlockItem::Let(binding) => {
                    let value_terminates = self.analyze_expr(&binding.value);
                    if !value_terminates {
                        self.define_pattern_bindings(&binding.pattern, true);
                    }
                    value_terminates
                }
                AstBlockItem::Var { target, value } => {
                    let value_terminates = self.analyze_expr(value);
                    if !value_terminates {
                        self.analyze_decl_target(target);
                        self.define_target_binding(target, true);
                    }
                    value_terminates
                }
                AstBlockItem::Expr(expr) => self.analyze_expr(expr),
            };
        }

        if let (Some(start), Some(end)) = (unreachable_start, unreachable_end) {
            self.push_unreachable_diagnostic(sail_parser::Span::new(start, end));
        }

        self.tracker.pop_scope(self.file, &mut self.diagnostics);
        terminates
    }

    fn analyze_let_expr(
        &mut self,
        binding: &sail_parser::LetBinding,
        body: &(AstExpr, sail_parser::Span),
    ) -> bool {
        let value_terminates = self.analyze_expr(&binding.value);
        if value_terminates {
            self.push_unreachable_diagnostic(body.1);
            return true;
        }

        self.tracker.push_scope();
        self.define_pattern_bindings(&binding.pattern, true);
        let body_terminates = self.analyze_expr(body);
        self.tracker.pop_scope(self.file, &mut self.diagnostics);
        body_terminates
    }

    fn analyze_var_expr(
        &mut self,
        target: &(AstLexp, sail_parser::Span),
        value: &(AstExpr, sail_parser::Span),
        body: &(AstExpr, sail_parser::Span),
    ) -> bool {
        let value_terminates = self.analyze_expr(value);
        if value_terminates {
            self.push_unreachable_diagnostic(body.1);
            return true;
        }

        self.tracker.push_scope();
        self.analyze_decl_target(target);
        self.define_target_binding(target, true);
        let body_terminates = self.analyze_expr(body);
        self.tracker.pop_scope(self.file, &mut self.diagnostics);
        body_terminates
    }

    fn analyze_decl_target(&mut self, lexp: &(AstLexp, sail_parser::Span)) -> bool {
        match &lexp.0 {
            AstLexp::Attribute { lexp, .. } | AstLexp::Typed { lexp, .. } => {
                self.analyze_decl_target(lexp)
            }
            AstLexp::Id(_) => false,
            _ => self.analyze_lexp(lexp),
        }
    }

    fn analyze_lexp(&mut self, lexp: &(AstLexp, sail_parser::Span)) -> bool {
        match &lexp.0 {
            AstLexp::Attribute { lexp, .. } => self.analyze_lexp(lexp),
            // B3: Upstream Sail warns about redundant type annotations on
            // assignments to registers or mutable locals (type_check.ml:2040).
            AstLexp::Typed { lexp: inner, .. } => {
                if let AstLexp::Id(name) = &inner.0 {
                    if self.registers.contains(name) || self.tracker.is_mutable(name) {
                        self.diagnostics.push(diagnostic_for_warning(
                            self.file,
                            DiagnosticCode::RedundantTypeAnnotation,
                            lexp.1,
                            Message::line(format!(
                                "Redundant type annotation on assignment to '{}'. Type is already known.",
                                name
                            )),
                        ));
                    }
                }
                self.analyze_lexp(inner)
            }
            AstLexp::Id(name) => {
                self.tracker.mark_used(name);
                self.tracker.mark_modified(name);
                false
            }
            AstLexp::Deref(expr) => self.analyze_expr(expr),
            AstLexp::Call(call) => {
                let mut terminates = self.analyze_expr(&call.callee);
                for arg in &call.args {
                    terminates |= self.analyze_expr(arg);
                }
                terminates
            }
            AstLexp::Field { lexp, .. } => self.analyze_lexp(lexp),
            AstLexp::Vector { lexp, index } => self.analyze_lexp(lexp) || self.analyze_expr(index),
            AstLexp::VectorRange { lexp, start, end } => {
                self.analyze_lexp(lexp) || self.analyze_expr(start) || self.analyze_expr(end)
            }
            AstLexp::VectorConcat(items) | AstLexp::Tuple(items) => {
                let mut terminates = false;
                for item in items {
                    terminates |= self.analyze_lexp(item);
                }
                terminates
            }
            AstLexp::Error(_) => false,
        }
    }

    fn analyze_if(
        &mut self,
        cond: &(AstExpr, sail_parser::Span),
        then_branch: &(AstExpr, sail_parser::Span),
        else_branch: Option<&(AstExpr, sail_parser::Span)>,
    ) -> bool {
        if self.analyze_expr(cond) {
            let unreachable_span = else_branch
                .map(|else_branch| sail_parser::Span::new(then_branch.1.start, else_branch.1.end))
                .unwrap_or(then_branch.1);
            self.push_unreachable_diagnostic(unreachable_span);
            return true;
        }

        let then_terminates = self.analyze_expr(then_branch);
        let else_terminates = else_branch.map(|expr| self.analyze_expr(expr));
        then_terminates && else_terminates.unwrap_or(false)
    }

    fn analyze_match_like(
        &mut self,
        scrutinee: &(AstExpr, sail_parser::Span),
        cases: &[(AstMatchCase, sail_parser::Span)],
    ) -> bool {
        if self.analyze_expr(scrutinee) {
            if let (Some(first), Some(last)) = (cases.first(), cases.last()) {
                self.push_unreachable_diagnostic(sail_parser::Span::new(first.1.start, last.1.end));
            }
            return true;
        }

        let mut all_terminate = !cases.is_empty();
        for case in cases {
            all_terminate &= self.analyze_case(case);
        }
        all_terminate
    }

    fn analyze_case(&mut self, case: &(AstMatchCase, sail_parser::Span)) -> bool {
        self.tracker.push_scope();
        self.define_pattern_bindings(&case.0.pattern, true);

        let terminates = if let Some(guard) = &case.0.guard {
            if self.analyze_expr(guard) {
                self.push_unreachable_diagnostic(case.0.body.1);
                true
            } else {
                self.analyze_expr(&case.0.body)
            }
        } else {
            self.analyze_expr(&case.0.body)
        };

        self.tracker.pop_scope(self.file, &mut self.diagnostics);
        terminates
    }

    fn analyze_repeat(
        &mut self,
        measure: Option<&(AstExpr, sail_parser::Span)>,
        body: &(AstExpr, sail_parser::Span),
        until: &(AstExpr, sail_parser::Span),
    ) -> bool {
        if let Some(measure) = measure {
            if self.analyze_expr(measure) {
                self.push_unreachable_diagnostic(sail_parser::Span::new(body.1.start, until.1.end));
                return true;
            }
        }

        if self.analyze_expr(body) {
            self.push_unreachable_diagnostic(until.1);
            return true;
        }

        self.analyze_expr(until)
    }

    fn analyze_while(
        &mut self,
        measure: Option<&(AstExpr, sail_parser::Span)>,
        cond: &(AstExpr, sail_parser::Span),
        body: &(AstExpr, sail_parser::Span),
    ) -> bool {
        if let Some(measure) = measure {
            if self.analyze_expr(measure) {
                self.push_unreachable_diagnostic(sail_parser::Span::new(cond.1.start, body.1.end));
                return true;
            }
        }

        if self.analyze_expr(cond) {
            self.push_unreachable_diagnostic(body.1);
            return true;
        }

        self.analyze_expr(body);
        false
    }

    fn analyze_field_expr(&mut self, field: &(AstFieldExpr, sail_parser::Span)) -> bool {
        match &field.0 {
            AstFieldExpr::Assignment { value, .. } => self.analyze_expr(value),
            AstFieldExpr::Shorthand(name) => {
                self.tracker.mark_used(&name.0);
                false
            }
        }
    }

    fn define_pattern_bindings(
        &mut self,
        pattern: &(AstPattern, sail_parser::Span),
        warn_unused: bool,
    ) {
        match &pattern.0 {
            AstPattern::Attribute { pattern: inner, .. } => {
                self.define_pattern_bindings(inner, warn_unused);
            }
            AstPattern::Ident(name) => {
                // B2: Upstream Sail warns when a bare identifier in a pattern
                // is also a union constructor (type_check.ml:2821).
                if self.union_constructors.contains(name) {
                    self.diagnostics.push(diagnostic_for_warning(
                        self.file,
                        DiagnosticCode::UnionConstructorInPattern,
                        pattern.1,
                        Message::line(format!(
                            "Identifier '{}' found in pattern is also a union constructor. \
                             Did you mean '{}()'?",
                            name, name
                        )),
                    ));
                }
                if !self.pattern_constants.contains(name) {
                    // In Sail, uppercase identifiers in patterns are typically
                    // enum constructors from other files. Don't track them as
                    // variable bindings to avoid false unused-variable warnings.
                    let is_likely_constructor =
                        name.starts_with(|c: char| c.is_ascii_uppercase());
                    if !is_likely_constructor {
                        self.tracker
                            .define_binding(name, pattern.1, warn_unused, false);
                    }
                }
            }
            AstPattern::Typed(inner, _) | AstPattern::AsType(inner, _) => {
                self.define_pattern_bindings(inner, warn_unused);
            }
            AstPattern::AsBinding { pattern, binding } => {
                self.define_pattern_bindings(pattern, warn_unused);
                self.tracker
                    .define_binding(&binding.0, binding.1, warn_unused, false);
            }
            AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
                for item in items {
                    self.define_pattern_bindings(item, warn_unused);
                }
            }
            AstPattern::App { args, .. } => {
                for arg in args {
                    self.define_pattern_bindings(arg, warn_unused);
                }
            }
            AstPattern::Struct { fields, .. } => {
                for field in fields {
                    match &field.0 {
                        AstFieldPattern::Binding { pattern, .. } => {
                            self.define_pattern_bindings(pattern, warn_unused);
                        }
                        AstFieldPattern::Shorthand(name) => {
                            self.tracker
                                .define_binding(&name.0, name.1, warn_unused, false);
                        }
                        AstFieldPattern::Wild(_) => {}
                    }
                }
            }
            AstPattern::Infix { lhs, rhs, .. } => {
                self.define_pattern_bindings(lhs, warn_unused);
                self.define_pattern_bindings(rhs, warn_unused);
            }
            AstPattern::Wild
            | AstPattern::Literal(_)
            | AstPattern::TypeVar(_)
            | AstPattern::Index { .. }
            | AstPattern::RangeIndex { .. }
            | AstPattern::Error(_) => {}
        }
    }

    fn define_target_binding(
        &mut self,
        target: &(AstLexp, sail_parser::Span),
        warn_unused: bool,
    ) -> bool {
        match &target.0 {
            AstLexp::Id(name) => {
                self.tracker
                    .define_binding(name, target.1, warn_unused, true);
                true
            }
            AstLexp::Attribute { lexp, .. } | AstLexp::Typed { lexp, .. } => {
                self.define_target_binding(lexp, warn_unused)
            }
            _ => false,
        }
    }

    fn push_unreachable_diagnostic(&mut self, span: sail_parser::Span) {
        self.diagnostics.push(unnecessary_warning(
            self.file,
            DiagnosticCode::UnreachableCode,
            span,
            Message::line("Unreachable code"),
            Severity::Hint,
        ));
    }
}

fn unmodified_mutable_diagnostic(file: &File, span: sail_parser::Span, name: &str) -> Diagnostic {
    unnecessary_warning(
        file,
        DiagnosticCode::UnmodifiedMutableVariable,
        span,
        Message::line(format!(
            "Variable `{name}` is declared as `var` but never modified; consider using `let`"
        )),
        Severity::WeakWarning,
    )
}

fn unused_binding_diagnostic(file: &File, span: sail_parser::Span, name: &str) -> Diagnostic {
    unnecessary_warning(
        file,
        DiagnosticCode::UnusedVariable,
        span,
        Message::line(format!("Unused variable: `{name}`")),
        Severity::Warning,
    )
}

pub(crate) fn compute_semantic_diagnostics(file: &File) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(parsed) = file.parsed() else {
        return diagnostics;
    };

    let mut is_scattered = std::collections::HashSet::<String>::new();
    for decl in &parsed.decls {
        if decl.scope == sail_parser::Scope::TopLevel && decl.is_scattered {
            is_scattered.insert(decl.name.clone());
        }
    }

    // Sail allows the same name in different namespaces (e.g. `type X` and `let X`).
    // Key on (name, kind_group) to avoid false positives.
    let mut seen_defs =
        std::collections::HashMap::<(String, u8), sail_parser::Span>::new();
    for decl in &parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel
            || decl.role != sail_parser::DeclRole::Definition
        {
            continue;
        }
        // Overloads are designed to be extended across multiple declarations.
        if decl.kind == sail_parser::DeclKind::Overload {
            continue;
        }

        // Sail allows the same name across different namespaces:
        // - types vs values (e.g. `type flen` and `let flen`)
        // - val spec vs function def (e.g. `val f : T` then `function f(...)`)
        // - scattered definitions (handled above via is_scattered)
        let kind_group = match decl.kind {
            sail_parser::DeclKind::Type
            | sail_parser::DeclKind::Enum
            | sail_parser::DeclKind::Union
            | sail_parser::DeclKind::Bitfield
            | sail_parser::DeclKind::Newtype => 0, // type namespace
            sail_parser::DeclKind::Value => 2,     // val specs (paired with function defs)
            _ => 1,                                // value namespace
        };

        if let Some(prev_span) = seen_defs.get(&(decl.name.clone(), kind_group)) {
            if !is_scattered.contains(&decl.name) {
                let prev_pos = file.source.position_at(prev_span.start);
                let error = TypeError::Alternate {
                    primary: Box::new(TypeError::other(format!(
                        "Duplicate definition of `{}` (previously defined at line {})",
                        decl.name,
                        prev_pos.line + 1
                    ))),
                    reasons: vec![(
                        "previous definition".to_string(),
                        *prev_span,
                        Box::new(TypeError::Hint("definition here".to_string())),
                    )],
                };
                diagnostics.push(diagnostic_for_error(
                    file,
                    DiagnosticCode::DuplicateDefinition,
                    ReportingError::Type {
                        span: decl.span,
                        error,
                    },
                ));
            }
        } else {
            seen_defs.insert((decl.name.clone(), kind_group), decl.span);
        }
    }

    // B1: Scattered definitions with no clauses.
    // NOTE: This check is disabled because scattered definitions and their
    // clauses are typically spread across multiple files. In single-file
    // analysis mode, we cannot reliably detect missing clauses without
    // producing false positives. This should be re-enabled when multi-file
    // analysis (workspace-level diagnostics) is implemented.

    if let Some(ast) = file.core_ast() {
        // B4: Option register without default value.
        for (item, _) in &ast.defs {
            if let CoreDefinitionKind::Named(def) = &item.kind {
                if def.kind == sail_parser::core_ast::NamedDefKind::Register && def.value.is_none()
                {
                    let is_option_type = match &def.ty {
                        Some(ty) => match &ty.0 {
                            sail_parser::core_ast::TypeExpr::App { callee, .. } => {
                                callee.0 == "option" || callee.0 == "Option"
                            }
                            _ => false,
                        },
                        None => false,
                    };
                    if is_option_type {
                        let span = def.name.1;
                        let range = tower_lsp::lsp_types::Range::new(
                            file.source.position_at(span.start),
                            file.source.position_at(span.end),
                        );
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::OptionRegisterNoDefault,
                            format!(
                                "Register '{}' of type option should explicitly be given a default value",
                                def.name.0
                            ),
                            range,
                            Severity::WeakWarning,
                        ));
                    }
                }
            }
        }

        let mut analyzer = AstSemanticAnalyzer::new(file);
        analyzer.analyze_source_file(ast);
        diagnostics.extend(analyzer.finish());
    }

    diagnostics
}
