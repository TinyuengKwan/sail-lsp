use std::collections::HashMap;

use crate::diagnostics::{Diagnostic, DiagnosticCode, Severity};
use crate::state::File;
use crate::symbols::{call_arg_count, collect_callable_signatures};
use sail_parser::{
    BlockItem as AstBlockItem, Expr as AstExpr, FieldExpr as AstFieldExpr,
    FieldPattern as AstFieldPattern, MatchCase as AstMatchCase, Pattern as AstPattern,
    TopLevelDef as AstTopLevelDef, VectorUpdate as AstVectorUpdate,
};

#[derive(Debug)]
struct BindingRecord {
    name: String,
    span: sail_parser::Span,
    used: bool,
    warn_unused: bool,
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
        }
    }

    fn define_binding(&mut self, name: &str, span: sail_parser::Span, warn_unused: bool) {
        let idx = self.bindings.len();
        self.bindings.push(BindingRecord {
            name: name.to_string(),
            span,
            used: false,
            warn_unused,
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
}

struct AstSemanticAnalyzer<'a> {
    file: &'a File,
    diagnostics: Vec<Diagnostic>,
    tracker: BindingTracker,
}

impl<'a> AstSemanticAnalyzer<'a> {
    fn new(file: &'a File) -> Self {
        Self {
            file,
            diagnostics: Vec::new(),
            tracker: BindingTracker::default(),
        }
    }

    fn finish(self) -> Vec<Diagnostic> {
        self.diagnostics
    }

    fn analyze_source_file(&mut self, ast: &sail_parser::SourceFile) {
        for (item, _) in &ast.items {
            match item {
                AstTopLevelDef::CallableDef(def) => {
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
                        }
                    }
                    self.tracker.pop_scope(self.file, &mut self.diagnostics);
                }
                AstTopLevelDef::Named(def) => {
                    let Some(value) = &def.value else {
                        continue;
                    };
                    self.tracker.push_scope();
                    self.analyze_expr(value);
                    self.tracker.pop_scope(self.file, &mut self.diagnostics);
                }
                AstTopLevelDef::Directive(_) => {}
                AstTopLevelDef::TerminationMeasure(def) => {
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
            AstExpr::Assign { lhs, rhs } | AstExpr::Infix { lhs, rhs, .. } => {
                self.analyze_expr(lhs) || self.analyze_expr(rhs)
            }
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
            AstExpr::Index { expr, index } => self.analyze_expr(expr) || self.analyze_expr(index),
            AstExpr::Slice { expr, start, end } => {
                self.analyze_expr(expr) || self.analyze_expr(start) || self.analyze_expr(end)
            }
            AstExpr::VectorSlice {
                expr,
                start,
                length,
            } => self.analyze_expr(expr) || self.analyze_expr(start) || self.analyze_expr(length),
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
            AstExpr::VectorUpdate { base, updates } => {
                let mut terminates = self.analyze_expr(base);
                for update in updates {
                    terminates |= self.analyze_vector_update(update);
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
        target: &(AstExpr, sail_parser::Span),
        value: &(AstExpr, sail_parser::Span),
        body: &(AstExpr, sail_parser::Span),
    ) -> bool {
        let value_terminates = self.analyze_expr(value);
        if value_terminates {
            self.push_unreachable_diagnostic(body.1);
            return true;
        }

        self.tracker.push_scope();
        self.define_target_binding(target, true);
        let body_terminates = self.analyze_expr(body);
        self.tracker.pop_scope(self.file, &mut self.diagnostics);
        body_terminates
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

    fn analyze_vector_update(&mut self, update: &(AstVectorUpdate, sail_parser::Span)) -> bool {
        match &update.0 {
            AstVectorUpdate::Assignment { target, value } => {
                self.analyze_expr(target) || self.analyze_expr(value)
            }
            AstVectorUpdate::RangeAssignment { start, end, value } => {
                self.analyze_expr(start) || self.analyze_expr(end) || self.analyze_expr(value)
            }
            AstVectorUpdate::Shorthand(name) => {
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
            AstPattern::Ident(name) => self.tracker.define_binding(name, pattern.1, warn_unused),
            AstPattern::Typed(inner, _) | AstPattern::AsType(inner, _) => {
                self.define_pattern_bindings(inner, warn_unused);
            }
            AstPattern::AsBinding { pattern, binding } => {
                self.define_pattern_bindings(pattern, warn_unused);
                self.tracker
                    .define_binding(&binding.0, binding.1, warn_unused);
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
                            self.tracker.define_binding(&name.0, name.1, warn_unused);
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
        target: &(AstExpr, sail_parser::Span),
        warn_unused: bool,
    ) -> bool {
        match &target.0 {
            AstExpr::Ident(name) => {
                self.tracker.define_binding(name, target.1, warn_unused);
                true
            }
            AstExpr::Cast { expr, .. } => self.define_target_binding(expr, warn_unused),
            _ => false,
        }
    }

    fn push_unreachable_diagnostic(&mut self, span: sail_parser::Span) {
        self.diagnostics.push(
            Diagnostic::new(
                DiagnosticCode::UnreachableCode,
                "Unreachable code".to_string(),
                tower_lsp::lsp_types::Range::new(
                    self.file.source.position_at(span.start),
                    self.file.source.position_at(span.end),
                ),
                Severity::Hint,
            )
            .with_tags(vec![tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY]),
        );
    }
}

fn unused_binding_diagnostic(file: &File, span: sail_parser::Span, name: &str) -> Diagnostic {
    Diagnostic::new(
        DiagnosticCode::UnusedVariable,
        format!("Unused variable: `{}`", name),
        tower_lsp::lsp_types::Range::new(
            file.source.position_at(span.start),
            file.source.position_at(span.end),
        ),
        Severity::Warning,
    )
    .with_tags(vec![tower_lsp::lsp_types::DiagnosticTag::UNNECESSARY])
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

    let mut seen_defs = std::collections::HashMap::<String, sail_parser::Span>::new();
    for decl in &parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel
            || decl.role != sail_parser::DeclRole::Definition
        {
            continue;
        }

        if let Some(prev_span) = seen_defs.get(&decl.name) {
            if !is_scattered.contains(&decl.name) {
                let start = file.source.position_at(decl.span.start);
                let end = file.source.position_at(decl.span.end);
                let prev_pos = file.source.position_at(prev_span.start);
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::DuplicateDefinition,
                    format!(
                        "Duplicate definition of `{}` (previously defined at line {})",
                        decl.name,
                        prev_pos.line + 1
                    ),
                    tower_lsp::lsp_types::Range::new(start, end),
                    Severity::Error,
                ));
            }
        } else {
            seen_defs.insert(decl.name.clone(), decl.span);
        }
    }

    if let Some(ast) = file.ast() {
        let mut analyzer = AstSemanticAnalyzer::new(file);
        analyzer.analyze_source_file(ast);
        diagnostics.extend(analyzer.finish());
    }

    let mut signatures: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    for sig in collect_callable_signatures(file) {
        let total = sig.params.len();
        let required = sig.params.iter().filter(|p| !p.is_implicit).count();

        let entry = signatures.entry(sig.name).or_insert((required, total));
        let current_has_implicits = entry.0 < entry.1;
        let new_has_implicits = required < total;

        if new_has_implicits && !current_has_implicits {
            *entry = (required, total);
        } else if new_has_implicits == current_has_implicits {
            if total > entry.1 || (total == entry.1 && required < entry.0) {
                *entry = (required, total);
            }
        }
    }

    for call in &parsed.call_sites {
        if let Some(&(required, total)) = signatures.get(&call.callee) {
            let actual_args = call_arg_count(call);

            if actual_args < required || actual_args > total {
                let start = file.source.position_at(call.callee_span.start);
                let end = file.source.position_at(call.callee_span.end);
                let message = if required == total {
                    format!("Expected {} arguments, found {}", total, actual_args)
                } else {
                    format!(
                        "Expected {}-{} arguments, found {}",
                        required, total, actual_args
                    )
                };
                diagnostics.push(Diagnostic::new(
                    DiagnosticCode::MismatchedArgCount,
                    message,
                    tower_lsp::lsp_types::Range::new(start, end),
                    Severity::Error,
                ));
            }
        }
    }

    diagnostics
}
