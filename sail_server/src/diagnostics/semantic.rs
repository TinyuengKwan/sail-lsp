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
    fn new_with_extra(
        file: &'a File,
        extra_pattern_constants: &HashSet<String>,
        extra_union_constructors: &HashSet<String>,
    ) -> Self {
        let mut pattern_constants: HashSet<String> = file
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
            .unwrap_or_default();
        pattern_constants.extend(extra_pattern_constants.iter().cloned());

        let mut union_constructors: HashSet<String> = file
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
            .unwrap_or_default();
        union_constructors.extend(extra_union_constructors.iter().cloned());

        Self {
            file,
            diagnostics: Vec::new(),
            pattern_constants,
            union_constructors,
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
                // Match upstream Sail's `E_for` handling in
                // `sail/src/lib/rewriter.ml::e_for`, which joins used-id
                // sets from start, end, step, AND body. Previously we
                // only walked the body, so variables only referenced in
                // the loop bounds (e.g. `foreach (i from eg_start to
                // eg_len - 1)`) were incorrectly flagged as unused.
                self.analyze_expr(&foreach.start);
                self.analyze_expr(&foreach.end);
                if let Some(step) = &foreach.step {
                    self.analyze_expr(step);
                }
                // Introduce the loop iterator in its own scope so the
                // body can reference it. Upstream's `lint.ml` never
                // reports the iterator as unused (it's not collected via
                // a pattern — `E_for`'s bound id is skipped entirely),
                // so mark it as warn_unused=false.
                self.tracker.push_scope();
                self.tracker
                    .define_binding(&foreach.iterator.0, foreach.iterator.1, false, false);
                self.analyze_expr(&foreach.body);
                self.tracker.pop_scope(self.file, &mut self.diagnostics);
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
        // Match-pattern bare identifiers may be enum constructors from other files.
        // Without cross-file type info we cannot reliably distinguish bindings from
        // constructors (upstream lint.ml line 154 uses Type_check.is_enum_member,
        // which requires the global type environment). Disable unused-variable
        // warnings for match patterns to avoid false positives like `mRET`, `sRET`
        // in sail-riscv.
        self.define_pattern_bindings(&case.0.pattern, false);

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

/// Workspace-aware variant: aggregates enum members and union constructors
/// from all files so that bare-identifier patterns like `mRET`, `Data`, etc.
/// are correctly recognized as constructors instead of being reported as
/// unused bindings or duplicate bindings.
pub(crate) fn compute_semantic_diagnostics_with_workspace<'a, I>(
    file: &File,
    all_files: I,
) -> Vec<Diagnostic>
where
    I: IntoIterator<Item = &'a File>,
{
    let mut extra_pattern_constants: HashSet<String> = HashSet::new();
    let mut extra_union_constructors: HashSet<String> = HashSet::new();
    let mut extra_enum_members: HashMap<String, Vec<String>> = HashMap::new();
    let mut extra_union_variants: HashMap<String, Vec<String>> = HashMap::new();

    for other in all_files {
        if let Some(parsed) = other.parsed() {
            for decl in &parsed.decls {
                if decl.scope == sail_parser::Scope::TopLevel
                    && decl.kind == sail_parser::DeclKind::EnumMember
                {
                    extra_pattern_constants.insert(decl.name.clone());
                }
            }
        }
        if let Some(ast) = other.core_ast() {
            for (item, _) in &ast.defs {
                if let CoreDefinitionKind::Named(def) = &item.kind {
                    match &def.detail {
                        Some(sail_parser::NamedDefDetail::Union { variants, .. }) => {
                            let variant_names: Vec<String> =
                                variants.iter().map(|(v, _)| v.name.0.clone()).collect();
                            extra_union_variants
                                .insert(def.name.0.clone(), variant_names.clone());
                            for variant in variants {
                                extra_union_constructors.insert(variant.0.name.0.clone());
                            }
                        }
                        Some(sail_parser::NamedDefDetail::Enum { members, .. }) => {
                            let names: Vec<String> =
                                members.iter().map(|(m, _)| m.name.0.clone()).collect();
                            extra_enum_members.insert(def.name.0.clone(), names.clone());
                            for m in members {
                                extra_pattern_constants.insert(m.0.name.0.clone());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    compute_semantic_diagnostics_impl(
        file,
        &extra_pattern_constants,
        &extra_union_constructors,
        &extra_enum_members,
        &extra_union_variants,
    )
}

pub(crate) fn compute_semantic_diagnostics(file: &File) -> Vec<Diagnostic> {
    compute_semantic_diagnostics_impl(
        file,
        &HashSet::new(),
        &HashSet::new(),
        &HashMap::new(),
        &HashMap::new(),
    )
}

fn compute_semantic_diagnostics_impl(
    file: &File,
    extra_pattern_constants: &HashSet<String>,
    extra_union_constructors: &HashSet<String>,
    extra_enum_members: &HashMap<String, Vec<String>>,
    extra_union_variants: &HashMap<String, Vec<String>>,
) -> Vec<Diagnostic> {
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

        let mut analyzer = AstSemanticAnalyzer::new_with_extra(
            file,
            extra_pattern_constants,
            extra_union_constructors,
        );
        analyzer.analyze_source_file(ast);
        diagnostics.extend(analyzer.finish());

        // Preprocessor directive validation (token-level)
        diagnostics.extend(check_preprocessor_directives(file));

        // Pattern completeness checking (with cross-file enum/union types)
        diagnostics.extend(check_pattern_completeness_with_extra(
            file,
            ast,
            extra_enum_members,
            extra_union_variants,
        ));

        // Mapping completeness checking
        diagnostics.extend(check_mapping_completeness(file, ast));

        // Scattered definition validation
        diagnostics.extend(check_scattered_completeness(file, ast, parsed));

        // Pragma validation
        diagnostics.extend(check_pragma_validity(file, ast));

        // Non-local control flow checks
        diagnostics.extend(check_nl_flow(file, ast));

        // Private visibility checks
        diagnostics.extend(check_private_visibility(file, ast));

        // Register type validation
        diagnostics.extend(check_register_types(file, ast));
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Pattern Completeness Checking
// ---------------------------------------------------------------------------

fn check_pattern_completeness_with_extra(
    file: &File,
    ast: &CoreSourceFile,
    extra_enum_members: &HashMap<String, Vec<String>>,
    extra_union_variants: &HashMap<String, Vec<String>>,
) -> Vec<Diagnostic> {
    use sail_parser::core_ast::{
        BlockItem, DefinitionKind, Expr, Pattern,
    };

    let mut diagnostics = Vec::new();

    fn check_expr(
        expr: &(Expr, sail_parser::Span),
        file: &File,
        diagnostics: &mut Vec<Diagnostic>,
        all_enum_members: &HashMap<String, Vec<String>>,
        all_union_variants: &HashMap<String, Vec<String>>,
    ) {
        match &expr.0 {
            Expr::Match { scrutinee, cases } => {
                // Collect pattern names
                let arm_names: Vec<Option<String>> = cases
                    .iter()
                    .map(|(case, _)| extract_arm_name(&case.pattern.0))
                    .collect();

                // Check for wildcard/catch-all
                let has_catchall = arm_names.iter().any(|n| n.is_none());

                if !has_catchall {
                    let named_arms: HashSet<String> = arm_names
                        .iter()
                        .filter_map(|n| n.clone())
                        .collect();

                    // Try to identify the type being matched
                    for (_type_name, members) in all_enum_members {
                        if named_arms.iter().any(|a| members.contains(a)) {
                            let missing: Vec<&String> = members
                                .iter()
                                .filter(|m| !named_arms.contains(*m))
                                .collect();
                            if !missing.is_empty() {
                                let range = tower_lsp::lsp_types::Range::new(
                                    file.source.position_at(expr.1.start),
                                    file.source.position_at(
                                        (expr.1.start + 10).min(expr.1.end),
                                    ),
                                );
                                let missing_str = missing
                                    .iter()
                                    .map(|m| m.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                diagnostics.push(Diagnostic::new(
                                    DiagnosticCode::IncompleteMatch,
                                    format!(
                                        "Non-exhaustive match: missing arm(s) for {}",
                                        missing_str
                                    ),
                                    range,
                                    Severity::Warning,
                                ));
                            }
                            break;
                        }
                    }
                    for (_type_name, variants) in all_union_variants {
                        if named_arms.iter().any(|a| variants.contains(a)) {
                            let missing: Vec<&String> = variants
                                .iter()
                                .filter(|v| !named_arms.contains(*v))
                                .collect();
                            if !missing.is_empty() {
                                let range = tower_lsp::lsp_types::Range::new(
                                    file.source.position_at(expr.1.start),
                                    file.source.position_at(
                                        (expr.1.start + 10).min(expr.1.end),
                                    ),
                                );
                                let missing_str = missing
                                    .iter()
                                    .map(|m| m.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                diagnostics.push(Diagnostic::new(
                                    DiagnosticCode::IncompleteMatch,
                                    format!(
                                        "Non-exhaustive match: missing arm(s) for {}",
                                        missing_str
                                    ),
                                    range,
                                    Severity::Warning,
                                ));
                            }
                            break;
                        }
                    }
                }

                // Check for redundant arms (duplicate patterns).
                // Use the full pattern source text as the key so that
                // `Load(Data)` and `Load(PageTableEntry)` are NOT confused.
                // Only flag patterns without guards (guards may differ).
                let text = file.source.text();
                let mut seen: HashSet<String> = HashSet::new();
                for (case, case_span) in cases {
                    if case.guard.is_some() {
                        continue;
                    }
                    let pat_span = case.pattern.1;
                    let Some(pat_text) = text.get(pat_span.start..pat_span.end) else {
                        continue;
                    };
                    let normalized: String = pat_text.split_whitespace().collect::<Vec<_>>().join(" ");
                    if normalized.is_empty() {
                        continue;
                    }
                    if !seen.insert(normalized.clone()) {
                        let range = tower_lsp::lsp_types::Range::new(
                            file.source.position_at(case_span.start),
                            file.source.position_at(case_span.end),
                        );
                        diagnostics.push(
                            Diagnostic::new(
                                DiagnosticCode::RedundantMatchArm,
                                format!("Redundant match arm: `{}` already handled", normalized),
                                range,
                                Severity::WeakWarning,
                            )
                            .with_tags(vec![
                                tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY,
                            ]),
                        );
                    }
                }

                // Recurse into match arms
                for (case, _) in cases {
                    check_expr(&case.body, file, diagnostics, all_enum_members, all_union_variants);
                }
                check_expr(scrutinee, file, diagnostics, all_enum_members, all_union_variants);
            }
            Expr::Block(items) => {
                for item in items {
                    match &item.0 {
                        BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                            check_expr(e, file, diagnostics, all_enum_members, all_union_variants);
                        }
                        BlockItem::Let(lb) => {
                            check_expr(&*lb.value, file, diagnostics, all_enum_members, all_union_variants);
                        }
                    }
                }
            }
            Expr::If { cond, then_branch, else_branch, .. } => {
                check_expr(cond, file, diagnostics, all_enum_members, all_union_variants);
                check_expr(then_branch, file, diagnostics, all_enum_members, all_union_variants);
                if let Some(e) = else_branch {
                    check_expr(e, file, diagnostics, all_enum_members, all_union_variants);
                }
            }
            Expr::Let { body, .. } | Expr::Var { body, .. } => {
                check_expr(body, file, diagnostics, all_enum_members, all_union_variants);
            }
            Expr::Foreach(f) => check_expr(&f.body, file, diagnostics, all_enum_members, all_union_variants),
            Expr::While { body, .. } | Expr::Repeat { body, .. } => {
                check_expr(body, file, diagnostics, all_enum_members, all_union_variants);
            }
            _ => {}
        }
    }

    fn extract_arm_name(pattern: &Pattern) -> Option<String> {
        match pattern {
            Pattern::Ident(name) => {
                // A lowercase-leading bare identifier is treated as a binding
                // (catch-all), not a constructor reference. This avoids
                // false-positive incomplete-match warnings on patterns like
                // `failure => ...` that catch all remaining cases.
                if name.chars().next().is_some_and(|c| c.is_ascii_lowercase()) {
                    None
                } else {
                    Some(name.clone())
                }
            }
            Pattern::App { callee, .. } => Some(callee.0.clone()),
            Pattern::Typed(inner, _) => extract_arm_name(&inner.0),
            Pattern::Attribute { pattern: inner, .. } => extract_arm_name(&inner.0),
            Pattern::Wild => None,
            _ => None,
        }
    }

    // Collect all enum members and union variants from this file plus extras
    let mut all_enum_members: HashMap<String, Vec<String>> = extra_enum_members.clone();
    let mut all_union_variants: HashMap<String, Vec<String>> = extra_union_variants.clone();

    for (def, _) in &ast.defs {
        if let DefinitionKind::Named(nd) = &def.kind {
            match &nd.detail {
                Some(sail_parser::core_ast::NamedDefDetail::Enum { members, .. }) => {
                    let names: Vec<String> = members.iter().map(|(m, _)| m.name.0.clone()).collect();
                    all_enum_members.insert(nd.name.0.clone(), names);
                }
                Some(sail_parser::core_ast::NamedDefDetail::Union { variants, .. }) => {
                    let names: Vec<String> = variants.iter().map(|(v, _)| v.name.0.clone()).collect();
                    all_union_variants.insert(nd.name.0.clone(), names);
                }
                _ => {}
            }
        }
    }

    // Walk all function bodies
    for (def, _) in &ast.defs {
        if let DefinitionKind::Callable(c) = &def.kind {
            if let Some(body) = &c.body {
                check_expr(body, file, &mut diagnostics, &all_enum_members, &all_union_variants);
            }
            for (clause, _) in &c.clauses {
                if let Some(body) = &clause.body {
                    check_expr(body, file, &mut diagnostics, &all_enum_members, &all_union_variants);
                }
            }
        }
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Scattered Definition Validation
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Mapping Completeness Checking
// ---------------------------------------------------------------------------

fn check_mapping_completeness(
    file: &File,
    ast: &CoreSourceFile,
) -> Vec<Diagnostic> {
    // Sail mapping clauses (`mapping clause`) are commonly distributed across
    // many files for scattered mappings. A file may legitimately contain only
    // forwards or only backwards arms, with the complementary direction in
    // another file. Without workspace-level analysis, single-file checks
    // produce false positives. Disable this check.
    let _ = (file, ast);
    return Vec::new();

    #[allow(unreachable_code)]
    let _diagnostics: Vec<Diagnostic> = Vec::new();
    use sail_parser::core_ast::{
        DefinitionKind, MappingArmDirection,
    };

    let mut diagnostics = Vec::new();

    for (def, _def_span) in &ast.defs {
        if let DefinitionKind::Callable(c) = &def.kind {
            if let Some(mapping_body) = &c.mapping_body {
                let has_forwards = mapping_body.arms.iter().any(|(arm, _)| {
                    matches!(
                        arm.direction,
                        MappingArmDirection::Forwards | MappingArmDirection::Bidirectional
                    )
                });
                let has_backwards = mapping_body.arms.iter().any(|(arm, _)| {
                    matches!(
                        arm.direction,
                        MappingArmDirection::Backwards | MappingArmDirection::Bidirectional
                    )
                });

                if !has_forwards && !has_backwards {
                    continue; // No arms at all, probably incomplete
                }

                if has_forwards && !has_backwards {
                    let range = tower_lsp::lsp_types::Range::new(
                        file.source.position_at(c.name.1.start),
                        file.source.position_at(c.name.1.end),
                    );
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::IncompleteMatch,
                        format!(
                            "Mapping `{}` has forwards arms but no backwards arms",
                            c.name.0
                        ),
                        range,
                        Severity::WeakWarning,
                    ));
                } else if !has_forwards && has_backwards {
                    let range = tower_lsp::lsp_types::Range::new(
                        file.source.position_at(c.name.1.start),
                        file.source.position_at(c.name.1.end),
                    );
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::IncompleteMatch,
                        format!(
                            "Mapping `{}` has backwards arms but no forwards arms",
                            c.name.0
                        ),
                        range,
                        Severity::WeakWarning,
                    ));
                }
            }
        }
    }

    diagnostics
}

fn check_scattered_completeness(
    file: &File,
    ast: &CoreSourceFile,
    _parsed: &sail_parser::ParsedFile,
) -> Vec<Diagnostic> {
    // Scattered definitions in real Sail projects (e.g., sail-riscv) are
    // commonly split across many files: the declaration in one file, the
    // clauses in extension files, the `end` in a finalize file. Without
    // workspace-level analysis we cannot reliably check completeness from a
    // single file. Disable this check to avoid false positives.
    let _ = (file, ast);
    return Vec::new();

    #[allow(unreachable_code)]
    let _diagnostics: Vec<Diagnostic> = Vec::new();
    use sail_parser::core_ast::DefinitionKind;

    let mut diagnostics = Vec::new();

    // Find scattered declarations and check for matching end/clauses
    let mut scattered_funcs = HashMap::<String, sail_parser::Span>::new();
    let mut scattered_has_clause = HashSet::<String>::new();
    let mut scattered_has_end = HashSet::<String>::new();

    for (def, _def_span) in &ast.defs {
        match &def.kind {
            DefinitionKind::Scattered(sd) => {
                scattered_funcs.insert(sd.name.0.clone(), sd.name.1);
            }
            DefinitionKind::ScatteredClause(sc) => {
                scattered_has_clause.insert(sc.name.0.clone());
            }
            DefinitionKind::End(ed) => {
                scattered_has_end.insert(ed.name.0.clone());
            }
            _ => {}
        }
    }

    for (name, span) in &scattered_funcs {
        if !scattered_has_clause.contains(name) {
            let range = tower_lsp::lsp_types::Range::new(
                file.source.position_at(span.start),
                file.source.position_at(span.end),
            );
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::IncompleteScattered,
                format!("Scattered definition `{name}` has no clauses in this file"),
                range,
                Severity::WeakWarning,
            ));
        }
        if !scattered_has_end.contains(name) {
            let range = tower_lsp::lsp_types::Range::new(
                file.source.position_at(span.start),
                file.source.position_at(span.end),
            );
            diagnostics.push(Diagnostic::new(
                DiagnosticCode::IncompleteScattered,
                format!("Scattered definition `{name}` has no `end` in this file"),
                range,
                Severity::WeakWarning,
            ));
        }
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Pragma Validation
// ---------------------------------------------------------------------------

/// Known Sail pragmas (from upstream pragma.ml).
pub(crate) const KNOWN_PRAGMAS: &[&str] = &[
    "define",
    "anchor",
    "span",
    "include",
    "include_error",
    "ifdef",
    "ifndef",
    "iftarget",
    "else",
    "endif",
    "option",
    "optimize",
    "latex",
    "property",
    "counterexample",
    "suppress_warnings",
    "include_start",
    "include_end",
    "sail_internal",
    "target_set",
    "non_exec",
    // Sail attributes also commonly used
    "no_enum_number_conversions",
    "undefined_gen",
    "incomplete",
    "no_warn",
    "deprecated",
    "fold",
    "complete",
    "format",
    "infallible",
    "pure",
];

fn check_pragma_validity(file: &File, ast: &CoreSourceFile) -> Vec<Diagnostic> {
    use sail_parser::core_ast::DefinitionKind;
    let mut diagnostics = Vec::new();

    // Upstream behavior (preprocess.ml line 282-284):
    // - Only `$directive` (DEF_pragma) lines get the unrecognised warning.
    // - `$[attribute ...]` annotations are arbitrary metadata and never warned about.
    // So we only check Directive definitions, not def.meta.attrs.
    for (def, _) in &ast.defs {
        if let DefinitionKind::Directive(d) = &def.kind {
            let name = &d.name.0;
            let name_no_dollar = name.strip_prefix('$').unwrap_or(name);
            // Skip names containing '#' (matches upstream `String.contains p '#'` exception)
            if name_no_dollar.contains('#') {
                continue;
            }
            if !KNOWN_PRAGMAS.contains(&name_no_dollar) {
                let range = tower_lsp::lsp_types::Range::new(
                    file.source.position_at(d.name.1.start),
                    file.source.position_at(d.name.1.end),
                );
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::UnknownDirective,
                    format!("Unrecognised directive `${name_no_dollar}`"),
                    range,
                    Severity::WeakWarning,
                ));
            }
        }
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Preprocessor Directive Validation (token-level)
// ---------------------------------------------------------------------------

fn check_preprocessor_directives(file: &File) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let Some(tokens) = file.tokens.as_deref() else {
        return diagnostics;
    };

    // Track $ifdef/$ifndef nesting
    let mut directive_stack: Vec<(String, sail_parser::Span)> = Vec::new();

    for (token, span) in tokens.iter() {
        if let sail_parser::Token::Directive { name, .. } = token {
            match name.as_str() {
                "ifdef" | "ifndef" | "iftarget" => {
                    directive_stack.push((name.clone(), *span));
                }
                "endif" => {
                    if directive_stack.pop().is_none() {
                        let range = tower_lsp::lsp_types::Range::new(
                            file.source.position_at(span.start),
                            file.source.position_at(span.end),
                        );
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::UnclosedDirective,
                            "`$endif` without matching `$ifdef`/`$ifndef`/`$iftarget`".to_string(),
                            range,
                            Severity::Warning,
                        ));
                    }
                }
                "else" => {
                    if directive_stack.is_empty() {
                        let range = tower_lsp::lsp_types::Range::new(
                            file.source.position_at(span.start),
                            file.source.position_at(span.end),
                        );
                        diagnostics.push(Diagnostic::new(
                            DiagnosticCode::UnclosedDirective,
                            "`$else` without matching `$ifdef`/`$ifndef`".to_string(),
                            range,
                            Severity::Warning,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    // Any unclosed directives left on stack
    for (name, span) in directive_stack {
        let range = tower_lsp::lsp_types::Range::new(
            file.source.position_at(span.start),
            file.source.position_at(span.end),
        );
        diagnostics.push(Diagnostic::new(
            DiagnosticCode::UnclosedDirective,
            format!("Unclosed `${name}` directive (missing `$endif`)"),
            range,
            Severity::Warning,
        ));
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Non-Local Control Flow Diagnostics
// ---------------------------------------------------------------------------

/// Detect statements after a guaranteed escape (throw, return, exit) in a block.
fn check_nl_flow(file: &File, ast: &CoreSourceFile) -> Vec<Diagnostic> {
    use sail_parser::core_ast::{BlockItem, DefinitionKind, Expr};

    let mut diagnostics = Vec::new();

    fn always_escapes(expr: &Expr) -> bool {
        match expr {
            Expr::Return(_) | Expr::Throw(_) | Expr::Exit(_) => true,
            Expr::Block(items) => items
                .last()
                .map(|item| match &item.0 {
                    BlockItem::Expr(e) => always_escapes(&e.0),
                    _ => false,
                })
                .unwrap_or(false),
            Expr::If {
                then_branch,
                else_branch: Some(eb),
                ..
            } => always_escapes(&then_branch.0) && always_escapes(&eb.0),
            Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
                !cases.is_empty()
                    && cases
                        .iter()
                        .all(|(c, _)| always_escapes(&c.body.0))
            }
            _ => false,
        }
    }

    fn walk(
        expr: &(Expr, sail_parser::Span),
        file: &File,
        diagnostics: &mut Vec<Diagnostic>,
    ) {
        match &expr.0 {
            Expr::Block(items) => {
                let mut escaped_at: Option<usize> = None;
                for (i, item) in items.iter().enumerate() {
                    if let Some(esc_idx) = escaped_at {
                        // The current item is unreachable
                        let range = tower_lsp::lsp_types::Range::new(
                            file.source.position_at(item.1.start),
                            file.source.position_at(item.1.end),
                        );
                        diagnostics.push(
                            Diagnostic::new(
                                DiagnosticCode::UnreachableAfterEscape,
                                "Unreachable code after non-local control flow".to_string(),
                                range,
                                Severity::WeakWarning,
                            )
                            .with_tags(vec![
                                tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY,
                            ]),
                        );
                        let _ = esc_idx;
                        break;
                    }
                    match &item.0 {
                        BlockItem::Expr(e) => {
                            walk(e, file, diagnostics);
                            if always_escapes(&e.0) {
                                escaped_at = Some(i);
                            }
                        }
                        BlockItem::Var { value, .. } => walk(value, file, diagnostics),
                        BlockItem::Let(lb) => walk(&*lb.value, file, diagnostics),
                    }
                }
            }
            Expr::If { cond, then_branch, else_branch, .. } => {
                walk(cond, file, diagnostics);
                walk(then_branch, file, diagnostics);
                if let Some(e) = else_branch {
                    walk(e, file, diagnostics);
                }
            }
            Expr::Match { scrutinee, cases, .. } | Expr::Try { scrutinee, cases, .. } => {
                walk(scrutinee, file, diagnostics);
                for (case, _) in cases {
                    walk(&case.body, file, diagnostics);
                }
            }
            Expr::Let { binding, body } => {
                walk(&*binding.value, file, diagnostics);
                walk(body, file, diagnostics);
            }
            Expr::Var { value, body, .. } => {
                walk(value, file, diagnostics);
                walk(body, file, diagnostics);
            }
            Expr::Foreach(f) => walk(&f.body, file, diagnostics),
            Expr::While { cond, body, .. } => {
                walk(cond, file, diagnostics);
                walk(body, file, diagnostics);
            }
            Expr::Repeat { body, until, .. } => {
                walk(body, file, diagnostics);
                walk(until, file, diagnostics);
            }
            _ => {}
        }
    }

    for (def, _) in &ast.defs {
        if let DefinitionKind::Callable(c) = &def.kind {
            if let Some(body) = &c.body {
                walk(body, file, &mut diagnostics);
            }
            for (clause, _) in &c.clauses {
                if let Some(body) = &clause.body {
                    walk(body, file, &mut diagnostics);
                }
            }
        }
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Private Visibility Checking
// ---------------------------------------------------------------------------

fn check_private_visibility(file: &File, ast: &CoreSourceFile) -> Vec<Diagnostic> {
    use sail_parser::core_ast::{BlockItem, DefinitionKind, Expr};

    // Sail's `private` keyword is MODULE-scoped, not symbol-scoped.
    // Calls within the same file/module to private functions are allowed.
    // Without a workspace module model, we cannot reliably detect cross-module
    // accesses. Disable per-file checking to avoid false positives.
    // (Reference: sail/src/lib/project.ml shows visibility is module-scoped.)
    let _ = file;
    let _ = ast;
    return Vec::new();

    #[allow(unreachable_code)]
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Collect names of private definitions and the function they're declared in
    let mut private_funcs: HashSet<String> = HashSet::new();
    for (def, _) in &ast.defs {
        if def.meta.is_private {
            if let DefinitionKind::Callable(c) = &def.kind {
                private_funcs.insert(c.name.0.clone());
            }
            if let DefinitionKind::CallableSpec(s) = &def.kind {
                private_funcs.insert(s.name.0.clone());
            }
        }
    }

    if private_funcs.is_empty() {
        return diagnostics;
    }

    // For each non-private callable, check its body for calls to private funcs
    // declared by other definitions (i.e., not the same function recursively)
    fn collect_calls(
        expr: &(Expr, sail_parser::Span),
        calls: &mut Vec<(String, sail_parser::Span)>,
    ) {
        match &expr.0 {
            Expr::Call(c) => {
                if let Expr::Ident(name) = &c.callee.0 {
                    calls.push((name.clone(), c.callee.1));
                }
                for arg in &c.args {
                    collect_calls(arg, calls);
                }
            }
            Expr::Block(items) => {
                for item in items {
                    match &item.0 {
                        BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                            collect_calls(e, calls);
                        }
                        BlockItem::Let(lb) => collect_calls(&*lb.value, calls),
                    }
                }
            }
            Expr::If { cond, then_branch, else_branch, .. } => {
                collect_calls(cond, calls);
                collect_calls(then_branch, calls);
                if let Some(e) = else_branch {
                    collect_calls(e, calls);
                }
            }
            Expr::Match { scrutinee, cases, .. } | Expr::Try { scrutinee, cases, .. } => {
                collect_calls(scrutinee, calls);
                for (c, _) in cases {
                    collect_calls(&c.body, calls);
                }
            }
            Expr::Let { binding, body } => {
                collect_calls(&*binding.value, calls);
                collect_calls(body, calls);
            }
            Expr::Var { value, body, .. } => {
                collect_calls(value, calls);
                collect_calls(body, calls);
            }
            Expr::Foreach(f) => collect_calls(&f.body, calls),
            Expr::While { cond, body, .. } => {
                collect_calls(cond, calls);
                collect_calls(body, calls);
            }
            Expr::Repeat { body, until, .. } => {
                collect_calls(body, calls);
                collect_calls(until, calls);
            }
            Expr::Infix { lhs, rhs, .. } => {
                collect_calls(lhs, calls);
                collect_calls(rhs, calls);
            }
            Expr::Prefix { expr: e, .. }
            | Expr::Cast { expr: e, .. }
            | Expr::Field { expr: e, .. }
            | Expr::Return(e)
            | Expr::Throw(e) => collect_calls(e, calls),
            _ => {}
        }
    }

    for (def, _) in &ast.defs {
        if def.meta.is_private {
            continue;
        }
        if let DefinitionKind::Callable(c) = &def.kind {
            let mut calls = Vec::new();
            if let Some(body) = &c.body {
                collect_calls(body, &mut calls);
            }
            for (clause, _) in &c.clauses {
                if let Some(body) = &clause.body {
                    collect_calls(body, &mut calls);
                }
            }
            for (callee_name, callee_span) in calls {
                if private_funcs.contains(&callee_name) && callee_name != c.name.0 {
                    let range = tower_lsp::lsp_types::Range::new(
                        file.source.position_at(callee_span.start),
                        file.source.position_at(callee_span.end),
                    );
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::PrivateAccess,
                        format!(
                            "Public function `{}` calls private function `{}`",
                            c.name.0, callee_name
                        ),
                        range,
                        Severity::WeakWarning,
                    ));
                }
            }
        }
    }

    diagnostics
}

// ---------------------------------------------------------------------------
// Register Type Validation
// ---------------------------------------------------------------------------

fn check_register_types(file: &File, ast: &CoreSourceFile) -> Vec<Diagnostic> {
    use sail_parser::core_ast::{DefinitionKind, NamedDefKind, TypeExpr};

    let mut diagnostics = Vec::new();

    fn is_supported_register_type(ty: &TypeExpr) -> Result<(), &'static str> {
        match ty {
            TypeExpr::Named(_) => Ok(()),
            TypeExpr::App { args, .. } => {
                for (arg, _) in args {
                    match arg {
                        TypeExpr::Literal(_)
                        | TypeExpr::Named(_)
                        | TypeExpr::TypeVar(_) => {}
                        TypeExpr::Infix { .. } => {
                            return Err(
                                "register type contains complex numeric expression in arguments",
                            );
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            TypeExpr::Tuple(_) => Ok(()),
            TypeExpr::Arrow { .. } => Err("register type cannot be a function type"),
            TypeExpr::Forall { .. } => Err("register type cannot be polymorphic"),
            TypeExpr::Existential { .. } => Err("register type cannot be existential"),
            TypeExpr::Effect { ty: inner, .. } => is_supported_register_type(&inner.0),
            _ => Ok(()),
        }
    }

    for (def, _) in &ast.defs {
        if let DefinitionKind::Named(nd) = &def.kind {
            if nd.kind != NamedDefKind::Register {
                continue;
            }
            if let Some(ty) = &nd.ty {
                if let Err(reason) = is_supported_register_type(&ty.0) {
                    let range = tower_lsp::lsp_types::Range::new(
                        file.source.position_at(ty.1.start),
                        file.source.position_at(ty.1.end),
                    );
                    diagnostics.push(Diagnostic::new(
                        DiagnosticCode::UnsupportedRegisterType,
                        format!("Register `{}` has unsupported type: {}", nd.name.0, reason),
                        range,
                        Severity::Warning,
                    ));
                }
            }
        }
    }

    diagnostics
}
