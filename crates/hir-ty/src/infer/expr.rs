use super::*;

/// Returns true if a function is likely from the Sail stdlib or auto-generated.
fn is_likely_external_or_generated(name: &str) -> bool {
    name.starts_with("num_of_")
        || name.starts_with("sail_")
        || name.starts_with("float_")
        || name.starts_with("update_")
        || name.starts_with("rvfi_")
        || name.ends_with("_forwards_matches")
        || name.ends_with("_backwards_matches")
        || (name.starts_with(|c: char| c.is_ascii_uppercase()) && name.contains('_'))
}

#[derive(Clone)]
struct FreshenEntry {
    fresh_name: String,
    orig_name: String,
    infer_ty: Ty,
}

/// Pre-filter for overload candidates (never rejects a valid match).
fn types_plausibly_compatible(param: &Ty, arg: &Ty) -> bool {
    fn leaf(ty: &Ty) -> String {
        match ty.kind() {
            TyKind::Scalar(s) => s.name().to_string(),
            TyKind::Adt(name, _) => name.clone(),
            TyKind::App { name, .. } => name.clone(),
            TyKind::Param(_) | TyKind::Infer(_) | TyKind::Error => "?".to_string(),
            _ => "?".to_string(),
        }
    }
    let p = leaf(param);
    let a = leaf(arg);
    let p = p.as_str();
    let a = a.as_str();
    if p == "?" || a == "?" {
        return true; // unknown → always plausible
    }
    if p == a {
        return true; // same name
    }
    // Numeric types are mutually compatible.
    let is_numeric = |s: &str| matches!(s, "int" | "nat" | "atom" | "range" | "implicit");
    if is_numeric(p) && is_numeric(a) {
        return true;
    }
    // bit ↔ bits
    if (p == "bit" && a == "bits") || (p == "bits" && a == "bit") {
        return true;
    }
    // atom_bool ↔ bool
    if (p == "atom_bool" && a == "bool") || (p == "bool" && a == "atom_bool") {
        return true;
    }
    // string ↔ string_literal
    if (p == "string" && a == "string_literal") || (p == "string_literal" && a == "string") {
        return true;
    }
    // Non-primitive types (user-defined) are always plausible
    // (could be type aliases for compatible types).
    let primitives = [
        "int",
        "nat",
        "bool",
        "string",
        "unit",
        "real",
        "bit",
        "bits",
        "atom",
        "range",
        "vector",
        "list",
        "option",
        "implicit",
        "atom_bool",
    ];
    if !primitives.contains(&p) || !primitives.contains(&a) {
        return true;
    }
    false
}

/// Whether two types differ only in numeric args (dependent-width mismatch).
fn is_dependent_width_mismatch(a: &Ty, b: &Ty) -> bool {
    match (a.kind(), b.kind()) {
        // Same App constructor with different args → dependent width
        (TyKind::App { name: na, .. }, TyKind::App { name: nb, .. }) => na == nb,
        // Tuples: recursively check each position. Every element must be
        // either unifiable (same type) or a dependent-width pair (same App name).
        (TyKind::Tuple(as_), TyKind::Tuple(bs)) if as_.len() == bs.len() => {
            as_.iter().zip(bs.iter()).all(|(ea, eb)| {
                // If elements are equal, OK
                if ea == eb {
                    return true;
                }
                // Recurse for nested dependent patterns
                is_dependent_width_mismatch(ea, eb)
            })
        }
        // Error/Infer/Param on either side → permissive
        (TyKind::Error, _) | (_, TyKind::Error) => true,
        (TyKind::Infer(_), _) | (_, TyKind::Infer(_)) => true,
        (TyKind::Param(_), _) | (_, TyKind::Param(_)) => true,
        _ => false,
    }
}

#[derive(Clone, Default)]
struct FreshenInfo {
    entries: Vec<FreshenEntry>,
}

/// Inference context for a single callable body.
pub(crate) struct InferenceContext<'db> {
    #[allow(dead_code)]
    pub(crate) db: Option<&'db dyn salsa::Database>,
    #[allow(dead_code)]
    pub(crate) owner: Option<hir_def::def_query::DefWithBodyId<'db>>,
    pub(crate) source: &'db str,
    pub(crate) body: std::sync::Arc<hir_def::Body>,
    source_map: Option<std::sync::Arc<hir_def::BodySourceMap>>,
    pub(crate) resolver: std::sync::Arc<hir_def::DefMap>,
    pub(super) env: TopLevelEnv,
    pub(super) pattern_constants: HashSet<String>,
    pub(super) table: InferenceTable,
    pub(crate) result: InferenceResult,
    pub(crate) return_ty: Ty,
    pub(crate) diverges: Diverges,
    pub(crate) is_pure_context: bool,
    pub(crate) observed_effects: std::collections::BTreeSet<hir_def::EffectTag>,
    pub(crate) workspace_effects: Option<std::sync::Arc<hir_def::effects::WorkspaceEffects>>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_byte_spans: Vec<(usize, usize)>,
    expr_types: Vec<(SpanKey, String)>,
    binding_types: Vec<(SpanKey, String)>,
    seen_errors: HashSet<(DiagnosticCode, usize, usize, TypeError)>,
    seen_error_log: Vec<(DiagnosticCode, usize, usize, TypeError)>,
    cancel: CancellationToken,
    pub(crate) has_incomplete_match: bool,
    pub(super) inference_fuel: usize,
    pub(super) fresh_var_counter: u32,
    pub(crate) used_bindings: HashSet<String>,
    expr_scopes: hir_def::expr_store::scope::ExprScopes,
}

struct ConcatOperandInfo {
    width: String,
    elem: Ty,
    is_vector: bool,
}

impl<'db> InferenceContext<'db> {
    /// Construct a fresh InferenceContext for a single callable body.
    pub fn new_for_body(
        source: &'db str,
        body: std::sync::Arc<hir_def::Body>,
        source_map: std::sync::Arc<hir_def::BodySourceMap>,
        env: TopLevelEnv,
        pattern_constants: HashSet<String>,
    ) -> Self {
        Self::new_for_body_with_cancel(
            source,
            body,
            source_map,
            env,
            pattern_constants,
            CancellationToken::never(),
        )
    }

    /// Construct with cancellation support.
    pub fn new_for_body_with_cancel(
        source: &'db str,
        body: std::sync::Arc<hir_def::Body>,
        source_map: std::sync::Arc<hir_def::BodySourceMap>,
        env: TopLevelEnv,
        pattern_constants: HashSet<String>,
        cancel: CancellationToken,
    ) -> Self {
        use std::sync::Arc;
        let mut table = InferenceTable::with_aliases(env.type_aliases.clone());
        table.set_alias_schemes(env.alias_schemes.clone());
        let resolver = Arc::new(hir_def::DefMap::default());
        // Build ExprScopes at construction for static binding lookup.
        let expr_scopes = hir_def::expr_store::scope::ExprScopes::new(&body);
        Self {
            db: None,
            owner: None,
            source,
            body,
            source_map: Some(source_map),
            resolver,
            env,
            pattern_constants,
            table,
            result: InferenceResult::default(),
            return_ty: Ty::error(),
            diverges: Diverges::Maybe,
            is_pure_context: false,
            observed_effects: std::collections::BTreeSet::new(),
            workspace_effects: None,
            diagnostics: Vec::new(),
            diagnostic_byte_spans: Vec::new(),
            expr_types: Vec::new(),
            binding_types: Vec::new(),
            seen_errors: HashSet::new(),
            seen_error_log: Vec::new(),
            cancel,
            has_incomplete_match: false,
            inference_fuel: 2_000,
            fresh_var_counter: 0,
            used_bindings: HashSet::new(),
            expr_scopes,
        }
    }

    /// Access the ExpressionStore of the current body.
    ///
    /// In our architecture, ExpressionStore is embedded inside Body.
    #[allow(dead_code)]
    pub(crate) fn store(&self) -> &hir_def::expr_store::ExpressionStore {
        &self.body.store
    }

    /// Derive a borrowing Resolver on demand from the owned DefMap.
    pub(crate) fn make_resolver(&self) -> hir_def::Resolver<'_> {
        hir_def::Resolver::for_file(&self.resolver)
    }

    #[allow(dead_code)] // will be wired into inference query
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Look up source span for an ExprId via the source_map.
    /// Returns None when no source_map is available.
    fn expr_span(&self, _body: &hir_def::Body, id: hir_def::ExprId) -> Option<Span> {
        self.source_map.as_ref()?.expr_syntax(id)
    }

    /// Look up source span for a PatId via the source_map.
    pub(super) fn pat_span(&self, _body: &hir_def::Body, id: hir_def::PatId) -> Option<Span> {
        self.source_map.as_ref()?.pat_syntax(id)
    }

    /// Finish and return result.
    pub fn finish_query(mut self) -> TypeCheckResult {
        // Double-pass obligation solving.
        self.solve_pending_obligations();
        self.solve_pending_obligations();
        // Resolve remaining inference variables to defaults.
        self.table.fallback_unsolved();
        // Unsolved constraints are often runtime assertions; callers
        // should treat as warning for pass-file validation.
        if !self.result.unsolved_constraints.is_empty() {
            let body_root = self.body.root();
            let diags: Vec<_> = self
                .result
                .unsolved_constraints
                .iter()
                .map(|ob| InferenceDiagnostic::UnsolvedConstraint {
                    expr: body_root,
                    constraint: format!("{:?}", ob),
                })
                .collect();
            for d in diags {
                self.push_inference_diagnostic(d);
            }
        }
        // Body validation using ExprScopes for binding enumeration.
        //
        // ExprScopes provides the authoritative list of all bindings in each
        // scope, replacing the ad-hoc `let_binding_pats` tracking.
        // Collect ALL identifier references from the Body arena,
        // not just those encountered during inference traversal.
        // This catches variables used in vector indexing, foreach bodies,
        // match arms, and other contexts where inference may short-circuit.
        let mut all_used = self.used_bindings.clone();
        for (_expr_id, expr) in self.body.iter_exprs() {
            match expr {
                hir_def::hir::Expr::Ident(name) => {
                    all_used.insert(name.clone());
                }
                hir_def::hir::Expr::TypeVar(name) => {
                    all_used.insert(name.clone());
                }
                _ => {}
            }
        }

        let expr_scopes = hir_def::expr_store::scope::ExprScopes::new(&self.body);
        let body_diags = crate::diagnostics::BodyValidationDiagnostic::collect(
            &self.body,
            &expr_scopes,
            &all_used,
            &self.pattern_constants,
        );
        for d in body_diags {
            match d {
                crate::diagnostics::BodyValidationDiagnostic::UnusedVariable { pat, name } => self
                    .push_inference_diagnostic(InferenceDiagnostic::UnusedVariable { pat, name }),
                crate::diagnostics::BodyValidationDiagnostic::RemoveTrailingReturn {
                    return_expr,
                } => self.push_inference_diagnostic(InferenceDiagnostic::RemoveTrailingReturn {
                    return_expr,
                }),
                crate::diagnostics::BodyValidationDiagnostic::RemoveUnnecessaryElse { if_expr } => {
                    self.push_inference_diagnostic(InferenceDiagnostic::RemoveUnnecessaryElse {
                        if_expr,
                    })
                }
            }
        }

        self.finish()
    }

    /// Attempt to resolve all pending obligations.
    fn solve_pending_obligations(&mut self) {
        let obligations = self.table.drain_obligations();
        for ob in obligations {
            match ob {
                Obligation::NumericConstraint(ref constraint) => {
                    let subst = Subst::default();
                    let result = constraint::try_normalize_constraint(constraint, &subst, &[]);
                    match result {
                        constraint::NormalizationResult::Decided(ConstraintStatus::Satisfied) => {}
                        constraint::NormalizationResult::Decided(ConstraintStatus::Failed) => {}
                        constraint::NormalizationResult::Decided(ConstraintStatus::Unknown)
                        | constraint::NormalizationResult::Deferred => {
                            self.result.unsolved_constraints.push(ob.clone());
                        }
                    }
                }
                Obligation::WidthEquality(ref lhs, ref rhs) => {
                    let subst = Subst::default();
                    let lhs_val = numeric::eval_numeric_expr(lhs, &subst, &[]);
                    let rhs_val = numeric::eval_numeric_expr(rhs, &subst, &[]);
                    match (lhs_val, rhs_val) {
                        (Some(l), Some(r)) if l == r => {}
                        (Some(_), Some(_)) => {}
                        _ => {
                            self.result.unsolved_constraints.push(ob.clone());
                        }
                    }
                }
            }
        }
    }

    fn finish(self) -> TypeCheckResult {
        let mut result = self.result;
        result.legacy_diagnostics = self.diagnostics;
        result.legacy_diagnostic_byte_spans = self.diagnostic_byte_spans;
        result.bodies = None;
        result.expr_types = HashMap::from_iter(self.expr_types);
        result.binding_types = HashMap::from_iter(self.binding_types);
        result
    }

    #[allow(dead_code)] // will be wired into inference query
    fn trace_typecheck(&self, kind: &str, name: &str, span: Option<Span>) {
        if std::env::var_os("SAIL_TYPECHECK_TRACE").is_none() {
            return;
        }
        if let Some(span) = span {
            let line = self.source[..span.start.min(self.source.len())]
                .chars()
                .filter(|&c| c == '\n')
                .count();
            tracing::debug!("[typecheck] {kind} {name} @ line {}", line + 1);
        } else {
            tracing::debug!("[typecheck] {kind} {name}");
        }
    }

    fn push_hint(&mut self, span: Span, message: String) {
        let code = DiagnosticCode::SailLint("unverified-constraint", Severity::Warning);
        let key = (code.clone(), span.start, span.end, TypeError::other(message.clone()));
        if !self.seen_errors.insert(key.clone()) {
            return;
        }
        self.seen_error_log.push(key);
        let range = base_db::text_range(span.start, span.end);
        self.diagnostics.push(Diagnostic::new(code, message, range, Severity::Hint));
        self.diagnostic_byte_spans.push((span.start, span.end));
    }

    /// Emit a typed `InferenceDiagnostic` to the inference result.
    ///
    /// This populates `InferenceResult.diagnostics` (the typed variant
    /// vector). Preferred over `push_error()` for diagnostics that have
    /// an `InferenceDiagnostic` variant — these go through the     /// cook → AnyDiagnostic → handler dispatch pipeline.
    ///
    /// directly during inference (`hir-ty/src/infer.rs`).
    pub(super) fn push_inference_diagnostic(&mut self, diag: InferenceDiagnostic) {
        self.result.diagnostics.push(diag);
    }

    fn record_expr_type(&mut self, span: Span, ty: &Ty) {
        if !ty.is_error() {
            self.expr_types.push(((span.start, span.end), ty.display_text()));
        }
    }

    pub(super) fn record_binding_type(&mut self, span: Span, ty: &Ty) {
        if !ty.is_error() {
            self.binding_types.push(((span.start, span.end), ty.display_text()));
        }
    }

    #[allow(dead_code)] // will be wired into inference query
    fn expect_type(&mut self, _span: Span, actual: &Ty, expected: &Ty) -> bool {
        // Skip checks involving Unknown — these come from cross-file references
        // (functions, constants, registers) that the per-file type checker
        // cannot resolve. Reporting subtype errors against Unknown produces
        // thousands of false positives.
        if actual.is_error() || expected.is_error() {
            return true;
        }
        // Type variables (e.g. polymorphic 'm, 'n in `forall 'n. bits('n)`)
        // should match any type for LSP purposes — we don't track kinds.
        if matches!(actual.kind(), TyKind::Param(_)) || matches!(expected.kind(), TyKind::Param(_))
        {
            return true;
        }
        // Resolve type aliases on both sides (e.g. xlenbits → bits(64)) before
        // unification so cross-file aliases compare correctly.
        // Alias resolution goes through InferenceTable, not env.
        let actual_resolved = self.table.normalize_alias_ty(actual);
        let expected_resolved = self.table.normalize_alias_ty(expected);
        if self.table.unify(&expected_resolved, &actual_resolved) {
            true
        } else {
            // This helper is dead code (only caller is
            // expect_int_type which is #[allow(dead_code)]).
            // Type mismatches should use record_type_mismatch(ExprId)
            // at call sites with ExprId context.
            false
        }
    }

    #[allow(dead_code)] // will be wired into inference query
    fn expect_int_type(&mut self, span: Span, actual: &Ty) -> bool {
        self.expect_type(span, actual, &Ty::named("int".to_string()))
    }

    /// Freshen a TypeScheme's quantifiers with fresh InferTys to avoid
    /// name collisions. Returns the freshened scheme plus a `FreshenInfo`
    /// for mapping freshened names back to originals.
    fn freshen_scheme(&mut self, scheme: &TypeScheme) -> (TypeScheme, FreshenInfo) {
        use super::constraint::{apply_subst, apply_subst_constraint_expr};
        if scheme.quantifiers.is_empty() {
            return (scheme.clone(), FreshenInfo::default());
        }
        let mut subst = Subst::default();
        let mut info = FreshenInfo::default();
        for q in &scheme.quantifiers {
            let fresh_var = self.table.new_type_var();
            subst.types.insert(q.clone(), fresh_var.clone());
            // For TyArg::Value expressions like "2 * 'n", InferTy can't
            // appear in value strings. Use a unique placeholder name that
            // won't collide with any real type variable.
            let placeholder = format!("'_fv{}", self.fresh_var_counter);
            self.fresh_var_counter += 1;
            subst.values.insert(q.clone(), placeholder.clone());
            // Register placeholder → InferTy mapping for deep_resolve.
            if let TyKind::Infer(crate::ty::InferTy(id)) = fresh_var.kind() {
                self.table.register_placeholder(&placeholder, *id);
            }
            info.entries.push(FreshenEntry {
                fresh_name: placeholder,
                orig_name: q.clone(),
                infer_ty: fresh_var,
            });
        }
        let freshened_quantifiers: Vec<String> =
            info.entries.iter().map(|e| e.fresh_name.clone()).collect();
        (
            TypeScheme {
                quantifiers: freshened_quantifiers,
                kind_bounds: scheme.kind_bounds.clone(),
                constraints: scheme
                    .constraints
                    .iter()
                    .map(|c| {
                        let expr = apply_subst_constraint_expr(&c.expr, &subst);
                        // Update mentions to use freshened variable names so
                        // evaluate_constraint can find them in the subst.
                        let mentions = c
                            .mentions
                            .iter()
                            .map(|m| subst.values.get(m).cloned().unwrap_or_else(|| m.clone()))
                            .collect();
                        // Keep original text for user-facing diagnostics (display
                        // uses original variable names like 'n, not freshened '_fv0).
                        QuantConstraint { text: c.text.clone(), mentions, expr }
                    })
                    .collect(),
                params: scheme.params.iter().map(|p| apply_subst(p, &subst)).collect(),
                implicit_params: scheme.implicit_params.clone(),
                ret: apply_subst(&scheme.ret, &subst),
                declared_effects: scheme.declared_effects.clone(),
                is_declared_pure: scheme.is_declared_pure,
            },
            info,
        )
    }

    /// Extract value-level bindings from matching App type args.
    /// When expected has `TyArg::Nexp(Var("'_fv0"))` and actual has
    /// `TyArg::Nexp(Var("'sew"))`, inserts `"'_fv0" → "'sew"` into subst.values.
    /// This replaces the supplementary constraint::unify call.
    /// Extract value-level bindings by walking type structure and
    /// calling `constraint::unify_value` for numeric arg pairs.
    /// This replaces the supplementary `constraint::unify` calls
    /// while preserving `unify_value`'s linear arithmetic solving.
    pub(super) fn extract_value_bindings(&self, expected: &Ty, actual: &Ty, subst: &mut Subst) {
        match (expected.kind(), actual.kind()) {
            (TyKind::App { args: exp_args, .. }, TyKind::App { args: act_args, .. }) => {
                for (ea, aa) in exp_args.iter().zip(act_args.iter()) {
                    match (ea, aa) {
                        (TyArg::Type(et), TyArg::Type(at)) => {
                            // Param('a) vs concrete type → bind in subst.types.
                            if let TyKind::Param(name) = et.kind() {
                                if !subst.types.contains_key(name) {
                                    subst.types.insert(name.clone(), at.clone());
                                    subst.values.insert(name.clone(), at.display_text());
                                }
                            } else {
                                self.extract_value_bindings(et, at, subst);
                            }
                        }
                        _ => {
                            if let (Some(e), Some(a)) = (ea.as_value_str(), aa.as_value_str()) {
                                let _ = constraint::unify_value(&e, &a, subst);
                            }
                        }
                    }
                }
            }
            // Param at top level.
            (TyKind::Param(name), _) if !subst.types.contains_key(name) => {
                subst.types.insert(name.clone(), actual.clone());
                subst.values.insert(name.clone(), actual.display_text());
            }
            (TyKind::App { args: exp_args, name, .. }, TyKind::Scalar(s))
                if matches!(name.as_str(), "int" | "atom" | "nat" | "range") =>
            {
                for ea in exp_args {
                    if let Some(e) = ea.as_value_str() {
                        if !subst.values.contains_key(&e) {
                            subst.values.insert(e, s.name().to_string());
                        }
                    }
                }
            }
            (TyKind::Tuple(et), TyKind::Tuple(at)) if et.len() == at.len() => {
                for (e, a) in et.iter().zip(at.iter()) {
                    self.extract_value_bindings(e, a, subst);
                }
            }
            _ => {}
        }
    }

    fn quantifier_is_bound(&self, name: &str, subst: &Subst) -> bool {
        subst.values.contains_key(name) || subst.types.contains_key(name)
    }

    fn fill_assumptions(&self, locals: &LocalEnv, out: &mut Vec<ConstraintExpr>) {
        out.clear();
        out.extend(self.env.global_constraints.iter().cloned());
        out.extend(locals.constraints.iter().cloned());
    }

    fn evaluate_constraint(
        &self,
        expr: &ConstraintExpr,
        subst: &Subst,
        assumptions: &[ConstraintExpr],
    ) -> ConstraintStatus {
        let status = self.evaluate_constraint_inner(expr, subst, assumptions);
        #[cfg(feature = "z3-solver")]
        let status = if matches!(status, ConstraintStatus::Unknown) {
            z3_solver::try_solve(expr, subst, assumptions)
        } else {
            status
        };
        status
    }

    fn evaluate_constraint_inner(
        &self,
        expr: &ConstraintExpr,
        subst: &Subst,
        assumptions: &[ConstraintExpr],
    ) -> ConstraintStatus {
        if constraint_implied_by_assumptions(assumptions, expr, subst) {
            return ConstraintStatus::Satisfied;
        }

        match expr {
            ConstraintExpr::Bool(true) => ConstraintStatus::Satisfied,
            ConstraintExpr::Bool(false) => ConstraintStatus::Failed,
            ConstraintExpr::Compare { lhs, op, rhs } => {
                let (Some(lhs), Some(rhs)) = (
                    eval_numeric_expr(lhs, subst, assumptions),
                    eval_numeric_expr(rhs, subst, assumptions),
                ) else {
                    return ConstraintStatus::Unknown;
                };
                let holds = match op {
                    CompareOp::Eq => lhs == rhs,
                    CompareOp::Neq => lhs != rhs,
                    CompareOp::Lt => lhs < rhs,
                    CompareOp::Lte => lhs <= rhs,
                    CompareOp::Gt => lhs > rhs,
                    CompareOp::Gte => lhs >= rhs,
                };
                if holds {
                    ConstraintStatus::Satisfied
                } else {
                    ConstraintStatus::Failed
                }
            }
            ConstraintExpr::InSet { value, items } => {
                let Some(value) = eval_numeric_expr(value, subst, assumptions) else {
                    return ConstraintStatus::Unknown;
                };
                let mut all_known = true;
                for item in items {
                    match eval_numeric_expr(item, subst, assumptions) {
                        Some(item) if item == value => return ConstraintStatus::Satisfied,
                        Some(_) => {}
                        None => all_known = false,
                    }
                }
                if all_known {
                    ConstraintStatus::Failed
                } else {
                    ConstraintStatus::Unknown
                }
            }
            ConstraintExpr::And(items) => {
                let mut saw_unknown = false;
                for item in items {
                    match self.evaluate_constraint_inner(item, subst, assumptions) {
                        ConstraintStatus::Satisfied => {}
                        ConstraintStatus::Failed => return ConstraintStatus::Failed,
                        ConstraintStatus::Unknown => saw_unknown = true,
                    }
                }
                if saw_unknown {
                    ConstraintStatus::Unknown
                } else {
                    ConstraintStatus::Satisfied
                }
            }
            ConstraintExpr::Or(items) => {
                let mut saw_unknown = false;
                for item in items {
                    match self.evaluate_constraint_inner(item, subst, assumptions) {
                        ConstraintStatus::Satisfied => return ConstraintStatus::Satisfied,
                        ConstraintStatus::Failed => {}
                        ConstraintStatus::Unknown => saw_unknown = true,
                    }
                }
                if saw_unknown {
                    ConstraintStatus::Unknown
                } else {
                    ConstraintStatus::Failed
                }
            }
            ConstraintExpr::Not(inner) => {
                match self.evaluate_constraint_inner(inner, subst, assumptions) {
                    ConstraintStatus::Satisfied => ConstraintStatus::Failed,
                    ConstraintStatus::Failed => ConstraintStatus::Satisfied,
                    ConstraintStatus::Unknown => ConstraintStatus::Unknown,
                }
            }
            ConstraintExpr::Unsupported => ConstraintStatus::Unknown,
            ConstraintExpr::App { .. } | ConstraintExpr::BoolVar(_) => ConstraintStatus::Unknown,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn instantiation_error_with_sig(
        &mut self,
        id: &str,
        quantifiers: &[String],
        constraints: &[QuantConstraint],
        subst: &Subst,
        assumptions: &[ConstraintExpr],
        signature: Option<String>,
        derived_from: &[Span],
        freshen_info: &FreshenInfo,
    ) -> Option<TypeError> {
        // Pre-compute bound status for each quantifier. A quantifier is
        // considered bound if either the Subst has a value for it OR its
        // corresponding InferTy was resolved by the table.
        let bound_set: std::collections::HashSet<&str> = quantifiers
            .iter()
            .filter(|name| {
                if self.quantifier_is_bound(name, subst) {
                    return true;
                }
                if let Some(entry) =
                    freshen_info.entries.iter().find(|e| e.fresh_name == name.as_str())
                {
                    let resolved = self.table.resolve(&entry.infer_ty);
                    if !matches!(resolved.kind(), TyKind::Infer(_)) {
                        return true;
                    }
                }
                false
            })
            .map(|s| s.as_str())
            .collect();

        let is_bound = |name: &str| -> bool { bound_set.contains(name) };

        // Map freshened names back to originals for display.
        let unfreshen = |name: &str| -> String {
            freshen_info
                .entries
                .iter()
                .find(|e| e.fresh_name == name)
                .map(|e| e.orig_name.clone())
                .unwrap_or_else(|| name.to_string())
        };

        let mut unresolved = quantifiers
            .iter()
            .filter(|quantifier| !is_bound(quantifier))
            .map(|q| unfreshen(q))
            .collect::<Vec<_>>();

        for constraint in constraints {
            if constraint.mentions.iter().any(|name| quantifiers.contains(name) && !is_bound(name))
            {
                unresolved.push(constraint.text.clone());
                continue;
            }

            let status = self.evaluate_constraint(&constraint.expr, subst, assumptions);
            match status {
                ConstraintStatus::Satisfied => {}
                ConstraintStatus::Failed => {
                    return Some(TypeError::FailedConstraint {
                        constraint: constraint.text.clone(),
                        derived_from: derived_from.to_vec(),
                    });
                }
                ConstraintStatus::Unknown => unresolved.push(constraint.text.clone()),
            }
        }

        unresolved.sort();
        unresolved.dedup();
        (!unresolved.is_empty()).then_some(TypeError::UnresolvedQuants {
            id: id.to_string(),
            quants: unresolved,
            signature,
        })
    }

    // add_expr_constraint + propagate_post_expr_constraints removed — used core_ast types.

    fn record_info_for_type(&self, ty: &Ty) -> Option<(String, RecordInfo, Subst)> {
        match ty.kind() {
            TyKind::Adt(name, _) => self
                .env
                .records
                .get(name)
                .cloned()
                .map(|info| (name.clone(), info, Subst::default())),
            TyKind::App { name, args, .. } => {
                let info = self.env.records.get(name)?.clone();
                // Use GenericArgs for positional → named bridge
                let generic_args = GenericArgs::from_ty_args(args);
                let subst = generic_args.to_subst(&info.params);
                Some((name.clone(), info, subst))
            }
            _ => None,
        }
    }

    /// Look up a cross-file value's type via per-query salsa lookup.
    ///
    /// Uses `symbol_index` to find the defining file, then queries
    /// `top_level_env(db, file)` to get the value's type.
    fn cross_file_value_type(&mut self, name: &str) -> Option<Ty> {
        let db = self.db?;
        let file = self.env.symbol_index.get_file(name)?;
        let env_data = crate::query::top_level_env(db, file);
        let file_env = &env_data.0.env;
        // Check values (let/var bindings)
        if let Some(ty) = file_env.values.get(name) {
            let expanded = self.table.normalize_alias_ty(ty);
            return Some(self.table.insert_type_vars(&expanded));
        }
        // Check registers
        if let Some(ty) = file_env.registers.get(name) {
            let expanded = self.table.normalize_alias_ty(ty);
            return Some(self.table.insert_type_vars(&expanded));
        }
        None
    }

    fn record_field_type(&self, ty: &Ty, field: &str) -> Option<Ty> {
        let (_, info, subst) = self.record_info_for_type(ty)?;
        info.fields.get(field).map(|field_ty| apply_subst(field_ty, &subst))
    }

    #[allow(dead_code)] // will be wired into inference query
    fn register_type_for_name(&self, name: &str) -> Option<Ty> {
        self.env.registers.get(name).cloned()
    }

    #[allow(dead_code)] // will be wired into inference query
    fn bitfield_info_for_type(&self, ty: &Ty) -> Option<(String, BitfieldInfo)> {
        match ty.kind() {
            TyKind::Adt(name, _) => {
                self.env.bitfields.get(name).cloned().map(|info| (name.clone(), info))
            }
            _ => None,
        }
    }

    #[allow(dead_code)] // will be wired into inference query
    fn bitfield_field_type(&self, ty: &Ty, field: &str) -> Option<Ty> {
        let (_, info) = self.bitfield_info_for_type(ty)?;
        if field == "bits" {
            Some(info.underlying)
        } else {
            info.fields.get(field).cloned()
        }
    }

    fn collection_element_type(&self, ty: &Ty) -> Option<Ty> {
        match ty.kind() {
            TyKind::App { name, args, .. } if name == "list" => {
                args.first().and_then(|arg| match arg {
                    TyArg::Type(ty) => Some(ty.clone()),
                    TyArg::Nexp(_) | TyArg::Value(_) => None,
                })
            }
            TyKind::App { name, args, .. } if name == "vector" => {
                args.last().and_then(|arg| match arg {
                    TyArg::Type(ty) => Some(ty.clone()),
                    TyArg::Nexp(_) | TyArg::Value(_) => None,
                })
            }
            TyKind::App { name, .. } if name == "bits" => Some(Ty::named("bit".to_string())),
            _ => None,
        }
    }

    fn collection_length_text(&self, ty: &Ty) -> Option<String> {
        match ty.kind() {
            TyKind::App { name, args, .. } if name == "vector" || name == "bits" => {
                args.first().map(|arg| match arg {
                    TyArg::Nexp(n) => n.to_string_repr(),
                    TyArg::Value(value) => value.clone(),
                    TyArg::Type(ty) => ty.display_text(),
                })
            }
            _ => None,
        }
    }

    #[allow(dead_code)] // will be wired into inference query
    fn collection_length_expr(&self, ty: &Ty) -> Option<NumericExpr> {
        self.collection_length_text(ty).and_then(|text| parse_numeric_expr_text(&text))
    }

    fn concat_operand_info(&self, ty: &Ty) -> Option<ConcatOperandInfo> {
        match ty.kind() {
            TyKind::Scalar(crate::ty::Scalar::Bit) => Some(ConcatOperandInfo {
                width: "1".to_string(),
                elem: Ty::named("bit".to_string()),
                is_vector: false,
            }),
            TyKind::App { name, .. } if name == "bits" => Some(ConcatOperandInfo {
                width: self.collection_length_text(ty)?,
                elem: Ty::named("bit".to_string()),
                is_vector: false,
            }),
            TyKind::App { name, .. } if name == "vector" => Some(ConcatOperandInfo {
                width: self.collection_length_text(ty)?,
                elem: self.collection_element_type(ty)?,
                is_vector: true,
            }),
            _ => None,
        }
    }

    fn concat_width_text(&self, lhs: &str, rhs: &str) -> String {
        let lhs_value = parse_numeric_expr_text(lhs)
            .and_then(|expr| eval_numeric_expr(&expr, &Subst::default(), &[]));
        let rhs_value = parse_numeric_expr_text(rhs)
            .and_then(|expr| eval_numeric_expr(&expr, &Subst::default(), &[]));
        match (lhs_value, rhs_value) {
            (Some(lhs), Some(rhs)) => (lhs + rhs).to_string(),
            _ => format!("({lhs}) + ({rhs})"),
        }
    }

    /// Resolve an overloaded binary operator to the return type of the
    /// first matching candidate (plausibility filter, then full unification).
    fn resolve_overloaded_binop(&mut self, op_str: &str, lhs_ty: &Ty, rhs_ty: &Ty) -> Option<Ty> {
        let members = self.env.overloads.get(op_str)?.clone();

        // Collect candidate schemes from local env.
        let mut all_schemes: SmallVec<[std::sync::Arc<TypeScheme>; 8]> = SmallVec::new();
        for member in &members {
            all_schemes.extend(self.env.lookup_functions(member));
        }

        // Plausibility filter: arity must be 2 + leaf-name match on both args.
        let plausible: Vec<_> = all_schemes
            .iter()
            .filter(|s| {
                s.params.len() == 2
                    && types_plausibly_compatible(&s.params[0], lhs_ty)
                    && types_plausibly_compatible(&s.params[1], rhs_ty)
            })
            .collect();

        // Full unification on plausible candidates with snapshot/rollback.
        let args = [lhs_ty.clone(), rhs_ty.clone()];
        for scheme in plausible {
            let (freshened, _) = self.freshen_scheme(scheme);
            let mut mrc =
                crate::method_resolution::MethodResolutionContext { table: &mut self.table };
            if mrc.try_candidate(&freshened.params, &args) {
                return Some(self.table.resolve(&freshened.ret));
            }
        }
        None
    }

    fn infer_concat_result_type(
        &mut self,
        _span: Span,
        expr_id: hir_def::ExprId,
        lhs_ty: &Ty,
        rhs_ty: &Ty,
    ) -> Ty {
        if lhs_ty.is_error() || rhs_ty.is_error() {
            return Ty::error();
        }
        // Cross-file type aliases (e.g. xlenbits, regidx) cannot be resolved
        // by the local checker. Skip the concat check for any type that isn't
        // explicitly bits(...) or vector(...).
        let is_definitely_concatenable = |t: &Ty| {
            let text = t.display_text();
            text.starts_with("bits(") || text.starts_with("vector(")
        };
        let is_definitely_not_concatenable = |t: &Ty| {
            let text = t.display_text();
            matches!(text.as_str(), "int" | "nat" | "bool" | "string" | "unit" | "real")
        };
        if !is_definitely_concatenable(lhs_ty) && !is_definitely_not_concatenable(lhs_ty) {
            return Ty::error();
        }
        if !is_definitely_concatenable(rhs_ty) && !is_definitely_not_concatenable(rhs_ty) {
            return Ty::error();
        }

        let Some(lhs) = self.concat_operand_info(lhs_ty) else {
            self.push_inference_diagnostic(InferenceDiagnostic::ConcatTypeMismatch {
                expr: expr_id,
                message: format!("Cannot concatenate non-vector type {}", lhs_ty.display_text()),
            });
            return Ty::error();
        };
        let Some(rhs) = self.concat_operand_info(rhs_ty) else {
            self.push_inference_diagnostic(InferenceDiagnostic::ConcatTypeMismatch {
                expr: expr_id,
                message: format!("Cannot concatenate non-vector type {}", rhs_ty.display_text()),
            });
            return Ty::error();
        };

        if !self.table.unify(&lhs.elem, &rhs.elem) {
            self.push_inference_diagnostic(InferenceDiagnostic::ConcatTypeMismatch {
                expr: expr_id,
                message: format!(
                    "Cannot concatenate {} with {}",
                    lhs_ty.display_text(),
                    rhs_ty.display_text()
                ),
            });
            return Ty::error();
        }

        let elem_ty = self.table.resolve(&lhs.elem);
        let width = self.concat_width_text(&lhs.width, &rhs.width);

        if lhs.is_vector && rhs.is_vector {
            vector_ty(width, elem_ty)
        } else {
            bits_ty(width)
        }
    }

    // Run the Maranget-style pattern usefulness check on a `match` and
    // emit `IncompleteMatch` / `RedundantMatchArm` diagnostics. Pattern
    // binding has already happened in `check_match_cases`.

    /// Run exhaustiveness checking on match arms from a Body arena.
    /// Parallel to `check_match_exhaustiveness` but uses `lower_arms_hir`
    /// (Pat from Body) instead of `lower_arms` (core_ast::Pattern).
    #[allow(dead_code)]
    fn check_match_exhaustiveness_hir(
        &mut self,
        scrutinee_ty: &Ty,
        body: &hir_def::Body,
        arms: &[hir_def::MatchArm],
    ) {
        if arms.is_empty() {
            return;
        }

        let resolved = self.table.normalize_alias_ty(scrutinee_ty);

        // Skip exhaustiveness check when scrutinee type has
        // unknown constructors. Without complete constructor
        // information the check produces false-positive
        // "non-exhaustive match" diagnostics.
        //
        // RA doesn't need this guard because CrateDefMap always
        // provides complete constructor information.
        let type_name = match resolved.kind() {
            TyKind::Adt(n, _) => Some(n.as_str()),
            TyKind::App { name, .. } => Some(name.as_str()),
            TyKind::Scalar(Scalar::Bool) => Some("bool"),
            _ => None,
        };
        if let Some(name) = type_name {
            // bool is always fully known — skip guard.
            if name != "bool" {
                let has_constructors =
                    self.env.enums.contains_key(name) || self.env.unions.contains_key(name);
                if !has_constructors {
                    return;
                }
                // Scattered open types: skip exhaustiveness.
                // Upstream: is_scattered_open checks if the enum/union
                // has no `end` marker yet. Open types may gain more
                // constructors, so match cannot be exhaustive.
                if self.env.scattered_open_types.contains(name) {
                    return;
                }
            }
        }

        let scrutinee_match_ty = promote_record_ty(&ty_to_match_ty(&resolved), &self.env.records);

        let pattern_constants = &self.pattern_constants;
        let has_workspace = self.env.has_workspace_context;
        let is_constant =
            |name: &str| -> bool { !is_pattern_binding(name, pattern_constants, has_workspace) };
        let lowered_arms = match_check::lower_arms_hir(body, arms, &is_constant);

        // Reuse the same EnvCx construction as check_match_exhaustiveness
        let mut arity_map: HashMap<String, usize> = HashMap::new();
        for (name, schemes) in &self.env.constructors {
            if let Some(scheme) = schemes.first() {
                let arity = match scheme.params.as_slice() {
                    [] => 0,
                    [single] => match single.kind() {
                        TyKind::Tuple(items) => items.len(),
                        // Unit-payload constructors: `None : unit` is called
                        // as `None()` with 0 args (Sail convention). Treat
                        // arity as 0 so the match checker aligns with the
                        // lowered pattern.
                        TyKind::Scalar(Scalar::Unit) => 0,
                        TyKind::Adt(name, _) if name == "unit" => 0,
                        _ => 1,
                    },
                    many => many.len(),
                };
                arity_map.insert(name.clone(), arity);
            }
        }
        for (name, arities) in &self.env.cross_file_function_arity {
            if arity_map.contains_key(name) {
                continue;
            }
            if let Some(&(_, max)) = arities.iter().max_by_key(|(_, max)| *max) {
                arity_map.insert(name.clone(), max);
            }
        }
        let mut records_for_cx: HashMap<String, RecordFields> = HashMap::new();
        for (record_name, info) in &self.env.records {
            let mut entries: Vec<(String, MatchTy)> = info
                .fields
                .iter()
                .map(|(f, t)| (f.clone(), promote_record_ty(&ty_to_match_ty(t), &self.env.records)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            records_for_cx.insert(record_name.clone(), RecordFields { fields: entries });
        }
        let env_ref = &self.env;
        let resolve_variant = move |name: &str, scrutinee: &MatchTy| -> Option<Vec<MatchTy>> {
            let scheme = env_ref.constructors.get(name)?.first()?;
            let scrutinee_args: &[MatchTy] = match scrutinee {
                MatchTy::Named(_, args) => args.as_slice(),
                _ => &[],
            };
            let mut subst: HashMap<String, MatchTy> = HashMap::new();
            if !scheme.quantifiers.is_empty() && scrutinee_args.len() == scheme.quantifiers.len() {
                for (q, a) in scheme.quantifiers.iter().zip(scrutinee_args.iter()) {
                    subst.insert(q.clone(), a.clone());
                }
            }
            // Filter out unit-payload constructors (arity 0 — see arity_map above).
            let payload: Vec<MatchTy> = scheme
                .params
                .iter()
                .map(|p| ty_to_match_ty_with_subst(p, &subst, &env_ref.records))
                .collect();
            if payload.len() == 1 {
                match &payload[0] {
                    MatchTy::Named(name, _) if name == "unit" => Some(Vec::new()),
                    _ => Some(payload),
                }
            } else {
                Some(payload)
            }
        };
        let cx = EnvCx {
            enums: &self.env.enums,
            unions: &self.env.unions,
            constructor_arity: &arity_map,
            records: &records_for_cx,
            resolve_variant: &resolve_variant,
        };
        let report = match_check::compute_match_usefulness(&lowered_arms, &scrutinee_match_ty, &cx);

        let has_concrete_witness =
            report.missing_witnesses.iter().any(|w| !matches!(w, match_check::MatchPat::Wild));
        if has_concrete_witness {
            self.has_incomplete_match = true;
            // Emit legacy diagnostic with concrete witness detail.
            // Also sets has_incomplete_match for the InferenceDiagnostic
            // emission at the call site.
            let anchor_span = self.pat_span(body, arms[0].pat).unwrap_or(Span::new(0, 0));
            let range = base_db::text_range(
                anchor_span.start,
                (anchor_span.start + 1).min(anchor_span.end),
            );
            let missing = report
                .missing_witnesses
                .iter()
                .filter(|w| !matches!(w, match_check::MatchPat::Wild))
                .map(|w| w.display_text())
                .collect::<Vec<_>>()
                .join(", ");
            self.diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailLint("incomplete-match", Severity::Warning),
                format!("Non-exhaustive match: missing arm(s) for {missing}"),
                range,
                Severity::Warning,
            ));
        }
        for redundant_span in &report.redundant {
            let range = base_db::text_range(redundant_span.start, redundant_span.end);
            self.diagnostics.push(Diagnostic::new(
                DiagnosticCode::SailLint("redundant-match-arm", Severity::Warning),
                "Unreachable match arm — pattern is subsumed by an earlier arm".to_string(),
                range,
                Severity::Warning,
            ));
            self.diagnostic_byte_spans.push((redundant_span.start, redundant_span.end));
        }
    }

    //
    // takes ExprId, indexes into Body arena, matches on Expr.
    // Falls back to Ty::error() for unimplemented arms.

    // `bind_pattern_hir` moved to `infer/pat.rs` ().
    // Methods are added via a separate `impl InferenceContext` block in pat.rs.

    /// Call inference on ExprId/Expr arena. Implements:
    /// 1. Callee name extraction from Expr
    /// 2. Argument type inference
    /// 3. Multi-candidate overload resolution with unification
    /// 4. Constraint instantiation checking
    /// 5. Cross-file arity checking
    fn infer_call_hir(
        &mut self,
        body: &hir_def::Body,
        callee_id: hir_def::ExprId,
        args: &[hir_def::ExprId],
        locals: &mut LocalEnv,
    ) -> Ty {
        let expected_ret = locals.expected_return.clone();
        use hir_def::Expr;

        // 1. Extract callee name
        let callee_name = match body.expr(callee_id) {
            Some(Expr::Ident(name)) => name.clone(),
            Some(Expr::Field { expr: inner, field }) => {
                // Method-like call: obj.method(args) → _mod_method(obj, args)
                let _receiver_ty = self.infer_expr_hir(body, *inner, locals);
                format!("_mod_{field}")
            }
            _ => {
                // Non-name callee — infer its type but can't resolve signature.
                let callee_ty = self.infer_expr_hir(body, callee_id, locals);
                for &a in args {
                    self.infer_expr_hir(body, a, locals);
                }
                // D6: Emit typed InferenceDiagnostic::ExpectedFunction.
                if !callee_ty.is_error() {
                    self.push_inference_diagnostic(InferenceDiagnostic::ExpectedFunction {
                        call_expr: callee_id,
                        found: callee_ty,
                    });
                }
                return Ty::error();
            }
        };

        // Transitive purity enforcement.
        // If we're in a pure context and workspace effects are available,
        // check if the callee has impure transitive effects.
        if self.is_pure_context {
            if let Some(ref we) = self.workspace_effects {
                if !we.is_pure(&callee_name) {
                    let effects = we.combined_effects(&callee_name);
                    if let Some(&first_effect) = effects.iter().next() {
                        // Skip non-actionable effects
                        if !matches!(
                            first_effect,
                            hir_def::EffectTag::IncompleteMatch | hir_def::EffectTag::Scattered
                        ) {
                            let _call_span =
                                self.expr_span(body, callee_id).unwrap_or(Span::new(0, 0));
                            self.push_inference_diagnostic(InferenceDiagnostic::EffectViolation {
                                expr: callee_id,
                                effect: first_effect,
                                context: "pure function",
                            });
                        }
                    }
                }
            }
        }

        // 2. Infer argument types
        let arg_types: Vec<Ty> =
            args.iter().map(|&a| self.infer_expr_hir(body, a, locals)).collect();

        let callee_span = self.expr_span(body, callee_id).unwrap_or(Span::new(0, 0));
        let arg_spans: Vec<Span> =
            args.iter().map(|&a| self.expr_span(body, a).unwrap_or(Span::new(0, 0))).collect();

        // 3. Built-in call dispatch — inline checks for vector ops
        if callee_name == "vector_access#" && arg_types.len() >= 2 {
            let base_ty = &arg_types[0];
            let index_ty = &arg_types[1];
            // Check index is int
            if !index_ty.is_error() {
                let int_ty = Ty::named("int".to_string());
                if !self.table.unify(&int_ty, index_ty) {
                    self.result.record_type_mismatch(args[1], &int_ty, index_ty);
                }
            }
            // Bounds check for known-width vectors (call-site path).
            // Provides LSP diagnostic hint; filtered as soft in corpus_check.
            if let Some(info) = self.concat_operand_info(base_ty) {
                let idx_span = arg_spans.get(1).copied().unwrap_or(Span::new(0, 0));
                let idx_text = self.source.get(idx_span.start..idx_span.end).unwrap_or("");
                if let Ok(idx_val) = idx_text.trim().parse::<usize>() {
                    if let Ok(width) = info.width.parse::<usize>() {
                        if idx_val >= width {
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::ConstraintViolation {
                                    expr: callee_id,
                                    constraint: format!("0 <= {} < {}", idx_val, width),
                                    derived_from: vec![idx_span],
                                },
                            );
                        }
                    }
                }
            }
            return self.concat_operand_info(base_ty).map(|info| info.elem).unwrap_or(Ty::error());
        }
        if callee_name == "vector_update_subrange#" && arg_types.len() >= 4 {
            // vector_update_subrange#(base, hi, lo, value)
            let base_ty = &arg_types[0];
            let value_ty = &arg_types[3];
            // Get hi/lo from source text
            let hi_span = arg_spans.get(1).copied().unwrap_or(Span::new(0, 0));
            let lo_span = arg_spans.get(2).copied().unwrap_or(Span::new(0, 0));
            let hi_text = self.source.get(hi_span.start..hi_span.end).unwrap_or("");
            let lo_text = self.source.get(lo_span.start..lo_span.end).unwrap_or("");
            // Check value width matches range width
            if let (Ok(hi), Ok(lo)) =
                (hi_text.trim().parse::<usize>(), lo_text.trim().parse::<usize>())
            {
                let range_width = if hi >= lo { hi - lo + 1 } else { lo - hi + 1 };
                if let TyKind::App { name: ref vname, args: ref vargs, .. } = value_ty.kind() {
                    if vname == "bits" {
                        if let Some(w_str) = vargs.first().and_then(|a| a.as_value_str()) {
                            if let Ok(val_width) = w_str.parse::<usize>() {
                                if val_width != range_width {
                                    let expected = Ty::app(
                                        "bits",
                                        vec![crate::ty::TyArg::numeric(range_width.to_string())],
                                        format!("bits({range_width})"),
                                    );
                                    if args.len() > 3 {
                                        self.result
                                            .record_type_mismatch(args[3], &expected, value_ty);
                                    }
                                }
                            }
                        }
                    }
                }
            }
            return base_ty.clone();
        }
        if callee_name == "vector_subrange#"
            || callee_name == "vector_update#"
            || callee_name == "slice"
        {
            return Ty::error();
        }

        // 4. Cross-file: look up the function scheme from the defining file
        //    so we return a real type instead of Ty::error().
        let cross_file_known = self.env.cross_file_function_names.contains(&callee_name)
            || self.env.cross_file_constructor_names.contains(&callee_name);
        if cross_file_known && !self.env.functions.contains_key(&callee_name) {
            // Cross-file arity check
            if let Some(arities) = self.env.cross_file_function_arity.get(&callee_name) {
                let arg_count = arg_types.len();
                let any_match =
                    arities.iter().any(|(req, total)| arg_count >= *req && arg_count <= *total);
                if !arities.is_empty() && !any_match {
                    let (_min_req, max_total) =
                        arities.iter().fold((usize::MAX, 0usize), |(mr, mt), (req, total)| {
                            (mr.min(*req), mt.max(*total))
                        });
                    self.push_inference_diagnostic(InferenceDiagnostic::MismatchedArgCount {
                        call_expr: callee_id,
                        expected: max_total,
                        found: arg_count,
                    });
                }
            }
            // Look up defining file, get scheme's return type, expand aliases,
            // then replace Params with fresh inference vars.
            if let Some(db) = self.db {
                // Symbol index lookup.
                use crate::infer::env::SymbolKind;
                if let Some((file, kind)) = self.env.symbol_index.get(&callee_name) {
                    let env_data = crate::query::top_level_env(db, file);
                    let schemes = match kind {
                        SymbolKind::Constructor => env_data.0.env.constructors.get(&callee_name),
                        _ => env_data.0.env.functions.get(&callee_name),
                    };
                    if let Some(scheme) = schemes.and_then(|s| s.first()) {
                        let expanded = self.table.normalize_alias_ty(&scheme.ret);
                        let instantiated = self.table.insert_type_vars(&expanded);
                        // Resolve Bidir return types (mapping val specs).
                        if let TyKind::Bidir { lhs, rhs } = instantiated.kind() {
                            if !arg_types.is_empty() {
                                if self.table.unify(&arg_types[0], lhs) {
                                    return rhs.clone();
                                } else if self.table.unify(&arg_types[0], rhs) {
                                    return lhs.clone();
                                }
                            }
                        }
                        return instantiated;
                    }
                    // Signature text parsing fallback removed — rsplit("->")
                    // is unreliable for complex type signatures with nested
                    // arrows.
                }
                // Cross-file overload member resolution.
                //
                // When `callee_name` is an overload (e.g., `X → [rX_bits, rX, ...]`)
                // and `symbol_index` doesn't have it (overload names aren't indexed),
                // resolve members individually via per-query lookup.
                //
                // Filter overload candidates by arity + plausibility,
                // then attempt full unification on the narrow set.
                if let Some(members) = self.env.overloads.get(&callee_name) {
                    if !members.is_empty() {
                        let members = members.clone();
                        let mut cross_candidates: SmallVec<[std::sync::Arc<TypeScheme>; 4]> =
                            SmallVec::new();
                        for member in &members {
                            if let Some((file, _)) = self.env.symbol_index.get(member) {
                                let env_data = crate::query::top_level_env(db, file);
                                if let Some(schemes) = env_data.0.env.functions.get(member.as_str())
                                {
                                    cross_candidates.extend(schemes.iter().cloned());
                                }
                            }
                        }
                        // filter_overload_tree: arity + plausibility
                        let plausible: Vec<_> = cross_candidates
                            .iter()
                            .filter(|c| {
                                let required = c.implicit_params.iter().filter(|im| !**im).count();
                                if arg_types.len() < required || arg_types.len() > c.params.len() {
                                    return false;
                                }
                                let non_implicit: Vec<&Ty> = c
                                    .params
                                    .iter()
                                    .zip(c.implicit_params.iter())
                                    .filter_map(|(p, im)| (!im).then_some(p))
                                    .collect();
                                non_implicit
                                    .iter()
                                    .zip(arg_types.iter())
                                    .all(|(p, a)| types_plausibly_compatible(p, a))
                            })
                            .collect();
                        if let Some(best) = plausible.first() {
                            let expanded = self.table.normalize_alias_ty(&best.ret);
                            let instantiated = self.table.insert_type_vars(&expanded);
                            return instantiated;
                        }
                    }
                }
            }
            return Ty::error();
        }

        // 5. Look up local candidates (overloads)
        let candidates = self.env.lookup_functions(&callee_name);
        if candidates.is_empty() {
            // Mapping fallback: try forwards then backwards.
            let mut mapping_schemes = self.env.lookup_mappings(&callee_name);
            // Cross-file mapping fallback via symbol_index.
            if mapping_schemes.is_empty() {
                if let Some(db) = self.db {
                    if let Some(file) = self.env.symbol_index.get_file(&callee_name) {
                        let env_data = crate::query::top_level_env(db, file);
                        mapping_schemes = env_data.0.env.lookup_mappings(&callee_name);
                    }
                }
            }
            if let Some(ms) = mapping_schemes.first() {
                if arg_types.len() == 1 {
                    use crate::infer::mapping::{resolve_mapping_call, MappingCallResult};
                    let result =
                        resolve_mapping_call(&mut self.table, &ms.lhs, &ms.rhs, &arg_types[0]);
                    match result {
                        MappingCallResult::Forwards(ty) => {
                            let _ = self.table.unify(&ms.lhs, &arg_types[0]);
                            return self.table.deep_resolve(&ty);
                        }
                        MappingCallResult::Backwards(ty) => {
                            let _ = self.table.unify(&ms.rhs, &arg_types[0]);
                            return self.table.deep_resolve(&ty);
                        }
                        MappingCallResult::NoMatch => {}
                    }
                }
                // Neither direction matched — return rhs as best guess
                return ms.rhs.clone();
            }
            // No function, no mapping — genuinely undefined
            if self.env.has_workspace_context && !is_likely_external_or_generated(&callee_name) {
                self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                    call_expr: callee_id,
                    name: callee_name,
                });
            }
            return Ty::error();
        }

        // 6. Overload resolution: try each candidate
        //
        // so snapshot/unify/rollback flows through the same API as RA's method
        // resolution. MRC is created per-candidate (after freshen_scheme) to
        // avoid holding a &mut self.table borrow across env/scheme accesses.

        let mut assumptions = Vec::new();
        self.fill_assumptions(locals, &mut assumptions);

        let mut count_mismatch: Option<(usize, usize)> = None;
        let mut candidate_errors: Vec<(String, Span, Box<TypeError>)> = Vec::new();
        // Collect all viable candidates for specificity ranking.
        let mut viable: Vec<(usize, Ty, Subst)> = Vec::new(); // (candidate_index, ret_ty, subst)

        // Pre-filter candidates by type shape plausibility.
        // Only active for overloaded functions (2+ candidates).
        // Single-candidate functions always proceed to full unification
        // for proper diagnostic reporting.
        let plausible_indices: Vec<usize> = if candidates.len() <= 1 {
            (0..candidates.len()).collect()
        } else {
            (0..candidates.len())
                .filter(|&ci| {
                    let c = &candidates[ci];
                    let required = c.implicit_params.iter().filter(|im| !**im).count();
                    if arg_types.len() < required || arg_types.len() > c.params.len() {
                        return true; // arity mismatch → keep for diagnostics
                    }
                    let params_to_check: Vec<&Ty> = c
                        .params
                        .iter()
                        .zip(c.implicit_params.iter())
                        .filter_map(|(p, im)| (!im).then_some(p))
                        .collect();
                    params_to_check
                        .iter()
                        .zip(arg_types.iter())
                        .all(|(param, arg)| types_plausibly_compatible(param, arg))
                })
                .collect()
        };

        for &ci in &plausible_indices {
            let raw_candidate = &candidates[ci];
            // Freshen quantifiers to avoid name collision with caller's
            // type variables. E.g., caller has `forall 'n` and callee also
            // has `forall 'n` — without freshening, unification creates
            // circular references causing infinite recursion.
            let (candidate, freshen_info) = self.freshen_scheme(raw_candidate);
            let required =
                candidate.implicit_params.iter().filter(|is_implicit| !**is_implicit).count();
            let total = candidate.params.len();

            // When all explicit params are unit, call with () (0 args)
            // is allowed. E.g. Ctor() for union variants with `Ctor : unit`.
            let unit_adjusted_required = if required == 1
                && candidate.params.len() == 1
                && matches!(candidate.params[0].kind(), TyKind::Scalar(Scalar::Unit))
            {
                0 // f() ≡ f(())
            } else {
                required
            };

            // When a constructor takes a single tuple param AND multiple
            // args are passed, the args are wrapped into a tuple:
            // Ctor(x, y) → Ctor((x, y)). Only effective when the param
            // IS a Tuple type (e.g., from val spec).
            let is_constructor = self.env.constructors.contains_key(&callee_name);
            let tuple_adjusted_total = if is_constructor
                && total == 1
                && arg_types.len() > 1
                && matches!(candidate.params.first().map(|p| p.kind()), Some(TyKind::Tuple(_)))
            {
                arg_types.len()
            } else {
                total
            };

            // Arity check
            if arg_types.len() < unit_adjusted_required || arg_types.len() > tuple_adjusted_total {
                count_mismatch = Some(match count_mismatch {
                    Some((prev_req, prev_total)) => (prev_req.min(required), prev_total.max(total)),
                    None => (required, total),
                });
                continue;
            }

            // Select params (skip implicit if fewer args provided)
            let expected_params: Vec<&Ty> = if arg_types.len() == total {
                candidate.params.iter().collect()
            } else {
                candidate
                    .params
                    .iter()
                    .zip(candidate.implicit_params.iter())
                    .filter_map(|(param, is_implicit)| (!is_implicit).then_some(param))
                    .collect()
            };

            // Outer snapshot: covers the entire candidate evaluation
            // (MRC unification + Subst bridge + constraint check). Rolled
            // back after collecting candidate info for specificity ranking.
            let snap = self.table.snapshot();

            // Collect owned copies of expected params for try_candidate's &[Ty].
            let params_owned: Vec<Ty> = expected_params.iter().map(|p| (*p).clone()).collect();
            let table_ok = {
                let mut mrc =
                    crate::method_resolution::MethodResolutionContext { table: &mut self.table };
                mrc.try_candidate(&params_owned, &arg_types)
            };
            // MRC dropped — self.table accessible again.

            let sig_text = format_scheme_signature(&candidate);

            if !table_ok {
                // MRC already rolled back its inner snapshot. Rollback
                // the outer snapshot too (to undo any partial state).
                self.table.rollback_to(snap);
                // Identify which parameter failed for error reporting.
                // Take a fresh snapshot so we can rollback after the
                // diagnostic unification attempts.
                let diag_snap = self.table.snapshot();
                for (index, (expected, actual)) in
                    expected_params.iter().zip(arg_types.iter()).enumerate()
                {
                    if !self.table.unify(expected, actual) {
                        let err_span = arg_spans.get(index).copied().unwrap_or(callee_span);
                        candidate_errors.push((
                            callee_name.clone(),
                            err_span,
                            Box::new(TypeError::FunctionArg {
                                span: err_span,
                                ty: expected.display_text(),
                                error: Box::new(TypeError::Subtype {
                                    lhs: actual.display_text(),
                                    rhs: expected.display_text(),
                                    constraint: Some(format!(
                                        "in call to `{callee_name}` with signature: {sig_text}"
                                    )),
                                }),
                            }),
                        ));
                        self.table.rollback_to(diag_snap);
                        break;
                    }
                }
                continue;
            }

            // Table unification succeeded (bindings kept by try_candidate).
            // Build Subst from constraint::unify for constraint checking,
            // then use deep_resolve for return type resolution.
            let mut subst = Subst::default();
            for entry in &freshen_info.entries {
                let resolved = self.table.resolve(&entry.infer_ty);
                if !matches!(resolved.kind(), TyKind::Infer(_)) {
                    subst.types.insert(entry.fresh_name.clone(), resolved.clone());
                    subst.values.insert(entry.fresh_name.clone(), resolved.display_text());
                }
            }
            // Extract value bindings from param/arg App type args directly.
            // When param is App("int", [Nexp(Var("'_fv0"))]) and arg is
            // App("int", [Nexp(Var("'sew"))]), record '_fv0 → 'sew.
            for (expected, actual) in expected_params.iter().zip(arg_types.iter()) {
                self.extract_value_bindings(expected, actual, &mut subst);
            }
            // Use deep_resolve for return type (handles TyArg placeholders).
            let ret = self.table.deep_resolve(&candidate.ret);
            // Unify with expected return type to resolve quantifiers.
            if let Some(ref exp_ret) = expected_ret {
                if !ret.is_error() && !exp_ret.is_error() {
                    let _ = self.table.unify(exp_ret, &ret);
                    self.extract_value_bindings(exp_ret, &ret, &mut subst);
                }
            }
            // Constraint instantiation check
            let constraint_error = self.instantiation_error_with_sig(
                &callee_name,
                &candidate.quantifiers,
                &candidate.constraints,
                &subst,
                &assumptions,
                Some(sig_text),
                &arg_spans,
                &freshen_info,
            );
            // Collect viable info before rollback (rollback consumes snap).
            let ret_ty = self.table.deep_resolve(&candidate.ret);
            self.table.rollback_to(snap);
            if let Some(error) = constraint_error {
                candidate_errors.push((callee_name.clone(), callee_span, Box::new(error)));
                continue;
            }
            // Candidate is viable — collect for specificity ranking
            viable.push((ci, ret_ty, subst));
        }

        // B2-4: Pick best candidate by specificity
        if !viable.is_empty() {
            // Score each viable candidate: prefer those with more concrete
            // (non-variable) parameter types ("most specific" rule).
            let best = if viable.len() == 1 {
                0
            } else {
                let mut scored: Vec<(usize, usize)> = viable
                    .iter()
                    .enumerate()
                    .map(|(vi, (ci, _, _))| {
                        let c = &candidates[*ci];
                        let concrete_count = c
                            .params
                            .iter()
                            .filter(|p| {
                                !p.is_error()
                                    && !matches!(
                                        p.kind(),
                                        TyKind::Param(_) | TyKind::Infer(crate::ty::InferTy(_))
                                    )
                            })
                            .count();
                        (vi, concrete_count)
                    })
                    .collect();
                scored.sort_by_key(|b| std::cmp::Reverse(b.1)); // most specific first
                scored[0].0
            };
            let (ci, ret_ty, _subst) = &viable[best];
            // Re-apply the winning candidate's unification to the table
            let winner = &candidates[*ci];
            let expected_params: Vec<&Ty> = if arg_types.len() == winner.params.len() {
                winner.params.iter().collect()
            } else {
                winner
                    .params
                    .iter()
                    .zip(winner.implicit_params.iter())
                    .filter_map(|(param, is_implicit)| (!is_implicit).then_some(param))
                    .collect()
            };
            for (expected, actual) in expected_params.iter().zip(arg_types.iter()) {
                let _ = self.table.unify(expected, actual);
            }
            if let Some(ref exp_ret) = expected_ret {
                if !ret_ty.is_error() && !exp_ret.is_error() {
                    let _ = self.table.unify(exp_ret, ret_ty);
                }
            }
            // Resolve Bidir return types from mapping val specs.
            if let TyKind::Bidir { lhs, rhs } = ret_ty.kind() {
                if !arg_types.is_empty() {
                    if self.table.unify(&arg_types[0], lhs) {
                        return rhs.clone();
                    } else if self.table.unify(&arg_types[0], rhs) {
                        return lhs.clone();
                    }
                }
            }
            return ret_ty.clone();
        }

        // For overloaded functions, some variants may live in other files
        // and lack a `val` declaration so we never built a scheme for them.
        let is_partial_overload = self
            .env
            .overloads
            .get(&callee_name)
            .map(|members| members.iter().any(|m| !self.env.functions.contains_key(m)))
            .unwrap_or(false);

        // No candidate matched
        let has_unknown_arg = arg_types.iter().any(|t| t.is_error());
        if has_unknown_arg {
            return Ty::error();
        }

        if let Some((_expected, actual)) = count_mismatch {
            if !is_partial_overload {
                // May be a false positive due to Sail implicit args.
                self.push_inference_diagnostic(InferenceDiagnostic::MismatchedArgCount {
                    call_expr: callee_id,
                    expected: actual,
                    found: arg_types.len(),
                });
            }
        } else if !candidate_errors.is_empty() && !is_partial_overload {
            // All candidates matched on arity but failed on type
            // unification or constraint checking. Report the first
            // type error so the user knows why the call failed.
            //
            // When all arity-matching candidates fail type checking,
            // the error from the first candidate is propagated.
            if let Some((_name, err_span, type_error)) = candidate_errors.first() {
                match type_error.as_ref() {
                    TypeError::FunctionArg { ty: expected_ty, error, .. } => {
                        if let TypeError::Subtype { lhs: actual_ty, .. } = error.as_ref() {
                            let arg_idx = args
                                .iter()
                                .enumerate()
                                .find(|(_, &a)| {
                                    self.expr_span(body, a).map(|s| s.start) == Some(err_span.start)
                                })
                                .map(|(i, _)| i)
                                .unwrap_or(0);
                            if arg_idx < args.len() {
                                let expected = Ty::named(expected_ty.clone());
                                let actual = Ty::named(actual_ty.clone());
                                self.result.record_type_mismatch(args[arg_idx], &expected, &actual);
                            }
                        }
                    }
                    TypeError::FailedConstraint { constraint, derived_from } => {
                        self.push_inference_diagnostic(
                            InferenceDiagnostic::CallConstraintViolation {
                                call_expr: callee_id,
                                constraint: constraint.clone(),
                                derived_from: derived_from.clone(),
                            },
                        );
                    }
                    TypeError::UnresolvedQuants { id, quants, signature } => {
                        // May be a false positive (constraint propagation
                        // can resolve these in the full compiler).
                        let is_pure_quant = |q: &str| q.starts_with('\'') && !q.contains(' ');
                        let all_pure = quants.iter().all(|q| is_pure_quant(q));
                        if all_pure {
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::UnresolvedCallQuantifiers {
                                    call_expr: callee_id,
                                    id: id.clone(),
                                    quants: quants.clone(),
                                    signature: signature.clone(),
                                },
                            );
                        }
                    }
                    _ => {}
                }
            }
        } else if !is_partial_overload && self.env.has_workspace_context && candidates.is_empty() {
            // Only emit NoOverloading when the function has ZERO
            // candidates. If candidates exist but none matched, this is
            // a type matching issue (often forall instantiation failure),
            // not a missing function. Suppressing when candidates exist
            // eliminates false positives in workspace mode where the
            // function is found but overload resolution is imprecise.
            // Only raise when the candidate tree is fully exhausted.
            //
            // Suppress for names that are likely defined in the Sail
            // standard library or auto-generated by the compiler (e.g.
            // sail_*, float_*, update_*, mapping _forwards/_backwards
            // matchers, uppercase constructors). These functions live in
            // sail/lib/ which is not part of the workspace scan.
            if !is_likely_external_or_generated(&callee_name) {
                self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                    call_expr: callee_id,
                    name: callee_name,
                });
            }
        }
        Ty::error()
    }

    /// Infer with optional expected type (bidirectional).
    /// `InferenceContext::infer_expr_coerce` pattern. After inference,
    /// if there's an expected type we check for a mismatch.
    fn infer_expr_hir_with(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expected: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        let ty = self.infer_expr_hir(body, id, locals);

        // Write inferred type to InferenceResult.
        self.result.write_expr_ty(id, ty.clone());

        if let Expectation::HasType(ref expected_ty) = expected {
            if !ty.is_error() && !expected_ty.is_error() {
                // Use InferenceTable::unify for type checking
                if !self.table.unify(expected_ty, &ty) {
                    self.result.record_type_mismatch(id, expected_ty, &ty);
                }
            }
        }
        ty
    }

    /// Infer the type of an expression in a Body arena.
    /// This is the entry point: `body[id]` → match on Expr.
    #[allow(dead_code)]
    pub(super) fn infer_expr_hir(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) -> Ty {
        // Fuel guard: prevent stack overflow on deeply recursive expressions.
        self.inference_fuel = self.inference_fuel.saturating_sub(1);
        if self.inference_fuel == 0 {
            return Ty::error();
        }
        // Cooperative cancellation (checked each fuel decrement).
        if self.cancel.is_cancelled() {
            return Ty::error();
        }
        use hir_def::Expr;

        let expr_span = self.expr_span(body, id).unwrap_or(Span::new(0, 0));
        let Some(hir) = body.expr(id) else {
            return Ty::error();
        };

        let ty = match hir {
            Expr::Missing | Expr::Error { .. } => Ty::error(),

            Expr::Literal(lit) => infer_literal_type(lit),

            Expr::Ident(name) => {
                self.used_bindings.insert(name.clone());

                // Resolution order: locals → top-level env → ExprScopes →
                // pattern_constants → cross-file → unresolved.

                if let Some(ty) = self.env.lookup_value(locals, name) {
                    ty
                }
                // 2. Functions (resolved at call site, return error type)
                else if self.env.functions.contains_key(name.as_str())
                    || self.env.cross_file_function_names.contains(name.as_str())
                {
                    Ty::error()
                }
                // 3. Cross-file values (let/var/register from other files).
                else if let Some(ty) = self.cross_file_value_type(name) {
                    ty
                }
                // The remaining cases all resolve `name` to a known binding or
                // symbol, so they type as error() with no further inference:
                //   4.  Top-level symbols (constructors, etc. — existence only)
                //   4b. ExprScopes: a local binding not tracked in `locals`
                //       (match arm patterns, foreach iterators in complex spots)
                //   5.  Pattern constants (enum constructors used as patterns)
                //   6.  Name appears as a binding in some pattern in this body
                //       (fallback for patterns bind_pattern_hir didn't register)
                //   6b. Auto-generated names or names that appear as pattern
                //       bindings elsewhere (scattered clause cross-body refs,
                //       funcl as-bindings, etc.)
                else if self.env.top_level_symbol_exists(name)
                    || self.expr_scopes.scope_for(id).is_some_and(|scope_id| {
                        self.expr_scopes
                            .resolve_name_in_scope(scope_id, &hir_def::name::Name::new(name))
                            .is_some()
                    })
                    || self.pattern_constants.contains(name.as_str())
                    || self.name_appears_in_body_patterns(body, name)
                    || name.starts_with("__")
                    || self.name_likely_pattern_binding(name)
                {
                    Ty::error()
                }
                // 7. Genuinely unresolved — emit diagnostic.
                else if self.env.has_workspace_context {
                    self.push_inference_diagnostic(InferenceDiagnostic::UnresolvedIdent {
                        expr: id,
                        name: name.clone(),
                    });
                    Ty::error()
                } else {
                    Ty::error()
                }
            }

            Expr::TypeVar(name) => {
                self.used_bindings.insert(name.clone());
                // In expression context, type variables are integer values.
                Ty::named("int".to_string())
            }

            Expr::BinaryOp { lhs, op, rhs } => {
                let lhs_ty = self.infer_expr_hir(body, *lhs, locals);
                let rhs_ty = self.infer_expr_hir(body, *rhs, locals);
                use hir_def::hir::{BinaryOp, HirBinaryOp};
                match op {
                    HirBinaryOp::Known(BinaryOp::CmpOp(_)) => Ty::named("bool".to_string()),
                    HirBinaryOp::Known(BinaryOp::LogicOp(logic_op)) => {
                        let op_str = match logic_op {
                            hir_def::hir::LogicOp::Or => "|",
                            hir_def::hir::LogicOp::And => "&",
                        };
                        if let Some(ret_ty) =
                            self.resolve_overloaded_binop(op_str, &lhs_ty, &rhs_ty)
                        {
                            ret_ty
                        } else if lhs_ty.is_bits_like() || rhs_ty.is_bits_like() {
                            // Bitwise operation: result is the bitvector type.
                            // |/& are overloaded: logical on bool, bitwise on bits.
                            if !lhs_ty.is_error() {
                                lhs_ty
                            } else {
                                rhs_ty
                            }
                        } else {
                            // When result is bool (not bits), verify both
                            // operands are bool. Short-circuited RHS should still
                            // have a valid bool type.
                            let bool_ty = Ty::named("bool".to_string());
                            if !lhs_ty.is_error() && !self.table.unify(&bool_ty, &lhs_ty) {
                                self.result.record_type_mismatch(*lhs, &bool_ty, &lhs_ty);
                            }
                            if !rhs_ty.is_error() && !self.table.unify(&bool_ty, &rhs_ty) {
                                self.result.record_type_mismatch(*rhs, &bool_ty, &rhs_ty);
                            }
                            bool_ty
                        }
                    }
                    HirBinaryOp::Known(BinaryOp::Concat) => {
                        self.infer_concat_result_type(expr_span, id, &lhs_ty, &rhs_ty)
                    }
                    // List cons (::) — LHS is element, RHS is list(T),
                    // result is list(T). Verify LHS unifies with element type.
                    HirBinaryOp::Known(BinaryOp::Cons) => {
                        // Extract element type from RHS if it's a list
                        let elem_ty = match rhs_ty.kind() {
                            TyKind::App { name, args, .. } if name == "list" => {
                                args.first().and_then(|a| match a {
                                    TyArg::Type(t) => Some(t.clone()),
                                    _ => None,
                                })
                            }
                            _ => None,
                        };
                        if let Some(ref elem) = elem_ty {
                            // Verify LHS (new element) is compatible with list element type
                            if !lhs_ty.is_error()
                                && !elem.is_error()
                                && !self.table.unify(elem, &lhs_ty)
                            {
                                self.result.record_type_mismatch(*lhs, elem, &lhs_ty);
                            }
                            // Result is the list type
                            rhs_ty
                        } else if rhs_ty.is_error() {
                            // RHS is error — construct list type from LHS
                            let text = format!("list({})", lhs_ty.display_text());
                            Ty::app("list", vec![TyArg::Type(lhs_ty)], text)
                        } else {
                            // RHS is not a list — construct list type from LHS
                            let text = format!("list({})", lhs_ty.display_text());
                            Ty::app("list", vec![TyArg::Type(lhs_ty)], text)
                        }
                    }
                    HirBinaryOp::Known(BinaryOp::ArithOp(arith_op)) => {
                        // Check overloaded operator resolution for arithmetic ops.
                        let op_str = match arith_op {
                            hir_def::hir::ArithOp::Add => "+",
                            hir_def::hir::ArithOp::Sub => "-",
                            hir_def::hir::ArithOp::Mul => "*",
                            hir_def::hir::ArithOp::Div => "/",
                            hir_def::hir::ArithOp::Rem => "%",
                            hir_def::hir::ArithOp::Shr => ">>",
                            hir_def::hir::ArithOp::Shl => "<<",
                            hir_def::hir::ArithOp::BitXor => "^",
                            hir_def::hir::ArithOp::BitOr => "|",
                            hir_def::hir::ArithOp::BitAnd => "&",
                        };
                        if let Some(ret_ty) =
                            self.resolve_overloaded_binop(op_str, &lhs_ty, &rhs_ty)
                        {
                            ret_ty
                        } else if lhs_ty.is_bits_like() || rhs_ty.is_bits_like() {
                            // Bitwise operation on bitvectors.
                            if !lhs_ty.is_error() {
                                lhs_ty
                            } else {
                                rhs_ty
                            }
                        } else if !lhs_ty.is_error() {
                            lhs_ty
                        } else {
                            rhs_ty
                        }
                    }
                    // Custom operators (>>>, <<<, etc.) and Pow (**):
                    // resolve through the overloads map first, then fall back
                    // to lhs_ty. Sail's user-defined infix operators are
                    // declared via `overload operator >>> = {rotate_bits_right}`
                    // and must go through the same resolution path.
                    _ => {
                        // Comparison operators always return bool.
                        if op.is_comparison() {
                            return Ty::scalar(crate::ty::Scalar::Bool);
                        }
                        let op_str = op.as_str();
                        if let Some(ret_ty) =
                            self.resolve_overloaded_binop(op_str, &lhs_ty, &rhs_ty)
                        {
                            ret_ty
                        } else if !lhs_ty.is_error() {
                            lhs_ty
                        } else {
                            rhs_ty
                        }
                    }
                }
            }

            Expr::UnaryOp { op, expr } => {
                let inner_ty = self.infer_expr_hir(body, *expr, locals);
                use hir_def::hir::{HirUnaryOp, UnaryOp};
                match op {
                    HirUnaryOp::Known(UnaryOp::Not) => {
                        // ~ is logical NOT on bool, bitwise NOT on bits.
                        if inner_ty.is_bits_like() {
                            inner_ty
                        } else {
                            Ty::named("bool".to_string())
                        }
                    }
                    HirUnaryOp::Known(UnaryOp::Neg) => inner_ty,
                    HirUnaryOp::Custom(_) => Ty::error(),
                }
            }

            Expr::Call { callee, args } => self.infer_call_hir(body, *callee, args, locals),

            Expr::If { cond, then_branch, else_branch } => {
                let saved_diverges = self.diverges;
                // Condition must be bool
                let cond_ty = self.infer_expr_hir(body, *cond, locals);
                if !cond_ty.is_error() {
                    let bool_ty = Ty::named("bool".to_string());
                    if !self.table.unify(&bool_ty, &cond_ty) {
                        self.result.record_type_mismatch(*cond, &bool_ty, &cond_ty);
                    }
                }

                // Flow typing — propagate condition constraints to branches.
                // Extract type narrowings from the condition and apply to
                // then-branch (and negation to else-branch).
                let flow = crate::flow::narrow_from_guard(body, *cond);

                // Then branch: apply positive flow narrowings
                let then_ty = if !flow.is_empty() {
                    locals.push_scope();
                    for (name, narrowed_ty) in flow.iter() {
                        locals.define(name.as_str(), narrowed_ty.clone());
                    }
                    let ty = self.infer_expr_hir(body, *then_branch, locals);
                    locals.pop_scope();
                    ty
                } else {
                    self.infer_expr_hir(body, *then_branch, locals)
                };

                let then_diverges = self.diverges;

                if let Some(else_id) = else_branch {
                    self.diverges = Diverges::Maybe;
                    let else_ty = self.infer_expr_hir(body, *else_id, locals);
                    let else_diverges = self.diverges;
                    // Both branches must diverge for the if to diverge.
                    self.diverges = saved_diverges | (then_diverges & else_diverges);
                    // Unify branches: prefer non-unknown
                    if !then_ty.is_error() && !else_ty.is_error() {
                        let _ = self.table.unify(&then_ty, &else_ty);
                    }
                    if !then_ty.is_error() {
                        then_ty
                    } else {
                        else_ty
                    }
                } else {
                    // if-without-else: control can always skip the then branch,
                    // so divergence is reset. Only the then branch might diverge,
                    // but the overall if does NOT diverge (the false path continues).
                    self.diverges = saved_diverges;
                    Ty::named("unit".to_string())
                }
            }

            Expr::Match { scrutinee, arms } => {
                let scrutinee_ty = self.infer_expr_hir(body, *scrutinee, locals);
                // Use InferTy for match result — will be refined by arm types
                let mut result_ty = self.table.new_type_var();
                for arm in arms {
                    locals.push_scope();
                    // Check for duplicate bindings in pattern tree
                    self.check_pattern_duplicates(body, arm.pat);
                    // Bind pattern variables with scrutinee type
                    self.bind_pattern_hir(body, arm.pat, &scrutinee_ty, locals);
                    // Infer guard — must be bool.
                    let has_guard = arm.guard.is_some();
                    if let Some(guard_id) = arm.guard {
                        let guard_ty = self.infer_expr_hir(body, guard_id, locals);
                        // Check guard is bool
                        if !guard_ty.is_error() {
                            let _ = self.table.unify(&Ty::named("bool".to_string()), &guard_ty);
                        }
                        // Flow typing — extract constraints from guard
                        // and apply as type narrowings for the arm body.
                        //
                        // When a guard constrains a variable, narrow its
                        // type in the arm body scope.
                        let flow_env = crate::flow::narrow_from_guard(body, guard_id);
                        for (name, narrowed_ty) in flow_env.iter() {
                            locals.define(name.as_str(), narrowed_ty.clone());
                        }
                    }
                    // Infer arm body
                    let arm_ty = self.infer_expr_hir(body, arm.body, locals);
                    // Unify with result type
                    if result_ty.is_error() && !arm_ty.is_error() {
                        result_ty = arm_ty;
                    } else if !result_ty.is_error()
                        && !arm_ty.is_error()
                        && !self.table.unify(&result_ty, &arm_ty)
                    {
                        // Guarded arms get lenient treatment.
                        if has_guard {
                            locals.pop_scope();
                            continue;
                        }
                        // Check if this is a dependent-type width mismatch
                        // (same outer constructor, different args) vs a real
                        // type error (completely different types).
                        //
                        // Sail dependent match pattern:
                        //   match 'm { 8 => bits(8), 16 => bits(16) }
                        // Upstream: each arm constrains 'm in its scope.
                        // We lack dependent types → same-constructor arms
                        // with different args are accepted permissively.
                        //
                        // Real mismatch (int vs bool): always report.
                        let resolved_result = self.table.resolve(&result_ty);
                        if is_dependent_width_mismatch(&resolved_result, &arm_ty) {
                            result_ty = Ty::error();
                        } else {
                            self.result.record_type_mismatch(arm.body, &result_ty, &arm_ty);
                        }
                    }
                    locals.pop_scope();
                }
                // Match result type is determined by arm unification above.
                // Do NOT compare with self.return_ty here — the match may
                // be in a let-binding where the expected type differs.

                // Exhaustiveness checking
                self.check_match_exhaustiveness_hir(&scrutinee_ty, body, arms);
                // IncompleteMatch stays in legacy diagnostic path
                // for now because the exhaustiveness checker produces
                // false positives for some enum types. The LSP skips
                // legacy diagnostics (b). Migration to
                // InferenceDiagnostic deferred until match_check
                // precision improves.
                result_ty
            }

            Expr::Block(stmts) => {
                locals.push_scope();
                let saved_diverges = self.diverges;
                self.diverges = Diverges::Maybe;
                let mut last_ty = Ty::named("unit".to_string());
                for stmt in stmts {
                    // after a diverging expression (return/throw/exit).
                    if self.diverges.is_always() {
                        break;
                    }
                    match stmt {
                        hir_def::Statement::Let { pat, value } => {
                            // Extract type annotation from Typed pattern
                            let annotated_ty = match body.pat(*pat) {
                                Some(hir_def::Pat::Typed { ty_span, .. }) => {
                                    let ty_text = &self.source[ty_span.start..ty_span.end];
                                    let ty = type_from_type_text(ty_text);
                                    if !ty.is_error() {
                                        Some(ty)
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            let val_ty = if let Some(ref ann_ty) = annotated_ty {
                                // Temporarily set expected_return so List/Vector
                                // handlers can extract expected element types
                                let saved = locals.expected_return.take();
                                locals.expected_return = Some(ann_ty.clone());
                                let ty = self.infer_expr_hir_with(
                                    body,
                                    *value,
                                    &Expectation::HasType(ann_ty.clone()),
                                    locals,
                                );
                                locals.expected_return = saved;
                                ty
                            } else {
                                self.infer_expr_hir(body, *value, locals)
                            };
                            let bind_ty = annotated_ty.unwrap_or(val_ty);
                            self.bind_pattern_hir(body, *pat, &bind_ty, locals);
                            self.check_pattern_duplicates(body, *pat);
                            last_ty = Ty::named("unit".to_string());
                        }
                        hir_def::Statement::Var { pat, value } => {
                            let val_ty = self.infer_expr_hir(body, *value, locals);
                            self.bind_pattern_hir(body, *pat, &val_ty, locals);
                            self.check_pattern_duplicates(body, *pat);
                            last_ty = Ty::named("unit".to_string());
                        }
                        hir_def::Statement::Expr(e) => {
                            last_ty = self.infer_expr_hir(body, *e, locals);
                            // Propagate constraints from assert/if-throw
                            self.propagate_hir_constraints(body, *e, locals);
                        }
                    }
                }
                locals.pop_scope();
                let block_diverges = self.diverges;
                self.diverges = block_diverges | saved_diverges;
                // If block unconditionally diverges (return/throw/exit),
                // its type is "never" — represented as Ty::error() which
                // is transparent in the unifier (unify.rs:503).
                //
                // Diverging block → "never" type (transparent in unifier).
                if block_diverges.is_always() {
                    Ty::error()
                } else {
                    last_ty
                }
            }

            Expr::Let { pat, value, body: let_body } => {
                let annotated_ty = match body.pat(*pat) {
                    Some(hir_def::Pat::Typed { ty_span, .. }) => {
                        let ty_text = &self.source[ty_span.start..ty_span.end];
                        let ty = type_from_type_text(ty_text);
                        if !ty.is_error() {
                            Some(ty)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let val_ty = if let Some(ref ann_ty) = annotated_ty {
                    let saved = locals.expected_return.take();
                    locals.expected_return = Some(ann_ty.clone());
                    let ty = self.infer_expr_hir_with(
                        body,
                        *value,
                        &Expectation::HasType(ann_ty.clone()),
                        locals,
                    );
                    locals.expected_return = saved;
                    ty
                } else {
                    self.infer_expr_hir(body, *value, locals)
                };
                let bind_ty = annotated_ty.unwrap_or(val_ty);
                locals.push_scope();
                self.bind_pattern_hir(body, *pat, &bind_ty, locals);
                self.check_pattern_duplicates(body, *pat);

                // Extract existential constraints from explicit type
                // annotations on let bindings.
                //
                // When the user writes
                // `let x : {'n, 'n > 0. bits('n)} = ...`, we extract the
                // constraint `'n > 0` and add it to the scope for downstream
                // type narrowing and Z3 solving.
                if let TyKind::Exist { constraint, .. } = bind_ty.kind() {
                    locals.add_constraint(constraint.clone());
                }

                let ty = self.infer_expr_hir(body, *let_body, locals);
                locals.pop_scope();
                ty
            }

            Expr::Var { target, value, body: var_body } => {
                // Expr::Var { target, value, body } — expression-form var binding.
                // Note: block-form var bindings now go through Statement::Var .
                // This path handles the `var x = v in body` expression form.
                let val_ty = self.infer_expr_hir(body, *value, locals);
                if let Some(hir_def::Expr::Ident(name)) = body.expr(*target) {
                    locals.push_scope();
                    locals.define(name, val_ty);
                    let ty = self.infer_expr_hir(body, *var_body, locals);
                    locals.pop_scope();
                    ty
                } else {
                    self.infer_expr_hir(body, *var_body, locals)
                }
            }

            Expr::Return(e) | Expr::Throw(e) => {
                self.infer_expr_hir(body, *e, locals);
                self.diverges = Diverges::Always;
                Ty::error()
            }

            Expr::Exit(_) => {
                self.diverges = Diverges::Always;
                Ty::error()
            }

            Expr::Assert { cond, message } => {
                self.infer_expr_hir(body, *cond, locals);
                if let Some(m) = message {
                    self.infer_expr_hir(body, *m, locals);
                }
                Ty::named("unit".to_string())
            }

            Expr::Tuple(items) => {
                let tys: Vec<Ty> =
                    items.iter().map(|&i| self.infer_expr_hir(body, i, locals)).collect();
                Ty::tuple(tys)
            }

            Expr::List(items) => {
                // Try to get expected element type from function return
                let expected_elem =
                    locals.expected_return.as_ref().and_then(|ret| match ret.kind() {
                        TyKind::App { name, args, .. } if name == "list" => {
                            args.first().and_then(|a| match a {
                                TyArg::Type(t) => Some(t.clone()),
                                _ => None,
                            })
                        }
                        _ => None,
                    });
                let mut elem_ty = expected_elem.clone();
                for &i in items {
                    let ty = self.infer_expr_hir(body, i, locals);
                    // Check element against expected element type
                    if let Some(ref expected) = expected_elem {
                        if !ty.is_error()
                            && !expected.is_error()
                            && !self.table.unify(expected, &ty)
                        {
                            self.result.record_type_mismatch(i, expected, &ty);
                        }
                    }
                    if elem_ty.is_none() && !ty.is_error() {
                        elem_ty = Some(ty);
                    }
                }
                let elem = elem_ty.unwrap_or(Ty::error());
                let text = format!("list({})", elem.display_text());
                Ty::app("list", vec![TyArg::Type(elem)], text)
            }

            Expr::Array(items) => {
                // Unify all element types against the first non-error
                // element type.
                let mut elem_ty: Option<Ty> = None;
                for &i in items {
                    let ty = self.infer_expr_hir(body, i, locals);
                    if ty.is_error() {
                        continue;
                    }
                    match elem_ty {
                        None => {
                            elem_ty = Some(ty);
                        }
                        Some(ref expected) => {
                            if !self.table.unify(expected, &ty) {
                                self.result.record_type_mismatch(i, expected, &ty);
                            }
                        }
                    }
                }
                let elem = elem_ty.unwrap_or(Ty::error());
                let len = items.len();
                let text = format!("vector({len}, {})", elem.display_text());
                Ty::app("vector", vec![TyArg::numeric(len.to_string()), TyArg::Type(elem)], text)
            }

            Expr::Field { expr, field } => {
                let base_ty = self.infer_expr_hir(body, *expr, locals);
                // Record field lookup with generic type argument instantiation
                if let Some(field_ty) = self.record_field_type(&base_ty, field) {
                    // Record which struct this field access resolves to.
                    // Enables goto-def for `x.field` → navigate to struct definition.
                    if let Some((record_name, _, _)) = self.record_info_for_type(&base_ty) {
                        // Look up the struct's DefId from the DefMap
                        let name_key = hir_def::Name::from(record_name.as_str());
                        let per_ns = self.make_resolver().def_map().root_scope().get(&name_key);
                        if let Some(struct_item) = per_ns.types {
                            self.result.record_field_resolution(id, struct_item.def);
                        }
                    }
                    return field_ty;
                }
                // Bitfield field lookup
                let base_name = match base_ty.kind() {
                    TyKind::Adt(name, _) => Some(name.clone()),
                    TyKind::App { name, .. } => Some(name.clone()),
                    _ => None,
                };
                if let Some(ref name) = base_name {
                    if let Some(bf) = self.env.bitfields.get(name) {
                        // Check named bitfield fields first
                        if let Some(field_ty) = bf.fields.get(field.as_str()) {
                            return field_ty.clone();
                        }
                        // Every bitfield has a synthetic `bits` field
                        // containing the full bitvector.
                        if field == "bits" {
                            return bf.underlying.clone();
                        }
                    }
                }
                // Cross-file unresolved record: emit hint
                if !base_ty.is_error() && !is_known_non_record(&base_ty) {
                    self.push_hint(
                        expr_span,
                        format!(
                            "Record type `{}` is defined in another file and could not be resolved; field access is unverified.",
                            base_ty.display_text()
                        ),
                    );
                }
                // D6: Emit typed InferenceDiagnostic::UnresolvedField.
                if !base_ty.is_error() {
                    let method_with_same_name_exists = self.env.functions.contains_key(field)
                        || self.env.cross_file_function_names.contains(field.as_str());
                    self.push_inference_diagnostic(InferenceDiagnostic::UnresolvedField {
                        expr: id,
                        receiver: base_ty.clone(),
                        name: field.clone(),
                        method_with_same_name_exists,
                    });
                }
                Ty::error()
            }

            Expr::Assign { target, value } => {
                // Imperative binding `name : type = expr` creates an implicit
                // variable declaration.
                if let Some(hir_def::Expr::Cast { expr: inner, .. }) = body.expr(*target) {
                    if let Some(hir_def::Expr::Ident(name)) = body.expr(*inner) {
                        // Register as local binding (like `var name`)
                        locals.define(name, Ty::error());
                    }
                }
                // Bare `name = expr` — register in locals if not already known,
                // to prevent false UnresolvedIdent for subsequent references.
                // This is permissive (accepts some invalid code) but avoids FPs.
                if let Some(hir_def::Expr::Ident(name)) = body.expr(*target) {
                    if !locals.bindings.contains_key(name) {
                        locals.define(name, Ty::error());
                    }
                }

                // LE_app(f, xs) = exp desugars to E_app(f, xs @ [exp]).
                if let Some(hir_def::Expr::Call { callee, args }) = body.expr(*target) {
                    self.infer_expr_hir(body, *callee, locals);
                    for &a in args {
                        self.infer_expr_hir(body, a, locals);
                    }
                    self.infer_expr_hir(body, *value, locals);
                } else {
                    self.infer_expr_hir(body, *target, locals);
                    let val_ty = self.infer_expr_hir(body, *value, locals);
                    // Update the local's type from the value
                    if let Some(hir_def::Expr::Cast { expr: inner, .. }) = body.expr(*target) {
                        if let Some(hir_def::Expr::Ident(name)) = body.expr(*inner) {
                            locals.define(name, val_ty.clone());
                        }
                    }
                }
                Ty::named("unit".to_string())
            }

            Expr::While { cond, body: lb } | Expr::Repeat { body: lb, until: cond } => {
                self.infer_expr_hir(body, *cond, locals);
                self.infer_expr_hir(body, *lb, locals);
                Ty::named("unit".to_string())
            }

            // Foreach now has pat: PatId.
            Expr::Foreach { pat, start, end, step, body: lb } => {
                let start_ty = self.infer_expr_hir(body, *start, locals);
                let _end_ty = self.infer_expr_hir(body, *end, locals);
                if let Some(step_id) = step {
                    self.infer_expr_hir(body, *step_id, locals);
                }
                locals.push_scope();
                let iter_ty =
                    if !start_ty.is_error() { start_ty } else { Ty::named("int".to_string()) };
                // Use bind_pattern_hir instead of locals.define
                self.bind_pattern_hir(body, *pat, &iter_ty, locals);
                self.infer_expr_hir(body, *lb, locals);
                locals.pop_scope();
                Ty::named("unit".to_string())
            }

            Expr::Cast { expr, .. } => {
                // Cast returns the target type; we don't have the type node
                // in Expr, so infer the inner expr type for now
                self.infer_expr_hir(body, *expr, locals)
            }

            Expr::Try { scrutinee, arms } => {
                // Try/catch with proper exception type handling.
                //
                // try { body } catch { pat => handler, ... }
                //
                // - body has type T (the "happy path" type)
                // - catch arm patterns match exception values
                //   (Sail exceptions are untyped — patterns match structurally)
                // - catch arm bodies must also produce type T
                // - overall expression type: T
                let body_ty = self.infer_expr_hir(body, *scrutinee, locals);
                let mut result_ty =
                    if body_ty.is_error() { self.table.new_type_var() } else { body_ty.clone() };

                for arm in arms {
                    locals.push_scope();
                    // Bind catch pattern — exception type is opaque
                    // (Sail exceptions are a built-in union, patterns
                    // match structurally against thrown values)
                    self.bind_pattern_hir(body, arm.pat, &Ty::error(), locals);

                    // Check guard (must be bool)
                    if let Some(guard_id) = arm.guard {
                        let guard_ty = self.infer_expr_hir(body, guard_id, locals);
                        if !guard_ty.is_error() {
                            let _ = self.table.unify(&Ty::named("bool".to_string()), &guard_ty);
                        }
                    }

                    // Infer catch handler body
                    let arm_ty = self.infer_expr_hir(body, arm.body, locals);

                    // Unify with result type (catch must return same type as try body)
                    if result_ty.is_error() && !arm_ty.is_error() {
                        result_ty = arm_ty;
                    } else if !result_ty.is_error()
                        && !arm_ty.is_error()
                        && !self.table.unify(&result_ty, &arm_ty)
                    {
                        self.result.record_type_mismatch(arm.body, &result_ty, &arm_ty);
                    }
                    locals.pop_scope();
                }

                // Track throw/catch effects
                self.observed_effects.insert(hir_def::EffectTag::Throw);

                result_ty
            }

            Expr::Ref(name) => {
                // Register reference: &reg
                if let Some(ty) = self.env.registers.get(name.as_str()) {
                    register_ty(ty.clone())
                } else {
                    Ty::error()
                }
            }

            Expr::Config(_keys) => {
                // Config expressions are compile-time pure constants whose
                // type is determined by the expected context. Return
                // Ty::error() since the LSP cannot determine the type.
                Ty::error()
            }

            Expr::SizeOf { nexp, .. } => {
                // sizeof('n) has type atom('n).
                // Parse the nexp text to a NumericExpr and construct atom(nexp).
                if let Some(parsed) = crate::ty::NumericExpr::parse(nexp) {
                    Ty::app("atom", vec![TyArg::Nexp(parsed)], format!("atom({nexp})"))
                } else {
                    // Complex nexp — use TyArg::Value fallback.
                    Ty::app("atom", vec![TyArg::numeric(nexp.clone())], format!("atom({nexp})"))
                }
            }

            Expr::Constraint(_) => Ty::named("bool".to_string()),

            Expr::Struct { name, fields } => {
                // Named struct: look up record info with type arg instantiation
                // Try to get type arguments from expected return type
                let expected_ret = locals.expected_return.clone();
                let record_resolved = name.as_ref().and_then(|sname| {
                    // Try instantiated lookup from expected return type
                    if let Some(ref ret_ty) = expected_ret {
                        if let Some((rname, info, subst)) = self.record_info_for_type(ret_ty) {
                            if rname == *sname {
                                return Some((sname.clone(), info, subst));
                            }
                        }
                    }
                    // Fallback to plain lookup
                    self.env
                        .records
                        .get(sname.as_str())
                        .cloned()
                        .map(|r| (sname.clone(), r, Subst::default()))
                });
                if let Some((sname, record, type_subst)) = record_resolved {
                    // Check each field with instantiated types
                    for (fname, field_expr) in fields {
                        let value_ty = self.infer_expr_hir(body, *field_expr, locals);
                        if let Some(raw_ty) = record.fields.get(fname.as_str()) {
                            let expected_ty = apply_subst(raw_ty, &type_subst);
                            if !value_ty.is_error()
                                && !expected_ty.is_error()
                                && !self.table.unify(&expected_ty, &value_ty)
                            {
                                self.result.record_type_mismatch(
                                    *field_expr,
                                    &expected_ty,
                                    &value_ty,
                                );
                            }
                        }
                    }
                    // Check for missing required fields.
                    // (hir-ty/src/diagnostics/expr.rs:555-596).
                    let provided: HashSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                    let mut missing: Vec<String> = record
                        .fields
                        .keys()
                        .filter(|k| !provided.contains(k.as_str()))
                        .map(|s| s.to_string())
                        .collect();
                    if !missing.is_empty() {
                        missing.sort();
                        self.push_inference_diagnostic(InferenceDiagnostic::MissingFields {
                            expr: id,
                            record_name: sname.clone(),
                            missing,
                        });
                    }
                    Ty::named(sname)
                } else {
                    // Unnamed struct
                    for (_, field_expr) in fields {
                        self.infer_expr_hir(body, *field_expr, locals);
                    }
                    Ty::error()
                }
            }

            Expr::Update { base, fields } => {
                let base_ty = self.infer_expr_hir(body, *base, locals);
                // Check field types against base record
                let base_name = match base_ty.kind() {
                    TyKind::Adt(name, _) => Some(name.clone()),
                    TyKind::App { name, .. } => Some(name.clone()),
                    _ => None,
                };
                let record_info =
                    base_name.as_ref().and_then(|rn| self.env.records.get(rn.as_str()).cloned());
                if let Some(record) = record_info {
                    let _rname = base_name.as_ref().unwrap();
                    for (fname, field_expr) in fields {
                        let value_ty = self.infer_expr_hir(body, *field_expr, locals);
                        if let Some(expected_ty) = record.fields.get(fname.as_str()) {
                            if !value_ty.is_error()
                                && !expected_ty.is_error()
                                && !self.table.unify(expected_ty, &value_ty)
                            {
                                self.result.record_type_mismatch(
                                    *field_expr,
                                    expected_ty,
                                    &value_ty,
                                );
                            }
                        }
                    }
                } else {
                    for (_, field_expr) in fields {
                        self.infer_expr_hir(body, *field_expr, locals);
                    }
                }
                base_ty
            }

            Expr::Attribute { expr } => self.infer_expr_hir(body, *expr, locals),

            Expr::Index { base, index } => {
                let base_ty = self.infer_expr_hir(body, *base, locals);
                let index_ty = self.infer_expr_hir(body, *index, locals);
                // Check index is int
                if !index_ty.is_error() {
                    let int_ty = Ty::named("int".to_string());
                    if !self.table.unify(&int_ty, &index_ty) {
                        self.result.record_type_mismatch(*index, &int_ty, &index_ty);
                    }
                }
                // Bounds check for known-width vectors (diagnostic hint).
                if let Some(info) = self.concat_operand_info(&base_ty) {
                    let idx_span = self.expr_span(body, *index).unwrap_or(expr_span);
                    let idx_text = self.source.get(idx_span.start..idx_span.end).unwrap_or("");
                    if let Ok(idx_val) = idx_text.trim().parse::<usize>() {
                        if let Ok(width) = info.width.parse::<usize>() {
                            if idx_val >= width {
                                self.push_inference_diagnostic(
                                    InferenceDiagnostic::ConstraintViolation {
                                        expr: *index,
                                        constraint: format!("0 <= {} < {}", idx_val, width),
                                        derived_from: vec![idx_span],
                                    },
                                );
                            }
                        }
                    }
                }
                self.concat_operand_info(&base_ty).map(|info| info.elem).unwrap_or(Ty::error())
            }

            Expr::Subrange { base, hi, lo } => {
                let base_ty = self.infer_expr_hir(body, *base, locals);
                let _hi_ty = self.infer_expr_hir(body, *hi, locals);
                let _lo_ty = self.infer_expr_hir(body, *lo, locals);
                // Compute result width from hi/lo if statically known
                let hi_span = self.expr_span(body, *hi).unwrap_or(expr_span);
                let lo_span = self.expr_span(body, *lo).unwrap_or(expr_span);
                let hi_text = self.source.get(hi_span.start..hi_span.end).unwrap_or("");
                let lo_text = self.source.get(lo_span.start..lo_span.end).unwrap_or("");
                if let (Ok(h), Ok(l)) =
                    (hi_text.trim().parse::<usize>(), lo_text.trim().parse::<usize>())
                {
                    // Check vector subrange order against declared order.
                    use hir_def::type_error::VectorOrder;
                    let order_violation = match self.env.vector_order {
                        VectorOrder::Dec => h < l,
                        VectorOrder::Inc => h > l,
                    };
                    if order_violation {
                        self.push_inference_diagnostic(InferenceDiagnostic::VectorSubrangeOrder {
                            expr: id,
                            first: h.to_string(),
                            second: l.to_string(),
                            order: self.env.vector_order,
                        });
                    }
                    let width = if h >= l { h - l + 1 } else { l - h + 1 };
                    Ty::app(
                        "bits",
                        vec![TyArg::numeric(width.to_string())],
                        format!("bits({width})"),
                    )
                } else if !base_ty.is_error() {
                    // Dynamic subrange: width unknown statically.
                    // Return bits(?N) — fresh inference var is permissive,
                    // avoids false positives from returning the full base
                    // width (e.g. bits(128) when the slice is bits(32)).
                    //
                    // Dynamic subrange: use a fresh var since we lack a
                    // full nexp solver for exact width computation.
                    let width_var = self.table.new_type_var();
                    Ty::app("bits", vec![TyArg::Type(width_var)], "bits(_)".to_string())
                } else {
                    Ty::error()
                }
            }
        };
        ty
    }

    /// Infer a single callable body via the HIR arena path.
    /// `name` is used to look up the expected TypeScheme from the env.
    pub fn infer_callable_body_hir(
        &mut self,
        name: &str,
        callable_body: &hir_def::bodies::CallableBody,
    ) {
        // body and source_map are set at construction (new_for_body).
        // No reset_table needed — fresh context per callable.
        // Clone the Arc to get &Body without borrowing self.
        let body_arc = self.body.clone();
        let body: &hir_def::Body = &body_arc;

        // Look up expected scheme from env (val spec or inline signature)
        let expected_scheme: Option<Arc<TypeScheme>> = {
            let local = self.env.functions.get(name).and_then(|schemes| schemes.first().cloned());
            // If local scheme has error return type (scattered clause without
            // inline return type), try cross-file val spec lookup.
            let needs_cross_file = local.as_ref().map(|s| s.ret.is_error()).unwrap_or(true);
            if needs_cross_file {
                if let Some(db) = self.db {
                    if let Some(file) = self.env.symbol_index.get_file(name) {
                        let env_data = crate::query::top_level_env(db, file);
                        if let Some(cross) =
                            env_data.0.env.functions.get(name).and_then(|s| s.first().cloned())
                        {
                            if !cross.ret.is_error() {
                                Some(cross)
                            } else {
                                local
                            }
                        } else {
                            local
                        }
                    } else {
                        local
                    }
                } else {
                    local
                }
            } else {
                local
            }
        };

        // Set self.return_ty
        if let Some(ref scheme) = expected_scheme {
            self.return_ty = scheme.ret.clone();
            // Only explicit `pure` annotation makes a function effect-free.
            // Absent effect clause means "infer effects", not "pure".
            self.is_pure_context = scheme.is_declared_pure;
        } else {
            self.return_ty = Ty::error();
            self.is_pure_context = false; // no scheme → unknown purity
        }

        let mut locals = LocalEnv::new(expected_scheme.as_ref().map(|scheme| scheme.ret.clone()));

        // Bind parameter patterns with expected types
        if let Some(scheme) = &expected_scheme {
            let mut param_tys = scheme.params.clone();
            if param_tys.len() == 1 && body.params.len() > 1 {
                if let TyKind::Tuple(items) = param_tys[0].kind() {
                    if items.len() == body.params.len() {
                        param_tys = items.clone();
                    }
                }
            }
            for (pat_id, expected_ty) in body.params.iter().zip(param_tys.iter()) {
                self.bind_pattern_hir(body, *pat_id, expected_ty, &mut locals);
            }
            // Extra params without types
            for &pat_id in body.params.iter().skip(param_tys.len()) {
                self.bind_pattern_hir(body, pat_id, &Ty::error(), &mut locals);
            }
        } else {
            for &pat_id in body.params.iter() {
                self.bind_pattern_hir(body, pat_id, &Ty::error(), &mut locals);
            }
        }

        // Infer the body expression with expected return type
        let expectation = match &expected_scheme {
            Some(scheme) if !scheme.ret.is_error() => Expectation::HasType(scheme.ret.clone()),
            _ => Expectation::None,
        };
        let body_ty = self.infer_expr_hir_with(body, body.root(), &expectation, &mut locals);
        // Resolve inference variables before recording
        let resolved_ty = self.table.resolve_or_unknown(&body_ty);
        self.record_expr_type(callable_body.body_span, &resolved_ty);

        // Check body type against declared return type.
        //
        // (`hir-ty/src/infer/expr.rs:1378-1384` + `coerce.rs:1510-1517`):
        // if coercion fails, record TypeMismatch.
        //
        // Check body type against declared return type.
        if let Some(scheme) = &expected_scheme {
            let ret_ty = &scheme.ret;
            if !ret_ty.is_error()
                && !resolved_ty.is_error()
                && !self.table.unify(ret_ty, &resolved_ty)
            {
                self.result.record_type_mismatch(body.root(), ret_ty, &resolved_ty);
            }
        }

        // Effects enforcement.
        // Copy observed effects from the pre-computed CallableBody.effects
        // (these were collected during CST→HIR lowering in bodies.rs).
        self.observed_effects = callable_body.effects.clone();

        // If function is pure but has impure effects, emit diagnostics.
        if self.is_pure_context && !self.observed_effects.is_empty() {
            for &effect in &self.observed_effects {
                // Skip effects that are always acceptable
                if matches!(
                    effect,
                    hir_def::EffectTag::IncompleteMatch | hir_def::EffectTag::Scattered
                ) {
                    continue;
                }
                // Removed duplicate push_error(EffectMismatch) — now only
                // emitted via InferenceDiagnostic (: hir-ty emits typed
                // diagnostics, not legacy Diagnostic).
                self.push_inference_diagnostic(InferenceDiagnostic::EffectViolation {
                    expr: body.root(),
                    effect,
                    context: "pure function",
                });
                break; // One diagnostic per function, not per effect
            }
        }
    }

    /// Infer a mapping body via the HIR arena path.
    /// Replaces `check_mapping_definition` / `check_mapping_body`.
    pub fn infer_mapping_body_hir(
        &mut self,
        name: &str,
        _callable_body: &hir_def::bodies::CallableBody,
    ) {
        // body and source_map set at construction.
        let body_arc = self.body.clone();
        let body: &hir_def::Body = &body_arc;

        // Look up mapping scheme for input/output types
        let mapping_scheme =
            self.env.mappings.get(name).and_then(|schemes| schemes.first()).cloned();
        let (input_ty, output_ty) = match &mapping_scheme {
            Some(ms) => (ms.lhs.clone(), ms.rhs.clone()),
            None => (Ty::error(), Ty::error()),
        };

        for arm in &body.mapping_arms {
            let mut locals = LocalEnv::new(None);

            // Determine expected types based on direction.
            //
            // Forwards: lhs ← input, rhs ← output
            // Backwards: lhs ← output, rhs ← input
            // Bidir: lhs ← input, rhs ← output (both directions checked)
            let (lhs_expected, rhs_expected) = match arm.direction {
                hir_def::MappingDirection::Forwards => (&input_ty, &output_ty),
                hir_def::MappingDirection::Backwards => (&output_ty, &input_ty),
                hir_def::MappingDirection::Bidirectional => (&input_ty, &output_ty),
            };

            // Bind LHS pattern
            if let Some(pat_id) = arm.lhs_pat {
                self.bind_pattern_hir(body, pat_id, lhs_expected, &mut locals);
            }
            // Bind RHS pattern.
            if let Some(pat_id) = arm.rhs_pat {
                self.bind_pattern_hir(body, pat_id, rhs_expected, &mut locals);
            }

            // Infer guard
            if let Some(guard_id) = arm.guard {
                let guard_ty = self.infer_expr_hir(body, guard_id, &mut locals);
                if !guard_ty.is_error()
                    && !self.table.unify(&Ty::named("bool".to_string()), &guard_ty)
                {
                    let bool_ty = Ty::named("bool".to_string());
                    self.result.record_type_mismatch(guard_id, &bool_ty, &guard_ty);
                }
            }

            // Infer LHS and RHS expressions
            self.infer_expr_hir(body, arm.lhs_expr, &mut locals);
            self.infer_expr_hir(body, arm.rhs_expr, &mut locals);

            // Bidirectional symmetry check: binding names must appear on both sides.
            // Only run with workspace context — without it, we can't distinguish
            // constructor names from binding names (lowercase constructors like
            // `float_class_negative_inf` would be misidentified as bindings).
            if arm.direction == hir_def::MappingDirection::Bidirectional
                && self.env.has_workspace_context
            {
                let lhs_names =
                    arm.lhs_pat.map(|p| self.collect_hir_pat_bindings(body, p)).unwrap_or_default();
                let rhs_names =
                    arm.rhs_pat.map(|p| self.collect_hir_pat_bindings(body, p)).unwrap_or_default();
                for name in lhs_names.keys() {
                    if !rhs_names.contains_key(name) {
                        self.push_inference_diagnostic(
                            InferenceDiagnostic::MappingBindingMismatch {
                                expr: body.root(),
                                name: name.clone(),
                                side: "left",
                            },
                        );
                    }
                }
                for name in rhs_names.keys() {
                    if !lhs_names.contains_key(name) {
                        self.push_inference_diagnostic(
                            InferenceDiagnostic::MappingBindingMismatch {
                                expr: body.root(),
                                name: name.clone(),
                                side: "right",
                            },
                        );
                    }
                }
            }
        }
    }

    // `collect_hir_pat_bindings`, `collect_hir_pat_bindings_inner`,
    // and `collect_vector_subrange_parts_hir` moved to `infer/pat.rs`.

    /// Propagate constraints from assert/if-throw expressions.
    ///
    /// Extracts constraints from assert conditions and adds them
    /// to the environment for downstream type narrowing.
    fn propagate_hir_constraints(
        &self,
        body: &hir_def::Body,
        expr_id: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) {
        use hir_def::Expr;
        let Some(hir) = body.expr(expr_id) else {
            return;
        };
        match hir {
            Expr::Assert { cond, .. } => {
                // Use narrow_from_guard (HIR-based) for type narrowing.
                let flow = crate::flow::narrow_from_guard(body, *cond);
                for (name, narrowed_ty) in flow.iter() {
                    locals.define(name.as_str(), narrowed_ty.clone());
                }
                // Also try text-based constraint extraction for Z3 solver
                if let Some(cond_span) = self.expr_span(body, *cond) {
                    if let Some(cond_text) = self.source.get(cond_span.start..cond_span.end) {
                        if let Some(constraint) = constraint_expr_from_expr_text(cond_text) {
                            locals.add_constraint(constraint);
                        }
                    }
                }
            }
            Expr::If { cond, then_branch, else_branch: None } => {
                // if cond then throw/exit — negate the condition
                let is_diverging =
                    matches!(body.expr(*then_branch), Some(Expr::Throw(_) | Expr::Exit(_)));
                if is_diverging {
                    if let Some(cond_span) = self.expr_span(body, *cond) {
                        if let Some(cond_text) = self.source.get(cond_span.start..cond_span.end) {
                            if let Some(constraint) = constraint_expr_from_expr_text(cond_text) {
                                locals.add_constraint(negate_constraint(constraint));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}
