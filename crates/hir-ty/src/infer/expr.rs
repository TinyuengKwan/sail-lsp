use super::*;

/// Whether `name` is bound by the compiler rather than by a definition.
///
/// Every entry names something upstream generates or matches by name, with
/// the reference beside it. Anything that can instead be *registered* when
/// the definition that generates it is seen belongs there and not here: a
/// name-shape rule silences a typo as readily as it silences a real
/// generated name.
/// Optimal string alignment distance — Levenshtein plus adjacent
/// transposition, which is `Util.levenshtein_distance ~osa:true`.
fn osa_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

fn is_likely_external_or_generated(name: &str) -> bool {
    // Typechecker intrinsics: these are matched by name inside
    // `check_exp` and have no `val` declaration anywhere
    // (type_check.ml:2284-2300, :2657).
    if matches!(name, "_prove" | "_not_prove" | "_check" | "_not_check" | "_assume") {
        return true;
    }
    // `undefined_T` is generated for every type the program defines
    // (`generate_undefineds`, initial_check.ml:2550), on top of the twelve
    // built-in ones declared at initial_check.ml:2529-2546.
    name == "internal_pick" || name.starts_with("undefined_")
}

#[derive(Clone)]
struct FreshenEntry {
    fresh_name: String,
    orig_name: String,
    infer_ty: Ty,
}

impl InferenceContext<'_> {
    /// A callable can be public while retaining a private nominal type in
    /// its signature (notably generated enum conversions). Alias support is
    /// expanded first: a private abbreviation of int is not a private ADT.
    fn unavailable_nominal_type<'a>(
        &self,
        types: impl IntoIterator<Item = &'a Ty>,
    ) -> Option<String> {
        use hir_def::source_order::{
            self,
            bindings::{self, Namespace},
        };
        let db = self.db?;
        let at = source_order::for_body(db, self.owner?)?;
        source_order::program(db, at.file)?;
        fn visit(
            ctx: &InferenceContext<'_>,
            db: &dyn salsa::Database,
            at: source_order::Declaration,
            ty: &Ty,
            seen: &mut HashSet<Ty>,
        ) -> Option<String> {
            let ty = ctx.table.normalize_alias_ty(ty);
            if !seen.insert(ty.clone()) {
                return None;
            }
            match ty.kind() {
                TyKind::Named(name, args) | TyKind::App { name, args, .. } => {
                    if !hir_def::is_builtin_type_name(name) {
                        let key = hir_def::Name::new(name);
                        if bindings::resolve(db, at, key.clone(), Namespace::Type).is_none()
                            && bindings::declared_before(db, at, key, Namespace::Type).is_some()
                        {
                            return Some(name.clone());
                        }
                    }
                    args.iter().find_map(|arg| match arg {
                        TyArg::Type(ty) => visit(ctx, db, at, ty, seen),
                        _ => None,
                    })
                }
                TyKind::Tuple(types) => types.iter().find_map(|ty| visit(ctx, db, at, ty, seen)),
                TyKind::Exist { inner, .. } => visit(ctx, db, at, inner, seen),
                TyKind::FnPtr(sig) => sig
                    .params
                    .iter()
                    .chain(std::iter::once(&sig.ret))
                    .find_map(|ty| visit(ctx, db, at, ty, seen)),
                TyKind::Bidir { lhs, rhs } => {
                    visit(ctx, db, at, lhs, seen).or_else(|| visit(ctx, db, at, rhs, seen))
                }
                _ => None,
            }
        }
        let mut seen = HashSet::new();
        types.into_iter().find_map(|ty| visit(self, db, at, ty, &mut seen))
    }

    fn invalid_expression(&mut self, expr: hir_def::ExprId, message: impl Into<String>) {
        self.push_inference_diagnostic(InferenceDiagnostic::InvalidTypeUse {
            node: hir_def::ExprOrPatId::ExprId(expr),
            message: message.into(),
        });
    }

    /// The name closest to `name` that this environment knows, by
    /// upstream's rule.
    ///
    /// `find_closest` (`type_error.ml:244-255`): only names whose length is
    /// within 2, optimal-string-alignment distance at most
    /// `min(4, max(1, len - 3))`, nearest wins. Copied rather than invented
    /// — a suggestion that differs from upstream's is one the reader has to
    /// double-check.
    fn closest_function_name(&self, name: &str) -> Option<String> {
        let max_distance = 4.min(1.max(name.len().saturating_sub(3)));
        let mut best: Option<(usize, String)> = None;
        let all_names = self
            .db
            .zip(self.owner)
            .map(|(db, owner)| super::dependencies::function_names(db, owner.file(db)));
        let candidates = self
            .env
            .functions
            .keys()
            .chain(self.env.overloads.keys())
            .chain(self.env.cross_file_function_names.iter())
            .map(String::as_str)
            .chain(all_names.into_iter().flatten().map(|n| n.as_str()));
        for other in candidates {
            if name.len().abs_diff(other.len()) > 2 || other == name {
                continue;
            }
            // Not an operator. `Z` and `/` are one edit apart and within
            // the length window, so the rule alone offers "Did you mean
            // /?" — a suggestion no reader can use. Upstream's
            // `Err_no_function_type` is handed the *function* bindings;
            // an operator lives in the overload table.
            if !other.starts_with(|c: char| c.is_alphabetic() || c == '_') {
                continue;
            }
            let d = osa_distance(name, other);
            if d <= max_distance && best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                best = Some((d, other.to_owned()));
            }
        }
        best.map(|(_, n)| n)
    }

    /// Whether `name` denotes something callable anywhere the workspace can
    /// see. `name_known_in_workspace` also counts values, which is right for
    /// "is this name bound at all" and wrong for "may it be called".
    fn name_is_callable_anywhere(&self, name: &str) -> bool {
        self.env.constructors.contains_key(name)
            || self.env.cross_file_constructor_names.contains(name)
            || self.env.cross_file_function_names.contains(name)
            || self.env.mappings.contains_key(name)
            || self.env.overloads.contains_key(name)
            || self.env.functions.contains_key(name)
            // A name the symbol index knows may be a function in a file this
            // handle has not typed; only a *value* binding here rules it out,
            // and the caller checks that.
            || is_likely_external_or_generated(name)
    }

    /// Whether `name` denotes anything the workspace knows about.
    ///
    /// Covers constructors and mappings as well as functions, plus the
    /// accessor names Sail derives from a mapping.
    fn name_known_in_workspace(&self, name: &str) -> bool {
        if self.env.constructors.contains_key(name)
            || self.env.cross_file_constructor_names.contains(name)
            || self.env.cross_file_function_names.contains(name)
            || self.env.mappings.contains_key(name)
            // An overload group is a binding too, and its members may be
            // generated in another file — `overload update_f = {..}` comes
            // from whichever file declares the bitfield.
            || self.env.overloads.contains_key(name)
            || self.env.symbol_index.get(name).is_some()
        {
            return true;
        }
        // `m_forwards`, `m_backwards`, `m_forwards_matches`, ... are generated
        // for every mapping `m`.
        for suffix in
            ["_forwards", "_backwards", "_forwards_matches", "_backwards_matches", "_matches"]
        {
            if let Some(base) = name.strip_suffix(suffix) {
                if self.env.mappings.contains_key(base) || self.env.symbol_index.get(base).is_some()
                {
                    return true;
                }
            }
        }
        false
    }
}

/// Type-variable names appearing anywhere in a scheme's parameters or return
/// type.
fn scheme_free_params(scheme: &TypeScheme) -> Vec<String> {
    fn walk(ty: &Ty, out: &mut Vec<String>) {
        match ty.kind() {
            TyKind::Param(name) => {
                if !out.contains(name) {
                    out.push(name.clone());
                }
            }
            TyKind::App { args, .. } | TyKind::Named(_, args) => {
                for a in args {
                    if let TyArg::Type(t) = a {
                        walk(t, out);
                    }
                }
            }
            TyKind::Tuple(items) => items.iter().for_each(|t| walk(t, out)),
            TyKind::FnPtr(sig) => {
                sig.params.iter().for_each(|t| walk(t, out));
                walk(&sig.ret, out);
            }
            TyKind::Exist { inner, .. } => walk(inner, out),
            TyKind::Bidir { lhs, rhs } => {
                walk(lhs, out);
                walk(rhs, out);
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    scheme.params.iter().for_each(|p| walk(p, &mut out));
    walk(&scheme.ret, &mut out);
    out
}

/// Every type variable `ty` mentions, in type *and* value position.
/// `bits('n)` carries `'n` as a `TyArg::Value`, not a `TyKind::Param`, so a
/// walk over type arguments alone would miss it.
pub(super) fn collect_type_vars(ty: &Ty, out: &mut Vec<String>) {
    let push = |name: &str, out: &mut Vec<String>| {
        if !out.iter().any(|s| s == name) {
            out.push(name.to_string());
        }
    };
    match ty.kind() {
        TyKind::Param(name) => {
            for var in
                name.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '\'' && c != '#')
            {
                if var.starts_with('\'') {
                    push(var, out);
                }
            }
        }
        TyKind::App { args, .. } | TyKind::Named(_, args) => {
            for a in args {
                match a {
                    TyArg::Type(t) => collect_type_vars(t, out),
                    other => {
                        let Some(text) = other.as_value_str() else { continue };
                        // `8 * 'n` mentions `'n`; scan for the variables
                        // rather than matching the expression as a whole.
                        for var in
                            text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '\'')
                        {
                            if var.starts_with('\'') {
                                push(var, out);
                            }
                        }
                    }
                }
            }
        }
        TyKind::Tuple(items) => items.iter().for_each(|t| collect_type_vars(t, out)),
        TyKind::FnPtr(sig) => {
            sig.params.iter().for_each(|t| collect_type_vars(t, out));
            collect_type_vars(&sig.ret, out);
        }
        TyKind::Exist { inner, .. } => collect_type_vars(inner, out),
        TyKind::Bidir { lhs, rhs } => {
            collect_type_vars(lhs, out);
            collect_type_vars(rhs, out);
        }
        _ => {}
    }
}

/// The variables bound by an existential anywhere inside `ty`.
pub(super) fn collect_existential_vars(ty: &Ty, out: &mut Vec<String>) {
    match ty.kind() {
        TyKind::Exist { vars, inner, .. } => {
            for v in vars {
                if !out.contains(v) {
                    out.push(v.clone());
                }
            }
            collect_existential_vars(inner, out);
        }
        TyKind::App { args, .. } | TyKind::Named(_, args) => {
            for a in args {
                if let TyArg::Type(t) = a {
                    collect_existential_vars(t, out);
                }
            }
        }
        TyKind::Tuple(items) => items.iter().for_each(|t| collect_existential_vars(t, out)),
        TyKind::FnPtr(sig) => {
            sig.params.iter().for_each(|t| collect_existential_vars(t, out));
            collect_existential_vars(&sig.ret, out);
        }
        TyKind::Bidir { lhs, rhs } => {
            collect_existential_vars(lhs, out);
            collect_existential_vars(rhs, out);
        }
        _ => {}
    }
}

/// Pre-filter for overload candidates (never rejects a valid match).
/// The outermost type constructor's name, or `?` when it is not yet known.
fn type_leaf_name(ty: &Ty) -> String {
    match ty.kind() {
        TyKind::Scalar(s) => s.name().to_string(),
        TyKind::Named(name, _) => name.clone(),
        TyKind::App { name, .. } => name.clone(),
        _ => "?".to_string(),
    }
}

/// Whether two type-constructor names denote the same Sail type constructor,
/// modulo the spellings that are interchangeable in a signature.
///
/// `?` (an inference variable, a type parameter) matches anything. A name
/// this checker cannot expand — a cross-file alias such as `xlenbits` — does
/// *not*: see [`types_plausibly_compatible`] for the looser test.
fn leaf_names_compatible(p: &str, a: &str) -> bool {
    if p == "?" || a == "?" || p == a {
        return true;
    }
    let is_numeric = |s: &str| matches!(s, "int" | "nat" | "atom" | "range" | "implicit");
    if is_numeric(p) && is_numeric(a) {
        return true;
    }
    let pair = |x: &str, y: &str| (p == x && a == y) || (p == y && a == x);
    pair("bit", "bits")
        || pair("bits", "bitvector")
        || pair("bit", "bitvector")
        || pair("atom_bool", "bool")
        || pair("string", "string_literal")
}

fn types_plausibly_compatible(param: &Ty, arg: &Ty) -> bool {
    let p = type_leaf_name(param);
    let a = type_leaf_name(arg);
    let p = p.as_str();
    let a = a.as_str();
    if leaf_names_compatible(p, a) {
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
    /// Freshened names that do not need an explicit argument binding.
    /// Return-only universals wait for the surrounding context and are checked
    /// at body completion. Existential binders are introduced at the call and
    /// do not occur free in that result.
    ret_only: Vec<String>,
}

struct PendingCallResult {
    callee: hir_def::ExprId,
    name: String,
    ret: Ty,
    freshening: FreshenInfo,
    signature: String,
}

struct PendingImplicit {
    callee: hir_def::ExprId,
    ty: Ty,
    witnesses: Vec<Ty>,
    assumptions: Vec<ConstraintExpr>,
}

/// Inference context for a single callable body.
pub(crate) struct InferenceContext<'db> {
    pub(crate) db: Option<&'db dyn salsa::Database>,
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
    /// Inferring the left-hand side of an assignment. Upstream has a
    /// separate sentence for a field there (`LE_field`,
    /// `type_check.ml:3611`) and the field arm cannot see which side of
    /// the `=` it is on.
    pub(super) in_assign_target: bool,
    /// Inside one side of a `mapping clause`. The two sides are checked in
    /// separate scopes on purpose — `AMO(op, aq, ..)` binds `aq : bool` from
    /// the constructor on one side and from a bit slice on the other — so a
    /// constructor argument that disagrees there is the design, not a defect.
    /// The largest value each of the enclosing callable's quantifiers can
    /// take, where its own signature says so — `forall 'n, 'n in {2, 3}`
    /// bounds `'n` at 3. Without it a `bits('n)` has no width at all here,
    /// and an index that cannot possibly fit one looked undecidable.
    pub(crate) quant_max: HashMap<String, i128>,
    /// Inside a `$[complete]` / `$[incomplete]` match.
    suppress_incomplete_match: bool,
    /// The existential payload of the constructor being checked: its own
    /// constraint, and the placeholder standing for each of its variables.
    pending_payload: Option<(ConstraintExpr, Vec<(String, String)>)>,
    pending_call_results: Vec<PendingCallResult>,
    pending_implicits: Vec<PendingImplicit>,
    pub(crate) in_mapping_arm: bool,
    pub(crate) diverges: Diverges,
    pub(crate) is_pure_context: bool,
    pub(crate) observed_effects: std::collections::BTreeSet<hir_def::EffectTag>,
    pub(crate) workspace_effects: Option<std::sync::Arc<hir_def::effects::WorkspaceEffects>>,
    diagnostics: Vec<Diagnostic>,
    diagnostic_byte_spans: Vec<(usize, usize)>,
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
    pub(crate) fn set_owner(
        &mut self,
        db: &'db dyn salsa::Database,
        owner: hir_def::def_query::DefWithBodyId<'db>,
    ) {
        self.db = Some(db);
        self.owner = Some(owner);
        if hir_def::source_order::program(db, owner.file(db)).is_some() {
            if let Some(at) = hir_def::source_order::for_body(db, owner) {
                // The projected type environment excludes inaccessible names,
                // but their existence still prevents implicit declarations.
                self.expr_scopes =
                    hir_def::source_order::bindings::body_scopes_at(db, at, &self.body);
            }
        }
    }

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
        if let Some(span) = source_map.expr_syntax(body.root()) {
            table.reserve_source_names(source.get(span.start..span.end).unwrap_or(""));
        }
        table.set_nominal_types(
            // Record identity is enforced here. Union and enum identity also
            // participates in constructor/pattern and overload selection;
            // bitfields retain their underlying-bitvector conversions.
            env.records.keys().filter(|n| !env.type_aliases.contains_key(*n)).cloned().collect(),
        );
        let resolver = Arc::new(hir_def::DefMap::default());
        // Build ExprScopes at construction for static binding lookup.
        let expr_scopes = hir_def::expr_store::scope::ExprScopes::new_with_globals(&body, |name| {
            env.value_binding_exists(name.as_str())
        });
        Self {
            in_assign_target: false,
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
            quant_max: HashMap::new(),
            suppress_incomplete_match: false,
            pending_payload: None,
            pending_call_results: Vec::new(),
            pending_implicits: Vec::new(),
            in_mapping_arm: false,
            diverges: Diverges::Maybe,
            is_pure_context: false,
            observed_effects: std::collections::BTreeSet::new(),
            workspace_effects: None,
            diagnostics: Vec::new(),
            diagnostic_byte_spans: Vec::new(),
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
    #[expect(dead_code)]
    pub(crate) fn store(&self) -> &hir_def::expr_store::ExpressionStore {
        &self.body.store
    }

    /// Derive a borrowing Resolver on demand from the owned DefMap.
    pub(crate) fn make_resolver(&self) -> hir_def::Resolver<'_> {
        hir_def::Resolver::for_file(&self.resolver)
    }

    #[expect(dead_code)] // will be wired into inference query
    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    /// Look up source span for an ExprId via the source_map.
    /// Returns None when no source_map is available.
    fn expr_span(&self, _body: &hir_def::Body, id: hir_def::ExprId) -> Option<Span> {
        self.source_map.as_ref()?.expr_syntax(id)
    }

    /// Finish and return result.
    pub fn finish_query(mut self) -> TypeCheckResult {
        self.solve_pending_obligations();
        self.check_pending_call_results();
        self.check_pending_implicits();
        // Resolve remaining inference variables to defaults.
        self.table.fallback_unsolved();
        self.resolve_recorded_mismatches();
        // Unsolved constraints are often runtime assertions; callers
        // should treat as warning for pass-file validation.
        if !self.result.unsolved_constraints.is_empty() {
            let body_root = self.body.root();
            let diags: Vec<_> = self
                .result
                .unsolved_constraints
                .iter()
                // An unsupported type relation is retained for analysis, but
                // provides no actionable numeric constraint to show the user.
                .filter(|ob| ob.cause == ObligationCause::Explicit)
                // A primitive type in a numeric slot is an internal recovery
                // artifact, not an actionable predicate for a source warning.
                .filter(|ob| {
                    ob.unknown_reason != Some(super::UnknownConstraintReason::ErasedNumeric)
                })
                .filter(|ob| match &ob.goal {
                    Obligation::NumericConstraint(c) => has_reportable_constraint(c),
                    Obligation::WidthEquality(..) => true,
                    Obligation::All(_) | Obligation::Assuming { .. } => {
                        unreachable!("queued goals are flattened")
                    }
                })
                .map(|ob| InferenceDiagnostic::UnsolvedConstraint {
                    expr: ob.origin.unwrap_or(body_root),
                    constraint: format!("{:?}", ob.goal),
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

        let body_diags = crate::diagnostics::BodyValidationDiagnostic::collect(
            &self.body,
            &self.expr_scopes,
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

    /// Solve each goal under the evidence captured where it was generated.
    /// Live inference bindings are resolved before fallback erases them.
    fn solve_pending_obligations(&mut self) {
        let _solver_budget =
            crate::solver_budget::SolverBudget::enter(self.cancel.clone(), Default::default());
        for mut ob in self.table.drain_obligations() {
            if let Some(db) = self.db {
                db.unwind_if_revision_cancelled();
            }
            let mut has_flexible_witness = false;
            let mut substitution =
                Subst { types: Default::default(), values: ob.substitution.values.clone() };
            for (name, ty) in &ob.substitution.types.entries {
                let resolved = self.table.deep_resolve(ty);
                if matches!(resolved.kind(), TyKind::Infer(_)) {
                    has_flexible_witness = true;
                }
                if !matches!(resolved.kind(), TyKind::Infer(_)) {
                    substitution.types.insert(name.clone(), resolved.clone());
                    substitution.values.insert(name.clone(), resolved.display_text());
                }
            }
            ob.substitution = substitution;
            let goal =
                constraint::apply_subst_constraint_expr(&ob.goal.constraint(), &ob.substitution);
            let goal = constraint::numeric_alias_unfoldings(&goal, &self.env.alias_schemes)
                .pop()
                .unwrap_or(goal);
            let (goal, flexible) = self.table.resolve_deferred_constraint(&goal);
            has_flexible_witness |= flexible;
            ob.assumptions = ob
                .assumptions
                .iter()
                .map(|assumption| {
                    let (resolved, flexible) = self.table.resolve_deferred_constraint(assumption);
                    has_flexible_witness |= flexible;
                    resolved
                })
                .collect();
            let incomplete_before = crate::solver_budget::SolverBudget::incomplete_count();
            let budget_before = crate::solver_budget::SolverBudget::budget_exhaustion_count();
            let erased_inputs =
                super::solver::has_erased_inputs(&goal, &ob.substitution, &ob.assumptions);
            let status = if erased_inputs {
                ConstraintStatus::Unknown
            } else {
                match constraint::try_normalize_constraint(&goal, &ob.substitution, &ob.assumptions)
                {
                    constraint::NormalizationResult::Decided(status) => status,
                    constraint::NormalizationResult::Deferred => {
                        super::solver::solve(&goal, &ob.substitution, &ob.assumptions)
                    }
                }
            };
            let status = if status == ConstraintStatus::Unknown
                && !has_flexible_witness
                && !ob.rigid_variables.is_empty()
            {
                super::solver::prove_quantified(
                    &goal,
                    &ob.substitution,
                    &ob.assumptions,
                    &ob.rigid_variables,
                )
            } else {
                status
            };
            match status {
                ConstraintStatus::Satisfied => self.result.obligation_stats.proved += 1,
                ConstraintStatus::Failed => {
                    self.result.obligation_stats.refuted += 1;
                    self.push_inference_diagnostic(InferenceDiagnostic::ConstraintViolation {
                        expr: ob.origin.unwrap_or(self.body.root()),
                        constraint: self.table.display_constraint(&goal),
                        derived_from: Vec::new(),
                    });
                    ob.goal = Obligation::NumericConstraint(goal);
                    self.result.refuted_constraints.push(ob);
                }
                ConstraintStatus::Unknown => {
                    self.result.obligation_stats.unknown += 1;
                    ob.unknown_reason = Some(if erased_inputs {
                        super::UnknownConstraintReason::ErasedNumeric
                    } else {
                        super::UnknownConstraintReason::classify(
                            &goal,
                            ob.cause,
                            has_flexible_witness,
                            crate::solver_budget::SolverBudget::budget_exhaustion_count()
                                > budget_before,
                            crate::solver_budget::SolverBudget::incomplete_count()
                                > incomplete_before,
                        )
                    });
                    // Report the instantiated, alias-expanded goal actually checked.
                    ob.goal = Obligation::NumericConstraint(goal);
                    self.result.unsolved_constraints.push(ob);
                }
            }
        }
    }

    /// Result-only quantifiers get all enclosing contexts before we report
    /// them as unresolved. Checking during candidate selection rejects valid
    /// `consume(zeros())`; forgetting them entirely hides unconstrained calls.
    fn check_pending_call_results(&mut self) {
        fn contains(ty: &Ty, variable: &Ty) -> bool {
            if ty == variable {
                return true;
            }
            match ty.kind() {
                TyKind::Named(_, args) | TyKind::App { args, .. } => {
                    args.iter().any(|arg| matches!(arg, TyArg::Type(ty) if contains(ty, variable)))
                }
                TyKind::Tuple(items) => items.iter().any(|ty| contains(ty, variable)),
                TyKind::FnPtr(sig) => {
                    contains(&sig.ret, variable)
                        || sig.params.iter().any(|ty| contains(ty, variable))
                }
                TyKind::Exist { inner, .. } => contains(inner, variable),
                TyKind::Bidir { lhs, rhs } => contains(lhs, variable) || contains(rhs, variable),
                _ => false,
            }
        }
        for pending in std::mem::take(&mut self.pending_call_results) {
            let ret = self.table.deep_resolve(&pending.ret);
            let mut names = HashSet::new();
            super::substitution::ty_names(&ret, &mut names, false);
            let quants: Vec<_> = pending
                .freshening
                .entries
                .iter()
                .filter_map(|entry| {
                    let variable = self.table.resolve(&entry.infer_ty);
                    (matches!(variable.kind(), TyKind::Infer(_))
                        && (names.contains(entry.fresh_name.trim_start_matches('\''))
                            || contains(&ret, &variable)))
                    .then(|| entry.orig_name.clone())
                })
                .collect();
            if !quants.is_empty() {
                self.push_inference_diagnostic(InferenceDiagnostic::UnresolvedCallQuantifiers {
                    call_expr: pending.callee,
                    id: pending.name,
                    quants,
                    signature: Some(pending.signature),
                });
            }
        }
    }

    fn scope_obligations(&mut self, start: usize, origin: hir_def::ExprId, locals: &LocalEnv) {
        if self.table.has_unscoped_obligations_since(start) {
            let mut assumptions = Vec::new();
            self.fill_assumptions(locals, &mut assumptions);
            self.table.scope_obligations_since(start, origin, assumptions);
        }
    }

    // Record only the selected signature. Delay resolution until surrounding
    // expected types have bound its indices, but preserve the call's locals.
    fn defer_implicit_arguments(
        &mut self,
        callee: hir_def::ExprId,
        scheme: &TypeScheme,
        supplied: usize,
        locals: &LocalEnv,
    ) {
        if supplied == scheme.params.len() || !scheme.implicit_params.iter().any(|p| *p) {
            return;
        }
        let mut assumptions = Vec::new();
        self.fill_assumptions(locals, &mut assumptions);
        let mut bindings: Vec<_> =
            locals.bindings.iter().filter(|(name, _)| !name.starts_with('$')).collect();
        // Proof budgets must not depend on HashMap iteration order.
        bindings.sort_unstable_by(|(a, _), (b, _)| a.cmp(b));
        let witnesses: Vec<_> = bindings.into_iter().map(|(_, ty)| ty.clone()).collect();
        for (ty, implicit) in scheme.params.iter().zip(&scheme.implicit_params) {
            if *implicit {
                self.pending_implicits.push(PendingImplicit {
                    callee,
                    ty: ty.clone(),
                    witnesses: witnesses.clone(),
                    assumptions: assumptions.clone(),
                });
            }
        }
    }

    fn check_pending_implicits(&mut self) {
        for pending in std::mem::take(&mut self.pending_implicits) {
            let ty = self.table.deep_resolve(&pending.ty);
            let Some(value) = super::refinement::singleton(&ty) else { continue };
            let (resolved, flexible) =
                self.table.resolve_deferred_constraint(&ConstraintExpr::Compare {
                    lhs: value,
                    op: CompareOp::Eq,
                    rhs: NumericExpr::Const(0),
                });
            if flexible {
                continue;
            }
            let ConstraintExpr::Compare { lhs: value, .. } = resolved else { continue };
            let assumptions: Vec<_> = pending
                .assumptions
                .iter()
                .map(|c| self.table.resolve_deferred_constraint(c).0)
                .collect();
            let witnesses: Vec<_> = pending
                .witnesses
                .iter()
                .map(|ty| {
                    let ty = self.table.deep_resolve(ty);
                    self.table.normalize_alias_ty(&ty)
                })
                .collect();
            if super::runtime_witness::recoverable(&value, &witnesses, &assumptions) == Some(false)
            {
                self.push_inference_diagnostic(InferenceDiagnostic::UnrecoverableImplicit {
                    expr: pending.callee,
                    message: format!("Cannot re-write sizeof({}) for an implicit argument: no runtime witness or unique constant is available", value.to_string_repr()),
                });
            }
        }
    }

    /// Put every recorded mismatch through the table before the result
    /// leaves.
    ///
    /// A mismatch is *recorded* while inference runs and *read* much later,
    /// by a renderer with no table of its own — so a variable still
    /// unresolved at the moment of recording is one printed to the user.
    /// `Type mismatch between bits(?34) and int` names this checker's own
    /// counter, which means nothing to anyone reading it, and a variable
    /// bound after the mismatch was recorded resolves fine here.
    fn resolve_recorded_mismatches(&mut self) {
        let ids: Vec<_> = self.result.type_mismatches.keys().copied().collect();
        for id in ids {
            let Some(m) = self.result.type_mismatches.get(&id).cloned() else { continue };
            let resolved = crate::infer::TypeMismatch {
                expected: self.table.deep_resolve(&m.expected),
                actual: self.table.deep_resolve(&m.actual),
            };
            self.result.type_mismatches.insert(id, resolved);
        }
    }

    fn finish(self) -> TypeCheckResult {
        let mut result = self.result;
        result.legacy_diagnostics = self.diagnostics;
        result.legacy_diagnostic_byte_spans = self.diagnostic_byte_spans;
        result.bodies = None;
        result
    }

    #[expect(dead_code)] // will be wired into inference query
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

    #[expect(dead_code)] // will be wired into inference query
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
            // expect_int_type which is #[expect(dead_code)]).
            // Type mismatches should use record_type_mismatch(ExprId)
            // at call sites with ExprId context.
            false
        }
    }

    #[expect(dead_code)] // will be wired into inference query
    fn expect_int_type(&mut self, span: Span, actual: &Ty) -> bool {
        self.expect_type(span, actual, &Ty::named("int".to_string()))
    }

    /// Freshen a TypeScheme's quantifiers with fresh InferTys to avoid
    /// name collisions. Returns the freshened scheme plus a `FreshenInfo`
    /// for mapping freshened names back to originals.
    fn freshen_scheme(&mut self, scheme: &TypeScheme) -> (TypeScheme, FreshenInfo) {
        use super::constraint::{apply_subst, apply_subst_constraint_expr};
        // Every type variable in a signature is universally quantified, with
        // or without an explicit `forall` — `val f : bits('n) -> int` is
        // legal Sail. Freshening only the declared quantifiers left the rest
        // as `Param`s that then unified against a caller's fresh variable and
        // reported `expected ?0, found bits('n)`.
        let mut quantifiers = scheme.quantifiers.clone();
        for name in scheme_free_params(scheme) {
            if !quantifiers.contains(&name) {
                quantifiers.push(name);
            }
        }
        if quantifiers.is_empty() {
            return (scheme.clone(), FreshenInfo::default());
        }
        let mut subst = Subst::default();
        let mut info = FreshenInfo::default();
        for q in &quantifiers {
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
        // `implicit('n)` is filled in by the checker from the expected type,
        // not by the caller, so a variable that occurs only there is not the
        // call's to resolve: `let x : bits(5) = zeros() @ 0b1` never passes
        // `'n` at all.
        let mut in_params = Vec::new();
        scheme
            .params
            .iter()
            .filter(|p| !super::ty_is_implicit(p))
            .for_each(|p| collect_type_vars(p, &mut in_params));
        // A variable bound by an existential — `val foo : unit -> {'n, 0 <
        // 'n <= 32. int('n)}` — is introduced fresh at each call site rather
        // than solved for, so it is never "unresolved". `scheme_free_params`
        // cannot tell it apart from a universal and adds it to the
        // quantifier list, which is why the exclusion is applied here.
        let mut existential = Vec::new();
        collect_existential_vars(&scheme.ret, &mut existential);
        info.ret_only = info
            .entries
            .iter()
            .filter(|e| existential.contains(&e.orig_name) || !in_params.contains(&e.orig_name))
            .map(|e| e.fresh_name.clone())
            .collect();
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
    /// Bind a signature's width from an argument whose type is a configured
    /// alias.
    ///
    /// `nan_unbox(F(i))` passes a `flenbits`, and `flenbits` is `bits(flen)`.
    /// `alias_scheme` refuses to look through that alias because `flen` comes
    /// from a configuration — rightly, where the question is whether
    /// something is *proved*. Here the question is only what the signature's
    /// `'n` stands for, and the answer is a symbol either way: `'n := flen`,
    /// never `'n := 64`. Every constraint over it is still proved
    /// symbolically or not at all, and `assert(flen >= 16)` is how the
    /// function says which.
    ///
    /// Refusing left `'n` unbound, the candidate dropped for an unresolved
    /// quantifier, and `rF_H` with no type at all.
    fn bind_width_through_shape_alias(&mut self, param: &Ty, actual: &Ty) {
        let param = self.table.resolve(param);
        let TyKind::App { name, args, .. } = param.kind() else { return };
        // One step at a time, and stopped as soon as the head agrees with
        // the parameter's: `flenbits` is `bits(flen)` after one step and
        // loses the width entirely by the last one.
        let mut current = actual.clone();
        for _ in 0..4 {
            let Some(next) = self.table.expand_alias_once_for_shape(&current) else { return };
            current = next;
            let TyKind::App { name: act_name, args: act_args, .. } = current.kind() else {
                continue;
            };
            if act_name != name || act_args.len() != args.len() {
                continue;
            }
            for (want, have) in args.iter().zip(act_args.iter()) {
                let TyArg::Type(want) = want else { continue };
                let want = self.table.resolve(want);
                let TyKind::Infer(var) = want.kind() else { continue };
                let var = var.0;
                match have {
                    TyArg::Type(have) => {
                        let have = have.clone();
                        self.table.bind(var, have);
                    }
                    TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => {
                        if let Some(text) = have.as_value_str() {
                            self.table.bind(var, Ty::param(text));
                        }
                    }
                }
            }
            return;
        }
    }

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
                        (TyArg::Type(et), other) if matches!(et.kind(), TyKind::Param(_)) => {
                            let TyKind::Param(name) = et.kind() else { unreachable!() };
                            if let Some(value) = other.as_value_str() {
                                let _ = constraint::unify_value(name, &value, subst);
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

    /// The facts a proof may use here: the file's own, plus what the
    /// control flow leading to this expression established.
    ///
    /// Both are expanded through the numeric type aliases on the way out,
    /// because the goal they will be matched against is expanded too — an
    /// `assert(flen >= 16)` records its fact under the alias's name and the
    /// signature's `'n` is bound to the alias's body. See
    /// `constraint::expand_numeric_aliases`.
    fn fill_assumptions(&self, locals: &LocalEnv, out: &mut Vec<ConstraintExpr>) {
        out.clear();
        out.extend(self.env.global_constraints.iter().cloned());
        out.extend(locals.constraints.iter().cloned());
        let unfolded: Vec<_> = out
            .iter()
            .flat_map(|a| constraint::numeric_alias_unfoldings(a, &self.env.alias_schemes))
            .collect();
        out.extend(unfolded);
    }

    /// Why an identifier came out untyped, for `SAIL_UNTYPED_IDENT=1`.
    ///
    /// Every branch here resolves the name to *something* — a top-level
    /// symbol, a scope-tree binding, a pattern constant — and then types it
    /// as `error()` because nothing carries a type back. Which of those it
    /// was is the whole question, and reading it off the source is not
    /// possible: `i` and `pbmt` and `addr` all look alike from outside.
    ///
    /// `NEVERBOUND` means the name is bound in no scope anywhere in the
    /// body — the strongest signal, because it says the *binder* was never
    /// lowered, not that the use looked in the wrong place. It is what
    /// found the mapping clause: `forwards Regidx(r) => zero_extend(r)`
    /// reached here as a flat list of expressions with `forwards` and `=>`
    /// as loose tokens, so nothing in it was a pattern and nothing bound
    /// `r`. 135 of the 241 went away when the grammar grew a node for it.
    fn report_untyped_ident(&self, id: hir_def::ExprId, name: &str) {
        if std::env::var("SAIL_UNTYPED_IDENT").is_err() {
            return;
        }
        let via_scope = self
            .expr_scopes
            .scope_for(id)
            .and_then(|s| {
                self.expr_scopes.resolve_name_in_scope(s, &hir_def::name::Name::new(name))
            })
            .map(|hit| hit.pat());
        // Scope first: a parameter named like a top-level symbol was
        // reported as the top-level symbol, which hid where it came from.
        let why = if let Some(pat) = via_scope {
            if self.result.pat_ty(pat).is_some() {
                "scope tree, pattern typed"
            } else {
                "scope tree, pattern UNTYPED"
            }
        } else if self.env.top_level_symbol_exists(name) {
            "top-level symbol"
        } else if self.pattern_constants.contains(name) {
            "pattern constant"
        } else {
            let n = hir_def::name::Name::new(name);
            let anywhere = self
                .expr_scopes
                .all_scopes()
                .any(|(sid, _)| self.expr_scopes.resolve_name_in_scope(sid, &n).is_some());
            if anywhere {
                "bound in another branch"
            } else {
                "NEVERBOUND"
            }
        };
        let line = self
            .expr_span(&self.body, id)
            .map(|sp| self.source[..sp.start.min(self.source.len())].matches('\n').count() + 1)
            .unwrap_or(0);
        let text = self
            .expr_span(&self.body, id)
            .map(|sp| {
                let start =
                    self.source[..sp.start.min(self.source.len())].rfind('\n').map_or(0, |p| p + 1);
                let end = self.source[start..].find('\n').map_or(self.source.len(), |p| start + p);
                self.source[start..end].trim().to_owned()
            })
            .unwrap_or_default();
        eprintln!("UNTYPED_IDENT\t{name}\t{why}\t{line}\t{text}");
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
        deferred: &mut Vec<ConstraintExpr>,
    ) -> Option<TypeError> {
        // Pre-compute bound status for each quantifier. A quantifier is
        // considered bound if either the Subst has a value for it OR its
        // corresponding InferTy was resolved by the table.
        let bound_set: std::collections::HashSet<&str> = quantifiers
            .iter()
            .filter(|name| {
                if freshen_info.ret_only.iter().any(|r| r == name.as_str()) {
                    return true;
                }
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

        // Values for the quantifiers that did get bound, so a constraint
        // mentioning one of them can still be read.
        let mut known: std::collections::HashMap<String, i64> = Default::default();
        for entry in &freshen_info.entries {
            let resolved = self.table.resolve(&entry.infer_ty);
            if let TyKind::Param(text) = resolved.kind() {
                if let Ok(v) = text.trim().parse::<i64>() {
                    known.insert(entry.fresh_name.clone(), v);
                }
            }
        }
        for (name, value) in &subst.values.entries {
            if let Ok(v) = value.trim().parse::<i64>() {
                known.insert(name.clone(), v);
            }
        }

        // What the unifier knows, said in the language the solver reads.
        //
        // A quantifier can be pinned in two places: the `Subst` the overload
        // bridge fills in, and the inference table, when the argument's type
        // unified with the parameter's. The solver only ever looked at the
        // first, so `'n := flen` — learnt by unifying `bits('n)` against
        // `flenbits` — left the goal `'n >= 'm` with a free variable and no
        // assumption could reach it.
        let mut subst = subst.clone();
        for entry in &freshen_info.entries {
            if subst.values.contains_key(&entry.fresh_name) {
                continue;
            }
            let resolved = self.table.resolve(&entry.infer_ty);
            // `Named` and not `Param`, because a width written as a name —
            // `bits(flen)` — reaches the table as the type it parses to.
            let text = match resolved.kind() {
                TyKind::Param(text) => text,
                TyKind::Named(name, args) if args.is_empty() => name,
                _ => continue,
            };
            subst.values.insert(entry.fresh_name.clone(), text.clone());
        }
        let subst = &subst;

        for constraint in constraints {
            // A synonym is expanded first — `is_mem_width('n)` becomes the
            // set membership it stands for. One that is *not* a synonym
            // stays `NC_app`, which nothing here can represent, and a
            // constraint we cannot represent is not evidence against a
            // candidate: before an application-shaped clause was recorded at
            // all, this was the behaviour by omission, and recording it and
            // then refusing the call is strictly worse than not knowing.
            let goal = constraint::expand_constraint_synonyms(
                &constraint.expr,
                &self.env.constraint_synonyms,
                0,
            );
            let goal = constraint::apply_subst_constraint_expr(&goal, subst);
            let goal = constraint::numeric_alias_unfoldings(&goal, &self.env.alias_schemes)
                .pop()
                .unwrap_or(goal);
            let (goal, has_flexible_witness) = self.table.resolve_deferred_constraint(&goal);
            if matches!(goal, ConstraintExpr::App { .. }) {
                deferred.push(goal);
                continue;
            }
            // Solve first, and let the gate below decide only what the
            // solver could not. An assumption may settle a constraint whose
            // variables are unbound — `'width in {1, 2, 4, 8}` proves
            // `'width > 0` without knowing which of the four it is — and
            // gating on boundness first threw those away unread.
            let mut status = super::solver::solve(&goal, subst, assumptions);
            if status == ConstraintStatus::Unknown && !has_flexible_witness {
                let mut names = HashSet::new();
                super::substitution::constraint_names(&goal, &mut names);
                let mut rigid: Vec<_> = names
                    .into_iter()
                    .map(|n| format!("'{n}"))
                    .filter(|n| self.table.is_rigid(n))
                    .collect();
                rigid.sort();
                if !rigid.is_empty() {
                    status = super::solver::prove_quantified(&goal, subst, assumptions, &rigid);
                }
            }
            match status {
                ConstraintStatus::Satisfied => continue,
                ConstraintStatus::Failed => {
                    return Some(TypeError::FailedConstraint {
                        constraint: constraint.text.clone(),
                        derived_from: derived_from.to_vec(),
                        proved_false: true,
                    });
                }
                // An unproved predicate remains an obligation of the chosen
                // call. It does not erase its return type: contextual widths
                // can be supplied later by the enclosing call or assignment.
                // Only a refutation above eliminates the candidate.
                ConstraintStatus::Unknown => {
                    deferred.push(goal.clone());
                }
            }
            {
                // An equation is still worth reading when everything but one
                // quantifier is known. `'m == 8 * 'n` with `'m` fixed at 65
                // by the return type has no integer `'n`, and calling that
                // "unresolved" says the opposite of what it is.
                match solve_equation_constraint(&goal, &known) {
                    Some(false) => {
                        return Some(TypeError::FailedConstraint {
                            constraint: constraint.text.clone(),
                            derived_from: derived_from.to_vec(),
                            proved_false: true,
                        });
                    }
                    Some(true) => continue,
                    None => {}
                }
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
        let ty = self.table.normalize_alias_deep(ty);
        match ty.kind() {
            TyKind::Named(name, _) => self
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

    /// Numeric/boolean evidence uses the same declarations as expression
    /// inference, including immutable singleton values in another file.
    fn refinement_value(&self, locals: &LocalEnv, name: &str) -> Option<Ty> {
        self.env.lookup_value(locals, name).or_else(|| {
            let db = self.db?;
            let file = self.env.symbol_index.get_file(name)?;
            super::dependencies::file_symbol(db, file, name.to_owned())
                .data
                .values
                .get(name)
                .cloned()
        })
    }

    /// Look up a cross-file value's type via per-query salsa lookup.
    ///
    /// Uses `symbol_index` to find the defining file, then queries
    /// `file_symbol(db, file, name)` to get only that value's type.
    fn cross_file_value_type(&mut self, name: &str) -> Option<Ty> {
        let db = self.db?;
        let file = self.env.symbol_index.get_file(name)?;
        let env_data = super::dependencies::file_symbol(db, file, name.to_owned());
        let file_env = &env_data.data;
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

    #[expect(dead_code)] // will be wired into inference query
    fn register_type_for_name(&self, name: &str) -> Option<Ty> {
        self.env.registers.get(name).cloned()
    }

    fn bitfield_info_for_type(&self, ty: &Ty) -> Option<(String, BitfieldInfo)> {
        match ty.kind() {
            TyKind::Named(name, _) => {
                self.env.bitfields.get(name).cloned().map(|info| (name.clone(), info))
            }
            _ => None,
        }
    }

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
                    TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => None,
                })
            }
            TyKind::App { name, args, .. } if name == "vector" => {
                args.last().and_then(|arg| match arg {
                    TyArg::Type(ty) => Some(ty.clone()),
                    TyArg::Nexp(_) | TyArg::Bool(_) | TyArg::Order(_) | TyArg::Value(_) => None,
                })
            }
            TyKind::App { name, .. } if name == "bits" => Some(Ty::named("bit".to_string())),
            _ => None,
        }
    }

    fn collection_length_text(&self, ty: &Ty) -> Option<String> {
        // A type written in an annotation can reach inference as `Named` rather
        // than `App` depending on how it was lowered, so both shapes count.
        let (name, args) = match ty.kind() {
            TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
            _ => return None,
        };
        // A type recovered from annotation text can arrive as a single named
        // type whose *name* is the whole spelling, e.g. `Named("bits(2)")`.
        if args.is_empty() {
            if let Some(inner) = name
                .strip_prefix("bits(")
                .or_else(|| name.strip_prefix("bitvector("))
                .and_then(|r| r.strip_suffix(')'))
            {
                return Some(inner.split(',').next().unwrap_or(inner).trim().to_string());
            }
        }
        if !matches!(name.as_str(), "vector" | "bits" | "bitvector") {
            return None;
        }
        args.first().map(|arg| match arg {
            TyArg::Nexp(n) => n.to_string_repr(),
            TyArg::Bool(c) => c.to_text(),
            TyArg::Order(value) | TyArg::Value(value) => value.clone(),
            TyArg::Type(ty) => ty.display_text(),
        })
    }

    fn concat_operand_info(&self, ty: &Ty) -> Option<ConcatOperandInfo> {
        match ty.kind() {
            TyKind::Scalar(crate::ty::Scalar::Bit) => Some(ConcatOperandInfo {
                width: "1".to_string(),
                elem: Ty::named("bit".to_string()),
                is_vector: false,
            }),
            TyKind::App { name, .. } | TyKind::Named(name, _)
                if name == "bits"
                    || name == "bitvector"
                    || name.starts_with("bits(")
                    || name.starts_with("bitvector(") =>
            {
                Some(ConcatOperandInfo {
                    width: self.collection_length_text(ty)?,
                    elem: Ty::named("bit".to_string()),
                    is_vector: false,
                })
            }
            TyKind::App { name, .. } | TyKind::Named(name, _) if name == "vector" => {
                Some(ConcatOperandInfo {
                    width: self.collection_length_text(ty)?,
                    elem: self.collection_element_type(ty)?,
                    is_vector: true,
                })
            }
            _ => None,
        }
    }

    fn concat_width_text(&self, lhs: &str, rhs: &str) -> String {
        let lhs_value = parse_numeric_expr_text(lhs)
            .and_then(|expr| super::integer::eval(&expr, &Subst::default(), &[]));
        let rhs_value = parse_numeric_expr_text(rhs)
            .and_then(|expr| super::integer::eval(&expr, &Subst::default(), &[]));
        match (lhs_value, rhs_value) {
            (Some(lhs), Some(rhs)) => (lhs + rhs).to_string(),
            _ => format!("({lhs}) + ({rhs})"),
        }
    }

    /// Resolve an overloaded binary operator to the return type of the
    /// first matching candidate (plausibility filter, then full unification).
    fn resolve_overloaded_binop(&mut self, op_str: &str, lhs_ty: &Ty, rhs_ty: &Ty) -> Option<Ty> {
        // `lookup_functions` already expands an overload group and consults
        // the cross-file scheme table, which is where a prelude operator
        // lives: `overload operator =^ = {sail_mask}` is declared in
        // lib/vector.sail. Resolving the members by hand only saw the local
        // file, so every prelude operator fell through to the caller's guess.
        let mut all_schemes: SmallVec<[std::sync::Arc<TypeScheme>; 8]> =
            self.env.lookup_overload_members_cross_file(op_str).into_iter().collect();
        // Local and imported indexes may contribute the same signature.
        // Repeated evidence is not an ambiguous overload, including when a
        // literal's singleton head differs from its parameter's `int` head.
        let mut i = 0;
        while i < all_schemes.len() {
            if all_schemes[..i].contains(&all_schemes[i]) {
                all_schemes.remove(i);
            } else {
                i += 1;
            }
        }
        if all_schemes.is_empty() {
            return None;
        }

        // Compare against the expanded form: `xlenbits` is `bits(xlen)`, and
        // an unexpanded alias is a non-primitive leaf, which the plausibility
        // filter treats as compatible with everything.
        let lhs_resolved = self.table.resolve(lhs_ty);
        let rhs_resolved = self.table.resolve(rhs_ty);
        let lhs_ty = &self.table.normalize_alias_ty(&lhs_resolved);
        let rhs_ty = &self.table.normalize_alias_ty(&rhs_resolved);

        // Plausibility filter: arity must be 2 + leaf-name match on both args.
        let plausible: Vec<_> = all_schemes
            .iter()
            .filter(|s| {
                s.params.len() == 2
                    && self.overload_types_plausible(&s.params[0], lhs_ty)
                    && self.overload_types_plausible(&s.params[1], rhs_ty)
            })
            .collect();

        // An operator is overloaded across the prelude's files — `|` is
        // `or_vec` in vector.sail and `or_bool` in flow.sail — so more than
        // one member is routinely plausible. Unification here is permissive
        // enough to accept either, and taking whichever comes first types
        // `bits & bits` as `bool`. Only commit when the choice is forced:
        // when a candidate's parameters name the same constructors as the
        // arguments, or when there is a single candidate at all. Otherwise
        // leave it unresolved and let the caller fall back to its own guess,
        // which is what happened before cross-file members were visible.
        let leaves = |s: &TypeScheme| {
            (
                type_leaf_name(&self.table.normalize_alias_ty(&s.params[0])),
                type_leaf_name(&self.table.normalize_alias_ty(&s.params[1])),
            )
        };
        let lhs_leaf = type_leaf_name(lhs_ty);
        let rhs_leaf = type_leaf_name(rhs_ty);
        // The plausibility filter is deliberately loose: it passes any name it
        // cannot expand, so a cross-file alias like `xlenbits` looks
        // compatible with `bool`. Picking the first survivor of a loose filter
        // is how `bits & bits` ends up typed `bool` — an operator is
        // overloaded across the prelude's files (`|` is `or_vec` in
        // vector.sail and `or_bool` in flow.sail), so several members are
        // routinely plausible and unification here accepts either. Commit only
        // when the choice is forced: exactly one candidate whose parameters
        // name compatible constructors, preferring the ones that match
        // outright. Otherwise leave the operator unresolved and let the caller
        // fall back to its own guess.
        let compatible: Vec<_> = plausible
            .iter()
            .filter(|s| {
                let (p0, p1) = leaves(s);
                leaf_names_compatible(&p0, &lhs_leaf) && leaf_names_compatible(&p1, &rhs_leaf)
            })
            .copied()
            .collect();
        let mut ordered: Vec<_> = compatible
            .iter()
            .filter(|s| {
                let (p0, p1) = leaves(s);
                lhs_leaf != "?" && rhs_leaf != "?" && p0 == lhs_leaf && p1 == rhs_leaf
            })
            .copied()
            .collect();
        if ordered.is_empty() {
            if compatible.len() != 1 {
                return None;
            }
            ordered = compatible;
        }

        // Full unification on the chosen candidates with snapshot/rollback.
        let args = [lhs_ty.clone(), rhs_ty.clone()];
        for scheme in ordered {
            let (freshened, _) = self.freshen_scheme(scheme);
            let mut mrc =
                crate::method_resolution::MethodResolutionContext { table: &mut self.table };
            let ok = mrc.try_candidate(&freshened.params, &args);
            if ok {
                return Some(self.table.deep_resolve(&freshened.ret));
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
        // `(T)` is `T` — a parenthesised operand such as `0b0 @ (x : bits(2))`
        // reaches here as a one-element tuple.
        let unwrap_parens = |t: &Ty| -> Ty {
            match t.kind() {
                TyKind::Tuple(items) if items.len() == 1 => items[0].clone(),
                _ => t.clone(),
            }
        };
        // Expand type aliases before classifying. `xlenbits` and friends are
        // declared in another file, which used to put them out of reach here;
        // the workspace env merges cross-file aliases now, so the operand of
        // `imm @ rd` arrives as a name this can resolve to `bits(...)`.
        // Deeply, through parameterised schemes: `type ppn_bits('v) = bits(..)`
        // and `type csreg = bits(12)` are both recorded as schemes rather
        // than as plain aliases, so the shallow expansion left every one of
        // them looking like a name this rule could not classify. Normalize
        // once and use the result everywhere below — classifying on the
        // expanded type while `concat_operand_info` saw the unexpanded one
        // is what turned four sail-riscv files red.
        // `_for_shape`, so a configured width — `type xlenbits = bits(xlen)`,
        // which is most of sail-riscv — is still a bitvector here. What it
        // may not do is decide a constraint; the width it yields stays
        // symbolic and `concat_width_text` renders `(xlen) + ('n)`.
        let lhs_ty = &self.table.normalize_alias_deep_for_shape(&unwrap_parens(lhs_ty));
        let rhs_ty = &self.table.normalize_alias_deep_for_shape(&unwrap_parens(rhs_ty));
        // Anything still unresolved after that is left alone: reporting on a
        // name this checker cannot see is how the concat rule produced false
        // positives before.
        // By constructor name, not by rendered text: `bits (8)` is `bits`,
        // and a `starts_with("bits(")` test says it is not.
        if self.in_assign_target {
            let lhs = self.table.normalize_alias_deep_for_shape(lhs_ty);
            let rhs = self.table.normalize_alias_deep_for_shape(rhs_ty);
            let bits = |t: &Ty| matches!(t.ctor_name(), Some("bits" | "bitvector"));
            if (bits(&lhs) && rhs.ctor_name() == Some("vector"))
                || (lhs.ctor_name() == Some("vector") && bits(&rhs))
            {
                self.invalid_expression(
                    expr_id,
                    "Concatenation l-expression cannot mix bitvector and vector types",
                );
            }
        }
        let is_definitely_concatenable = |t: &Ty| {
            matches!(
                // A single bit concatenates as a width-1 vector, which
                // `concat_operand_info` has always handled — the guard just
                // never let one through.
                //
                // A configured width is still a width: `xlenbits` is a
                // bitvector whether `xlen` is 32 or 64, and the comment that
                // used to stand here — "letting it past only moves the
                // failure to width extraction" — was a dated claim.
                // `collection_length_text` reads the width off the expanded
                // type and hands back the name; the failure it predicted
                // does not happen.
                self.table.normalize_alias_deep_for_shape(t).ctor_name(),
                Some("bits" | "vector" | "bitvector" | "bit")
            )
        };
        let is_definitely_not_concatenable = |t: &Ty| {
            matches!(t.ctor_name(), Some("int" | "nat" | "bool" | "string" | "unit" | "real"))
        };
        // `^` is string append when either operand is a string
        // (`"a" ^ spc() ^ f(x)`), and Sail overloads the operator rather than
        // reserving it for vectors. Reporting a vector-concat error there is
        // wrong even when the other operand's type is not yet resolved.
        let is_string = |t: &Ty| matches!(t.display_text().as_str(), "string" | "string_literal");
        if is_string(lhs_ty) || is_string(rhs_ty) {
            return Ty::named("string".to_string());
        }
        // A user-declared struct, enum or union is definitely not a
        // bitvector, so `imm @ rd` where `rd : regidx` is an error even
        // though `regidx` is not a builtin.
        let is_user_non_vector = |t: &Ty| {
            let text = t.display_text();
            let base = text.split('(').next().unwrap_or(&text);
            self.env.records.contains_key(base)
                || self.env.enums.contains_key(base)
                || self.env.unions.contains_key(base)
        };
        for (ty, side) in [(lhs_ty, "lhs"), (rhs_ty, "rhs")] {
            let _ = side;
            if is_user_non_vector(ty) {
                self.push_inference_diagnostic(InferenceDiagnostic::ConcatTypeMismatch {
                    expr: expr_id,
                    message: format!("Expected bitvector type, got {}", ty.display_text()),
                });
                return Ty::error();
            }
        }
        // One side still an inference variable is the common case — the
        // operand is a call whose result is not resolved yet — and it was
        // 89 of the 92 binary operations sail-riscv could not type. `@` is
        // the only operator that reaches here (`^` lowers to BitXor), so a
        // bitvector on one side makes the result a bitvector whatever the
        // other turns out to be. The width is left open rather than guessed:
        // an apostrophe-prefixed name is what `unify_ty_arg` treats as
        // undecidable, so this constrains nothing it cannot prove.
        let unresolved = |t: &Ty| matches!(t.kind(), TyKind::Infer(_));
        for (known, other) in [(lhs_ty, rhs_ty), (rhs_ty, lhs_ty)] {
            if unresolved(other) && matches!(known.ctor_name(), Some("bits" | "bitvector")) {
                let width = format!("'_cat{}", self.fresh_var_counter);
                self.fresh_var_counter += 1;
                return bits_ty(width);
            }
        }

        for t in [lhs_ty, rhs_ty] {
            if !is_definitely_concatenable(t) && !is_definitely_not_concatenable(t) {
                return Ty::error();
            }
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
    fn check_match_exhaustiveness_hir(
        &mut self,
        match_expr: hir_def::ExprId,
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
            TyKind::Named(n, _) => Some(n.as_str()),
            TyKind::App { name, .. } => Some(name.as_str()),
            TyKind::Scalar(Scalar::Bool) => Some("bool"),
            _ => None,
        };
        if let Some(name) = type_name {
            // bool is always fully known — skip guard.
            if !matches!(name, "bool" | "atom_bool") {
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
        let lowered_arms =
            match_check::lower_arms_hir(body, arms, self.source_map.as_deref(), &is_constant);

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
                        TyKind::Named(name, _) if name == "unit" => 0,
                        _ => 1,
                    },
                    many => many.len(),
                };
                arity_map.insert(name.clone(), arity);
            }
        }
        for (name, arities) in self.env.cross_file_function_arity.iter() {
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
            // Expand type aliases before lowering. `type instbits = bits(32)`
            // otherwise reaches the matrix as an opaque named type whose
            // constructors cannot be listed, and the checker reports the
            // *type name* as a missing pattern.
            let expand = |ty: &Ty| -> Ty {
                let mut current = ty.clone();
                let mut seen = std::collections::HashSet::new();
                loop {
                    let name = match current.kind() {
                        TyKind::Named(n, _) | TyKind::App { name: n, .. } => n.clone(),
                        _ => return current,
                    };
                    if !seen.insert(name.clone()) {
                        return current;
                    }
                    match env_ref.type_aliases.get(&name) {
                        Some(expanded) => current = expanded.clone(),
                        None => return current,
                    }
                }
            };
            // Filter out unit-payload constructors (arity 0 — see arity_map above).
            let payload: Vec<MatchTy> = scheme
                .params
                .iter()
                .map(|p| ty_to_match_ty_with_subst(&expand(p), &subst, &env_ref.records))
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
        if has_concrete_witness && !self.suppress_incomplete_match {
            self.has_incomplete_match = true;
            self.push_inference_diagnostic(InferenceDiagnostic::IncompleteMatch {
                expr: match_expr,
                missing_arms: report
                    .missing_witnesses
                    .iter()
                    .filter(|w| !matches!(w, match_check::MatchPat::Wild))
                    .map(|w| w.display_text())
                    .collect(),
            });
        }
        for (pat_id, _) in &report.redundant {
            if let Some(pat) = pat_id {
                self.push_inference_diagnostic(InferenceDiagnostic::RedundantMatchArm {
                    pat: *pat,
                });
            }
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
        expectation: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        // The receiver is the first argument of Sail's legacy `_mod_field`
        // accessor call, and keeps its own source expression/type for IDEs.
        let receiver_args;
        let args = if let Some(hir_def::Expr::Field { expr, .. }) = body.expr(callee_id) {
            receiver_args = std::iter::once(*expr).chain(args.iter().copied()).collect::<Vec<_>>();
            receiver_args.as_slice()
        } else {
            args
        };
        let out = self.infer_call_hir_inner(body, callee_id, args, expectation, locals);
        if !out.is_error() {
            if let Some(name) = self.unavailable_nominal_type(
                self.result.type_of_expr.get(callee_id).into_iter().chain(std::iter::once(&out)),
            ) {
                self.invalid_expression(callee_id,
                    format!("Well-formedness check failed: type {name} in this call is not visible in the current module"));
            }
        }
        // `SAIL_UNTYPED_CALL=1`, beside `SAIL_UNTYPED_IDENT`: which call came
        // back `error()`, what the context expected of it, and whether this
        // checker has its signature at all. The three-way split is the whole
        // question — a call with no signature is a name the environment never
        // learned, and a call with one is a constraint that could not be
        // discharged — and reading it off the source is guesswork.
        if out.is_error() && std::env::var("SAIL_UNTYPED_CALL").is_ok() {
            let name = match body.expr(callee_id) {
                Some(hir_def::Expr::Ident(n)) => n.to_string(),
                _ => "<non-ident>".to_string(),
            };
            let exp = match expectation {
                Expectation::HasType(t) => t.display_text(),
                _ => "<none>".to_string(),
            };
            let known = !self.env.lookup_functions(&name).is_empty();
            eprintln!("[call] {name} expected={exp} signature_known={known}");
        }
        out
    }

    fn infer_call_hir_inner(
        &mut self,
        body: &hir_def::Body,
        callee_id: hir_def::ExprId,
        args: &[hir_def::ExprId],
        expectation: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        let expected_ret = match expectation {
            Expectation::HasType(ty) if !ty.is_error() => Some(ty.clone()),
            _ => None,
        };
        use hir_def::Expr;

        // 1. Extract callee name
        let callee_name = match body.expr(callee_id) {
            Some(Expr::Ident(name)) => name.clone(),
            Some(Expr::Field { field, .. }) => {
                // Method-like call: obj.method(args) → _mod_method(obj, args)
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

        // Give the callee itself a type. Inference never visits it — the
        // name is read straight off the CST — so `f` in `f(x)` was one of
        // the 23,590 untyped `Ident` expressions on sail-riscv, and hover
        // over a call's function name had nothing to show. Overload
        // resolution overwrites this below once it has picked a candidate.
        let callee_sig = self
            .env
            .lookup_functions(&callee_name)
            .first()
            .map(|s| crate::ty::FnSig { params: s.params.clone(), ret: s.ret.clone() })
            .or_else(|| {
                // A mapping is callable in either direction; the forward one
                // is what a call expression means.
                self.env
                    .lookup_mappings(&callee_name)
                    .first()
                    .map(|m| crate::ty::FnSig { params: vec![m.lhs.clone()], ret: m.rhs.clone() })
            });
        match callee_sig {
            Some(sig) => self.result.write_expr_ty(callee_id, Ty::new(TyKind::FnPtr(sig))),
            // The `#`-suffixed builtins are handled by the arms below, which
            // return before overload resolution runs, so their callee never
            // reached the write above. It is still an expression in the
            // arena, and `v[i] = e` desugars to one on every vector update.
            None if callee_name.ends_with('#') => {
                self.result.write_expr_ty(callee_id, self.table.new_type_var())
            }
            None => {}
        }

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

        // The expected union fixes its constructor payload before arguments
        // are inferred. In particular an anonymous generic record must see
        // Output('n), rather than first being inferred as bare Output.
        let constructor_context = expected_ret.as_ref().and_then(|expected| {
            let schemes = self.env.constructors.get(&callee_name)?;
            let [scheme] = schemes.as_slice() else { return None };
            let mut subst = Subst::default();
            let expected = self.table.normalize_alias_deep(expected);
            if !constraint::collect_type_bindings(&scheme.ret, &expected, &mut subst) {
                return None;
            }
            let params: Vec<_> =
                scheme.params.iter().map(|p| constraint::apply_subst(p, &subst)).collect();
            if params.len() == 1 && args.len() > 1 {
                if let TyKind::Tuple(items) = self.table.normalize_alias_deep(&params[0]).kind() {
                    return Some(items.clone());
                }
                return None;
            }
            Some(params)
        });
        // A unary bitvector operation whose viable signatures preserve the
        // operand type can pass its result context into the operand. This
        // prevents an unconstrained nested extension from selecting a one-bit
        // overload before an enclosing bitwise expression supplies its width.
        let unary_context = expected_ret.as_ref().filter(|expected| {
            if args.len() != 1 || !self.table.normalize_alias_deep(expected).is_bits() {
                return false;
            }
            let candidates = self.env.lookup_functions(&callee_name);
            let viable: Vec<_> = candidates
                .iter()
                .filter(|c| self.overload_types_plausible(&c.ret, expected))
                .collect();
            !viable.is_empty() && viable.iter().all(|c| c.params.len() == 1 && c.params[0] == c.ret)
        });
        // 2. Infer argument types
        let arg_types: Vec<Ty> = args
            .iter()
            .enumerate()
            .map(|(index, &a)| {
                let expected = constructor_context
                    .as_ref()
                    .and_then(|params| params.get(index))
                    .or(unary_context);
                let ty = if let Some(expected) = expected {
                    self.infer_expr_expecting(
                        body,
                        a,
                        &Expectation::HasType(expected.clone()),
                        locals,
                    )
                } else {
                    self.infer_expr_hir(body, a, locals)
                };
                // A bare alias — `type bits_H = bits(16)` — tells a
                // parameter `bits('m)` nothing while it is still spelled
                // `bits_H`, and the substitution the constraint check reads
                // is built by the unifier in `constraint.rs`, which has no
                // aliases to consult. So `'m` was unresolved, the candidate
                // was dropped, and the call and everything read from it came
                // back untyped.
                let ty = self.table.expand_named_alias(&ty);
                // A returned existential is eliminated before instantiating
                // the callee. Its witnesses and bounds remain in this call's
                // scope, so an enclosing extension can bind its input width
                // to the width of e.g. ones(runtime_length).
                let ty = if let TyKind::Exist { vars, constraint, inner } = ty.kind() {
                    let opened = self.table.open_existential(vars, constraint, inner);
                    locals.add_constraint(opened.constraint);
                    opened.inner_ty
                } else {
                    ty
                };
                let ty = self.boolean_operand(ty, locals);
                let ty = if constraint::is_numeric_scalar_ty(&ty) {
                    literal_evidence(body, a, &ty)
                } else {
                    ty
                };
                if constraint::is_numeric_scalar_ty(&ty)
                    && integer_literal_value(body, a).is_none()
                    && matches!(
                        body.expr(a),
                        Some(
                            Expr::BinaryOp { .. }
                                | Expr::UnaryOp { .. }
                                | Expr::TypeVar(_)
                                | Expr::SizeOf { .. }
                        )
                    )
                {
                    if let Some(n) =
                        super::refinement::number(body, a, &|n| self.refinement_value(locals, n))
                    {
                        let n = super::substitution::numeric(&n, &locals.type_substitution());
                        return Ty::app("atom", vec![TyArg::Nexp(n)], "");
                    }
                }
                ty
            })
            .collect();
        // Keep literal values for legacy implicit-argument binding as well.
        // Call checking uses singleton evidence locally; the expression arena
        // keeps coarse literal types for unconstrained collection joins.
        let arg_lits: Vec<Option<i128>> =
            args.iter().map(|&a| integer_literal_value(body, a)).collect();

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
            // The index's *type* has to fit the vector, not just a literal
            // written at the call. `v[unsigned(n)]` for an `n : bits(32)`
            // indexes with a `range(0, 4294967295)`, which no two- or
            // three-bit vector can hold — upstream proves `0 <= i < 'n` and
            // rejects it, and nothing here looked past a literal.
            //
            // Only when *both* ends are decided: a concrete width, and an
            // index range with a concrete upper bound. A bare `int` index or
            // a symbolic width says nothing — that is the solver's, and
            // guessing there is what the `UnresolvedCallQuantifiers` trade
            // measured at ten wrong answers for three right ones.
            if let (Some(info), Some((_, idx_hi))) =
                (self.concat_operand_info(base_ty), concrete_range_bounds(index_ty))
            {
                let width = info
                    .width
                    .parse::<i128>()
                    .ok()
                    .or_else(|| self.quant_max.get(info.width.trim()).copied());
                if let Some(width) = width {
                    if idx_hi >= width {
                        let idx_span = arg_spans.get(1).copied().unwrap_or(Span::new(0, 0));
                        self.push_inference_diagnostic(InferenceDiagnostic::ConstraintViolation {
                            expr: callee_id,
                            constraint: format!("0 <= index < {width}"),
                            derived_from: vec![idx_span],
                        });
                    }
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
            let endpoint = |index: usize| {
                super::refinement::number_from_types(
                    body,
                    args[index],
                    &|name| self.refinement_value(locals, name),
                    &|id| self.result.expr_ty(id).cloned(),
                )
                .or_else(|| super::refinement::singleton(&arg_types[index]))
            };
            if let (Some(first), Some(last)) = (endpoint(1), endpoint(2)) {
                use NumericExpr as N;
                let (upper, lower) = match self.env.vector_order {
                    hir_def::type_error::VectorOrder::Dec => (first, last),
                    hir_def::type_error::VectorOrder::Inc => (last, first),
                };
                let width = N::Add(
                    Box::new(N::Sub(Box::new(upper), Box::new(lower))),
                    Box::new(N::Const(1)),
                );
                let width = super::substitution::numeric(&width, &locals.type_substitution());
                let shape = self.table.normalize_alias_deep_for_shape(base_ty);
                if let Some(info) = self.concat_operand_info(&shape) {
                    let width = TyArg::Nexp(super::nexp_simplify::simplify(&width));
                    let expected = if info.is_vector {
                        Ty::app("vector", vec![width, TyArg::Type(info.elem)], "")
                    } else {
                        Ty::app("bits", vec![width], "")
                    };
                    // Bind a replacement call's still-flexible width as well
                    // as checking concrete widths. Deferred call predicates
                    // see this binding when the enclosing body is finished.
                    if !value_ty.is_error() && !self.table.unify(&expected, value_ty) {
                        self.result.record_type_mismatch(args[3], &expected, value_ty);
                    }
                }
            }
            return base_ty.clone();
        }
        // `v[i] = e` desugars to `vector_update#(v, i, e)`, and a vector
        // update returns the vector. Upstream reads the same thing off the
        // first argument, rewriting to the `vector_update` overload or, when
        // the base is a bitfield, to that field's update accessor
        // (type_check.ml:4011). Both keep the base's type.
        if callee_name == "vector_update#" {
            if let Some(base) = arg_types.first() {
                if !base.is_error() {
                    return base.clone();
                }
            }
            return Ty::error();
        }
        if callee_name == "vector_subrange#" || callee_name == "slice" {
            return Ty::error();
        }

        // 4. Mappings, before any function lookup.
        //
        // Upstream matches `E_app (m, xs) when Env.is_mapping m env` ahead of
        // the general application rules (type_check.ml:3756). Order matters
        // here for the same reason: a mapping's `val spc : unit <-> string`
        // also registers `spc` as a cross-file *function* name, so the lookup
        // below found no scheme and returned `error()` for every call to a
        // mapping declared in another file — `spc`, `sep`, `reg_name`,
        // `encdec_reg` and the rest of sail-riscv's assembly and encdec
        // vocabulary.
        {
            let mappings = self.env.lookup_mappings(&callee_name);
            if !mappings.is_empty() {
                // "All constructors and mappings are treated as having one
                // argument, so `Ctor(x, y)` is checked as `Ctor((x, y))`"
                // (type_check.ml:2305).
                let arg = match arg_types.len() {
                    0 => Ty::named("unit".to_string()),
                    1 => arg_types[0].clone(),
                    _ => Ty::tuple(arg_types.clone()),
                };
                // With no information about the argument both directions
                // unify, and picking forwards is a guess that manufactures
                // mismatches downstream — `bool_bit(menvcfg[SSE])` would
                // claim `bits(1)` where the match arm beside it is `bool`.
                let resolved_arg = self.table.resolve(&arg);
                if resolved_arg.is_error()
                    || matches!(resolved_arg.kind(), TyKind::Infer(_) | TyKind::Param(_))
                {
                    if std::env::var("SAIL_UNTYPED_CALL").is_ok() {
                        eprintln!(
                            "[mapcall] {callee_name} arg={} (unusable)",
                            resolved_arg.display_text()
                        );
                    }
                    return Ty::error();
                }
                use crate::infer::mapping::{resolve_mapping_call, MappingCallResult};
                // Every scheme, not just the first. A name can carry more
                // than one, and reporting on the first one's failure would
                // be reporting on a signature the call was never using.
                // Which direction a mapping call takes is decided by
                // `unify`, and `unify` lets a non-primitive `Named` accept
                // anything — so `m : W <-> bits(2)` applied to `0b11` chose
                // *forwards*, unifying `W` with `bits(2)`, and answered
                // `bits(2)` where the answer is `W`. In sail-riscv that is
                // `encdec_compressed(bv)` typing as `bits(16)` instead of
                // `instruction`, at the centre of instruction decode.
                //
                // A side whose name is the argument's name decides the
                // direction on its own, and no unification can argue with
                // it. Only when neither side names the argument does the
                // permissive path below get to choose.
                // `as_name` answers for a `Named` or a `Scalar` and not for
                // an `App`, and `bits(2)` is an `App` — comparing with it
                // found nothing to match.
                let head_name = |t: &Ty| match t.kind() {
                    TyKind::Named(n, _) | TyKind::App { name: n, .. } => Some(n.clone()),
                    TyKind::Scalar(s) => Some(s.name().to_owned()),
                    _ => None,
                };
                let arg_name = head_name(&self.table.deep_resolve(&arg));
                if let Some(arg_name) = arg_name {
                    for scheme in &mappings {
                        let lhs = self.table.insert_type_vars(&scheme.lhs);
                        let rhs = self.table.insert_type_vars(&scheme.rhs);
                        let names = (
                            head_name(&lhs).as_deref() == Some(arg_name.as_str()),
                            head_name(&rhs).as_deref() == Some(arg_name.as_str()),
                        );
                        // The head name picks the direction; the whole type
                        // still has to agree. `bin_bits_2 : bits(3) <->
                        // string` applied to a `bits(2)` names `bits` on the
                        // left and is wrong anyway, and taking the direction
                        // on the name alone answered `string` for it.
                        match names {
                            (true, false) if self.table.unify(&lhs, &arg) => {
                                return self.table.deep_resolve(&rhs);
                            }
                            (false, true) if self.table.unify(&rhs, &arg) => {
                                return self.table.deep_resolve(&lhs);
                            }
                            _ => {}
                        }
                    }
                }
                let mut first = None;
                let mut all_sides_known = true;
                for scheme in &mappings {
                    let lhs = self.table.insert_type_vars(&scheme.lhs);
                    let rhs = self.table.insert_type_vars(&scheme.rhs);
                    all_sides_known &= lhs.is_fully_known() && rhs.is_fully_known();
                    match resolve_mapping_call(&mut self.table, &lhs, &rhs, &arg) {
                        MappingCallResult::Forwards(ty) => {
                            let _ = self.table.unify(&lhs, &arg);
                            return self.table.deep_resolve(&ty);
                        }
                        MappingCallResult::Backwards(ty) => {
                            let _ = self.table.unify(&rhs, &arg);
                            return self.table.deep_resolve(&ty);
                        }
                        MappingCallResult::NoMatch => {
                            if first.is_none() {
                                first = Some(rhs);
                            }
                        }
                    }
                }
                // No direction of any scheme accepts the argument. Upstream
                // reports "no overloading" here (type_check.ml:3756).
                //
                // This used to stay quiet, on the grounds that a failure
                // usually meant a type had not resolved rather than that the
                // call was wrong. That is a real risk and it is what
                // `is_fully_known` is for: a `bits('n)` mapping side, or an
                // argument with an inference variable anywhere inside it,
                // cannot disagree with anything yet. When every side and the
                // argument are concrete, the disagreement is real —
                // `bin_bits_2 : bits(3) <-> string` applied to a `bits(2)`
                // is the shape upstream rejects and this accepted.
                if all_sides_known && self.table.deep_resolve(&arg).is_fully_known() {
                    self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                        call_expr: callee_id,
                        name: callee_name.clone(),
                        no_function: false,
                        closest: None,
                    });
                }
                return first.map(|rhs| self.table.deep_resolve(&rhs)).unwrap_or_else(Ty::error);
            }
        }

        // 5. Cross-file: look up the function scheme from the defining file
        //    so we return a real type instead of Ty::error().
        let cross_file_known = self.env.cross_file_function_names.contains(&callee_name)
            || self.env.cross_file_constructor_names.contains(&callee_name);
        // A name-only index chooses one file, which is insufficient when
        // imported constructors share a name (e.g. Exception). Let the normal
        // candidate resolver use all signatures, payloads and the expected
        // return type. Local constructors retain their shadowing precedence.
        let imported_constructor_collision = !self.env.constructors.contains_key(&callee_name)
            && self.env.cross_file_constructor_schemes.get(&callee_name).is_some_and(|schemes| {
                schemes.first().is_some_and(|first| {
                    schemes.iter().skip(1).any(|s| s.ret.ctor_name() != first.ret.ctor_name())
                })
            });
        if cross_file_known
            && !self.env.functions.contains_key(&callee_name)
            && !imported_constructor_collision
        {
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
            // A local declaration shadows the imported one of the same name:
            // a file that declares `union U('a) = { Err : 'a, Ok : bits(16) }`
            // means its own `Ok`, not the prelude's `result` constructor.
            if let Some(scheme) =
                self.env.constructors.get(&callee_name).and_then(|schemes| schemes.first()).cloned()
            {
                // Returning the result type without looking at the arguments
                // meant a synthesized constructor was never checked at all:
                // `Mk_B1(0x12345678)` against `bitfield B1 : bits(33)` passed
                // silently. Resolution stays as it was — this path exists
                // because a union clause's payload may live in another file —
                // but a parameter that is known and does not match is
                // reported.
                let params: Vec<Ty> = scheme
                    .params
                    .iter()
                    .zip(scheme.implicit_params.iter().chain(std::iter::repeat(&false)))
                    .filter(|(_, implicit)| !**implicit)
                    .map(|(p, _)| p.clone())
                    .collect();
                // `Ctor(x, y, z)` for `Ctor : (A, B, C)` is `Ctor((x, y, z))`
                // — one tuple parameter against three arguments. The counts
                // disagreed, so the loop below did nothing and a
                // constructor's tuple payload went unchecked. An existential
                // payload is opened first: `Ctor : {'d, datasize('d). (nat,
                // int('d), bits(4))}` is applied like any other triple, the
                // same rule `Pat::App` already follows for the pattern side.
                let params = if params.len() == 1 && arg_types.len() > 1 {
                    let only = self.table.resolve(&params[0]);
                    // Open the existential into *inference variables*, not
                    // into the `Param`s it was written with. A `Param`
                    // unifies with anything, so `Foo : {'n, 'n in {16, 32}.
                    // (int('n), bits('n), bool)}` applied to `(16, imm, ..)`
                    // never noticed that the `16` fixes `'n` and the `imm`
                    // then has to be a `bits(16)` — upstream says "Multiple
                    // non-identical unifiers for 'n: 16 and 17". One
                    // variable shared across the payload's components is the
                    // whole content of an existential.
                    let mut payload: Option<(ConstraintExpr, Vec<(String, String)>)> = None;
                    let only = match only.kind() {
                        TyKind::Exist { vars, constraint, inner } => {
                            let subst = self.table.existential_substitution(
                                vars,
                                constraint,
                                inner,
                                &Ty::tuple(arg_types.clone()),
                            );
                            let witnesses = vars
                                .iter()
                                .filter_map(|v| {
                                    subst.values.get(v).map(|p| (v.clone(), p.to_owned()))
                                })
                                .collect();
                            payload = Some((constraint.clone(), witnesses));
                            super::constraint::apply_subst(inner, &subst)
                        }
                        _ => only,
                    };
                    self.pending_payload = payload;
                    match only.kind() {
                        TyKind::Tuple(items) if items.len() == arg_types.len() => items.clone(),
                        // Applied to several arguments, and the payload
                        // cannot take them: a tuple of another length, or
                        // not a tuple at all. The counts simply disagreed
                        // here, so the loop below did nothing and the call
                        // went unchecked — `Ctor1(a, x)` for a payload of
                        // three, and `Ething(Athing, 3)` for a payload of
                        // one, both passed. Upstream compares the two
                        // tuples: "Type mismatch between (bits(4), int('d),
                        // bits(4)) and (bits(4), int('d))".
                        TyKind::Tuple(items) if !only.is_error() => {
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::MismatchedArgCount {
                                    call_expr: callee_id,
                                    expected: items.len(),
                                    found: arg_types.len(),
                                },
                            );
                            params
                        }
                        _ if !only.is_error()
                            && !matches!(only.kind(), TyKind::Param(_) | TyKind::Infer(_)) =>
                        {
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::MismatchedArgCount {
                                    call_expr: callee_id,
                                    expected: 1,
                                    found: arg_types.len(),
                                },
                            );
                            params
                        }
                        _ => params,
                    }
                } else {
                    params
                };
                // `Some(x)` for `Some : 'a` in `union option('a)`: the
                // payload's type is whatever the context asks the union to
                // be. Nothing bound `'a` here, and `unify` lets a `Param`
                // accept anything, so `Some((i, i))` against
                // `option(nat)` — and every other constructor payload of
                // the wrong shape — passed in silence. The expected type
                // says what the parameters are; match the scheme's return
                // against it and substitute.
                let params = match expectation {
                    Expectation::HasType(exp) if !exp.is_error() && !scheme.ret.is_error() => {
                        let mut subst = Subst::default();
                        if super::constraint::collect_type_bindings(&scheme.ret, exp, &mut subst) {
                            params
                                .iter()
                                .map(|p| super::constraint::apply_subst(p, &subst))
                                .collect()
                        } else {
                            params
                        }
                    }
                    _ => params,
                };
                if params.len() == arg_types.len() && !self.in_mapping_arm {
                    for (i, (param, actual)) in params.iter().zip(arg_types.iter()).enumerate() {
                        if param.is_error() || actual.is_error() {
                            continue;
                        }
                        // A literal argument says what a numeric parameter's
                        // variable *is*: `Foo(16, imm, ..)` for
                        // `(int('n), bits('n), bool)` fixes `'n` at 16, and
                        // `imm` then has to be a `bits(16)`. Typed as a bare
                        // `int` the literal bound nothing, so the *second*
                        // argument bound `'n` to its own width and the two
                        // never met. The function path already does this.
                        let actual = &singleton_for_numeric_param(
                            param,
                            actual,
                            arg_lits.get(i).copied().flatten(),
                        );
                        if !self.table.unify(param, actual) {
                            if let Some(&arg) = args.get(i) {
                                // Resolved, so the message names the width
                                // the earlier argument fixed rather than the
                                // variable that carried it.
                                let param = self.table.deep_resolve(param);
                                self.result.record_type_mismatch(arg, &param, actual);
                            }
                        }
                    }
                    self.check_payload_constraint(locals);
                }
                let expanded = self.table.normalize_alias_ty(&scheme.ret);
                return self.table.insert_type_vars(&expanded);
            }
            // Look up defining file, get scheme's return type, expand aliases,
            // then replace Params with fresh inference vars.
            if let Some(db) = self.db {
                // Symbol index lookup.
                use crate::infer::env::SymbolKind;
                if let Some((file, kind)) = self.env.symbol_index.get(&callee_name) {
                    let env_data = super::dependencies::file_symbol(db, file, callee_name.clone());
                    let schemes = match kind {
                        SymbolKind::Constructor => env_data.data.constructors.get(&callee_name),
                        _ => env_data.data.functions.get(&callee_name),
                    };
                    if let Some(scheme) = schemes.and_then(|s| s.first()) {
                        // Freshen the whole signature, not the return type
                        // alone. `sail_zeros : forall 'n. int('n) -> bits('n)`
                        // was instantiated ret-first, so `'n` got a variable
                        // nothing ever bound and every call came back a
                        // bitvector of unknown width — which then agreed with
                        // whatever width the caller had declared.
                        //
                        // Unify to *bind*, not to report: a cross-file scheme
                        // is not reliable enough to accuse a call on
                        // (`lookup_overload_members_cross_file` says so). A
                        // real disagreement still surfaces, through the
                        // resolved return type meeting the expectation.
                        let scheme = scheme.clone();
                        // Open runtime numeric values before freshening the
                        // callee: its flexible indices may refer to these rigid
                        // witnesses, including values described by a range.
                        let supplied_params: Vec<_> = scheme
                            .params
                            .iter()
                            .zip(scheme.implicit_params.iter().chain(std::iter::repeat(&false)))
                            .filter(|(_, implicit)| {
                                arg_types.len() == scheme.params.len() || !**implicit
                            })
                            .map(|(param, _)| param)
                            .collect();
                        let arg_types: Vec<Ty> = arg_types
                            .iter()
                            .enumerate()
                            .map(|(i, actual)| {
                                if supplied_params
                                    .get(i)
                                    .is_some_and(|p| super::refinement::singleton(p).is_some())
                                {
                                    self.numeric_operand(actual.clone(), locals)
                                } else {
                                    actual.clone()
                                }
                            })
                            .collect();
                        let (fresh, _) = self.freshen_scheme(&scheme);
                        let params: Vec<Ty> = fresh
                            .params
                            .iter()
                            .zip(fresh.implicit_params.iter().chain(std::iter::repeat(&false)))
                            .filter(|(_, implicit)| {
                                arg_types.len() == fresh.params.len() || !**implicit
                            })
                            .map(|(p, _)| self.table.insert_type_vars(p))
                            .collect();
                        if params.len() == arg_types.len() {
                            for (i, (param, actual)) in
                                params.iter().zip(arg_types.iter()).enumerate()
                            {
                                if param.is_error() || actual.is_error() {
                                    continue;
                                }
                                let actual = singleton_for_numeric_param(
                                    param,
                                    actual,
                                    arg_lits.get(i).copied().flatten(),
                                );
                                // Bind, do not accuse — a cross-file scheme
                                // is not reliable enough to judge a call on.
                                // Except by *kind*: Sail has no coercion
                                // between a bitvector and a number, so
                                // `sail_zero_extend(x, m)` with the two
                                // swapped is wrong whatever the widths are,
                                // and upstream says exactly that ("Type
                                // mismatch between bitvector('n) and
                                // int('m)"). Every argument-order mutant
                                // whose call crosses a file went unreported
                                // without it.
                                if let (Some(a), Some(b)) = (type_kind(param), type_kind(&actual)) {
                                    if a != b {
                                        if let Some(&arg) = args.get(i) {
                                            self.result.record_type_mismatch(arg, param, &actual);
                                        }
                                    }
                                }
                                let _ = self.table.unify(param, &actual);
                            }
                        }
                        // Indices may come from runtime arguments or the result
                        // context. Keep every declared predicate until those
                        // bindings have been resolved; binding alone is no proof.
                        for c in &fresh.constraints {
                            self.table.push_subtype_obligation(Obligation::NumericConstraint(
                                c.expr.clone(),
                            ));
                        }
                        let expanded = self.table.normalize_alias_ty(&fresh.ret);
                        // The whole scheme was already freshened together.
                        // Resolving its return preserves caller-bound symbolic
                        // indices; freshening again would erase `2 ^ 'n - 1`
                        // and disconnect that bound from the argument width.
                        let instantiated = self.table.deep_resolve(&expanded);
                        self.defer_implicit_arguments(callee_id, &fresh, arg_types.len(), locals);
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
                                let env_data =
                                    super::dependencies::file_symbol(db, file, member.clone());
                                if let Some(schemes) = env_data.data.functions.get(member.as_str())
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
                                // `f()` passes `unit`, written as an empty
                                // argument list. Counting that as zero
                                // arguments against a `unit` parameter left
                                // `PC()` with no plausible member at all.
                                let unit_only = arg_types.is_empty()
                                    && c.params.len() == 1
                                    && c.params[0].as_name() == Some("unit");
                                if !unit_only
                                    && (arg_types.len() < required
                                        || arg_types.len() > c.params.len())
                                {
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
                                    .all(|(p, a)| self.overload_types_plausible(p, a))
                            })
                            .collect();
                        // The most specific, not the first one listed.
                        // `shl_int` is an overload of `_shl1`, `_shl8` and
                        // `_shl32`, each taking a *singleton* first argument
                        // — `int(1)`, `int(8)`, `int(32)` — and returning a
                        // different set. `types_plausibly_compatible` says
                        // yes to all three for a literal `8`, so whichever
                        // came first won and `shl_int(8, ..)` returned
                        // `{1, 2, 4, 8}`.
                        let specificity = |c: &TypeScheme| -> usize {
                            c.params
                                .iter()
                                .zip(c.implicit_params.iter().chain(std::iter::repeat(&false)))
                                .filter(|(_, im)| !**im)
                                .map(|(p, _)| p)
                                .zip(arg_lits.iter())
                                .filter(|(param, lit)| {
                                    let Some(lit) = lit else { return false };
                                    concrete_singleton_value(param) == Some(*lit)
                                })
                                .count()
                        };
                        let best_by_specificity = plausible
                            .iter()
                            .max_by_key(|c| specificity(c))
                            .filter(|c| specificity(c) > 0);
                        if let Some(best) = best_by_specificity.or_else(|| plausible.first()) {
                            // Freshen the whole signature and unify the
                            // arguments against it, the same way the direct
                            // cross-file call does. Instantiating the return
                            // type alone left `zeros(64)` — an overload whose
                            // chosen member is `sail_zeros : forall 'n.
                            // int('n) -> bits('n)` — a bitvector of unknown
                            // width, which agreed with any declared width.
                            let best = (*best).clone();
                            let (fresh, _) = self.freshen_scheme(&best);
                            let params: Vec<Ty> = fresh
                                .params
                                .iter()
                                .zip(fresh.implicit_params.iter().chain(std::iter::repeat(&false)))
                                .filter(|(_, implicit)| !**implicit)
                                .map(|(p, _)| self.table.insert_type_vars(p))
                                .collect();
                            if params.len() == arg_types.len() {
                                for (i, (param, actual)) in
                                    params.iter().zip(arg_types.iter()).enumerate()
                                {
                                    if param.is_error() || actual.is_error() {
                                        continue;
                                    }
                                    let actual = singleton_for_numeric_param(
                                        param,
                                        actual,
                                        arg_lits.get(i).copied().flatten(),
                                    );
                                    let _ = self.table.unify(param, &actual);
                                }
                            }
                            let expanded = self.table.normalize_alias_ty(&fresh.ret);
                            let inserted = self.table.insert_type_vars(&expanded);
                            self.defer_implicit_arguments(
                                callee_id,
                                &fresh,
                                arg_types.len(),
                                locals,
                            );
                            return self.table.deep_resolve(&inserted);
                        }
                        // Every member was found and none of them accepts
                        // these arguments. Upstream says "No possible
                        // overloading for f"; this returned `error()` in
                        // silence, so `f("s")` against an overload of
                        // `int -> unit` and `bool -> unit` reported nothing
                        // — while the direct call `f1("s")` reported the
                        // mismatch, which is what made it findable.
                        //
                        // Only when every member's signature is known:
                        // `filter_overload_tree` is a coarse filter, and a
                        // member whose scheme was never built is a member
                        // that might have accepted them.
                        let all_known =
                            members.iter().all(|m| self.env.symbol_index.get(m).is_some());
                        if all_known
                            && !cross_candidates.is_empty()
                            && cross_candidates.len() >= members.len()
                            && self.env.report_unknown_calls
                            && !arg_types.iter().any(|t| t.is_error())
                        {
                            self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                                call_expr: callee_id,
                                name: callee_name.clone(),
                                no_function: false,
                                closest: None,
                            });
                        }
                    }
                }
            }
            return Ty::error();
        }

        // 6. Look up local candidates (overloads)
        let candidates = self.env.lookup_functions(&callee_name);
        if candidates.is_empty() {
            if self.env.report_unknown_calls
                && self.env.overloads.get(&callee_name).is_some_and(Vec::is_empty)
            {
                self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                    call_expr: callee_id,
                    name: callee_name,
                    no_function: false,
                    closest: None,
                });
                return Ty::error();
            }
            // No function, no mapping — genuinely undefined.
            //
            // Only report when the name is unknown *everywhere*. A name that
            // exists as a constructor or mapping but whose signature did not
            // match is a different (and much harder) diagnosis; reporting it
            // as "no matching overload" fires on valid code — existential
            // union constructors, attributed constructors, and the
            // `<mapping>_forwards` / `_backwards` accessors upstream derives
            // from each mapping.
            if self.env.report_unknown_calls
                && !is_likely_external_or_generated(&callee_name)
                && !self.name_known_in_workspace(&callee_name)
            {
                // Genuinely undefined is upstream's `Err_no_function_type`
                // — "Function {name} not found", with the nearest name it
                // does know. The comment two branches down already said so.
                let closest = self.closest_function_name(&callee_name);
                self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                    call_expr: callee_id,
                    name: callee_name,
                    no_function: true,
                    closest,
                });
            } else if self.env.report_unknown_calls
                && !self.name_is_callable_anywhere(&callee_name)
                // A register is not one of these. `sysreg.bits()` reaches
                // here as a call on `sysreg` — Sail's accessor form, lowered
                // approximately — and a register is exactly the kind of
                // value that is written as if it were callable.
                && !self.env.registers.contains_key(&callee_name)
                && self
                    .env
                    .lookup_value(locals, &callee_name)
                    .is_some_and(|ty| !ty.is_error())
            {
                // Bound, and to something that is not callable. Upstream
                // looks a callee up in the function namespace alone and
                // says "Function x not found"; this said nothing, because
                // `name_known_in_workspace` counts a `let` as known and the
                // branch above only fires for a name that denotes nothing.
                self.push_inference_diagnostic(InferenceDiagnostic::NotAFunction {
                    call_expr: callee_id,
                    name: callee_name,
                });
            }
            return Ty::error();
        }

        // Sail constructors take exactly one payload. Surface `Ctor(a, b)`
        // means `Ctor((a, b))`, and `Ctor()` supplies unit. Keep the payload
        // together for both candidate probing and confirmation, including
        // existential tuples. Merely relaxing the arity check compared the
        // entire tuple parameter with the first argument and lost the rest.
        let packed_constructor =
            self.env.constructors.contains_key(&callee_name) && arg_types.len() != 1;
        let arg_types = if packed_constructor {
            vec![if arg_types.is_empty() { Ty::named("unit") } else { Ty::tuple(arg_types) }]
        } else {
            arg_types
        };

        // A range argument denotes one runtime integer, not the type `int`.
        // Give dependent singleton parameters a rigid value with those bounds.
        // The enclosing call scope packages any value escaping in the result.
        let supplied_count = arg_types.len();
        let arg_types: Vec<Ty> = arg_types
            .into_iter()
            .enumerate()
            .map(|(index, actual)| {
                let dependent = candidates.iter().any(|c| {
                    c.params
                        .iter()
                        .zip(c.implicit_params.iter().chain(std::iter::repeat(&false)))
                        .filter(|(_, implicit)| supplied_count == c.params.len() || !**implicit)
                        .nth(index)
                        .is_some_and(|(p, _)| super::refinement::singleton(p).is_some())
                });
                if dependent {
                    return self.numeric_operand(actual, locals);
                }
                actual
            })
            .collect();

        // 7. Overload resolution: try each candidate
        //
        // so snapshot/unify/rollback flows through the same API as RA's method
        // resolution. MRC is created per-candidate (after freshen_scheme) to
        // avoid holding a &mut self.table borrow across env/scheme accesses.

        let mut assumptions = Vec::new();
        self.fill_assumptions(locals, &mut assumptions);

        let mut count_mismatch: Option<(usize, usize)> = None;
        let mut candidate_errors: Vec<(String, Span, Box<TypeError>)> = Vec::new();
        // The return type of a candidate dropped *only* for an unresolved
        // quantifier. `forall 'n, 'n >= 0. int('n) -> bool` called with a
        // plain `int` cannot pin `'n`, and the candidate goes — but the
        // return type does not mention `'n`, so it is known whatever `'n`
        // turns out to be. Without this the call came back `error()`, and
        // every check downstream of it — the kind of an operand, the type
        // of a binding — had nothing to look at.
        let mut quant_only_ret: Option<(Ty, Vec<ConstraintExpr>)> = None;
        // Collect all viable candidates for specificity ranking.
        let mut viable: Vec<(usize, Ty, Subst, Vec<ConstraintExpr>, FreshenInfo)> = Vec::new();
        // The first candidate whose return type could not be what the
        // context asked for.
        let mut ret_conflict: Option<(Ty, Ty)> = None;

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
                    if imported_constructor_collision
                        && expected_ret.as_ref().is_some_and(|expected| {
                            !self.constructor_types_plausible(expected, &c.ret)
                        })
                    {
                        return false;
                    }
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
                    params_to_check.iter().zip(arg_types.iter()).all(|(param, arg)| {
                        if imported_constructor_collision {
                            self.constructor_types_plausible(param, arg)
                        } else {
                            self.overload_types_plausible(param, arg)
                        }
                    })
                })
                .collect()
        };

        if imported_constructor_collision
            && plausible_indices.is_empty()
            && !arg_types.iter().any(Ty::is_error)
        {
            self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                call_expr: callee_id,
                name: callee_name,
                no_function: false,
                closest: None,
            });
            return Ty::error();
        }

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
            // Every *explicit* parameter being unit means `f()` is a legal
            // call: `f()` is `f(())`. Implicit parameters are supplied by the
            // checker, so `(implicit('n), implicit('m), unit) -> ..` is called
            // as `f()` too.
            let all_explicit_params_are_unit = candidate
                .params
                .iter()
                .zip(candidate.implicit_params.iter().chain(std::iter::repeat(&false)))
                .filter(|(_, is_implicit)| !**is_implicit)
                .all(|(p, _)| matches!(p.kind(), TyKind::Scalar(Scalar::Unit)));
            let unit_adjusted_required =
                if required >= 1 && all_explicit_params_are_unit { 0 } else { required };

            if std::env::var_os("SAIL_DBG_Q").is_some() {
                eprintln!(
                    "CAND {} params={:?} ret={:?} args={:?}",
                    callee_name,
                    candidate.params.iter().map(|p| format!("{:?}", p.kind())).collect::<Vec<_>>(),
                    candidate.ret.kind(),
                    arg_types.iter().map(|a| a.display_text()).collect::<Vec<_>>()
                );
            }
            // Arity check
            if arg_types.len() < unit_adjusted_required || arg_types.len() > total {
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

            // An implicit parameter given a literal: `canonical_NaN(16)`
            // against `forall 'n, 'n in {16, 32, 64, 128}. implicit('n) ->
            // bits('n)`. The argument's *type* is `int` and says nothing, so
            // `'n` stayed unbound and the candidate was thrown away for an
            // unresolved quantifier. Its value is right there in the call.
            //
            // Narrow on purpose. Typing every integer literal as its own
            // singleton draws 381 false positives — two singletons never
            // join, so `if c then 1 else 2` disagrees with itself. Nothing
            // joins here: an implicit parameter is a number the checker
            // supplies, and this is the one place the caller wrote it down.
            for (param, lit) in expected_params.iter().zip(arg_lits.iter()) {
                let (Some(lit), TyKind::App { name, args: pargs, .. }) = (lit, param.kind()) else {
                    continue;
                };
                // Also `int('n)` / `atom('n)` / `range`: a literal argument
                // says exactly what the quantifier is, and a parameter that
                // still has a variable in it is the one place that cannot
                // be mistaken. This is not the global singleton rule — the
                // literal's own type is untouched, so nothing has to join.
                if !matches!(name.as_str(), "implicit" | "int" | "atom" | "nat") || pargs.len() != 1
                {
                    continue;
                }
                let TyArg::Type(inner) = &pargs[0] else { continue };
                let resolved = self.table.resolve(inner);
                if let TyKind::Infer(iv) = resolved.kind() {
                    self.table.bind(iv.0, Ty::param(lit.to_string()));
                }
            }

            // Collect owned copies of expected params for try_candidate's &[Ty].
            let params_owned: Vec<Ty> = expected_params.iter().map(|p| (*p).clone()).collect();
            let arg_types: Vec<_> = arg_types
                .iter()
                .enumerate()
                .zip(&params_owned)
                .map(|((index, actual), expected)| {
                    if !packed_constructor
                        && matches!(
                            self.table.normalize_alias_deep(expected).kind(),
                            TyKind::Exist { .. }
                        )
                    {
                        args.get(index).map_or_else(
                            || actual.clone(),
                            |id| literal_evidence(body, *id, actual),
                        )
                    } else {
                        actual.clone()
                    }
                })
                .collect();
            let table_ok = {
                let mut mrc =
                    crate::method_resolution::MethodResolutionContext { table: &mut self.table };
                mrc.try_candidate(&params_owned, &arg_types)
            };
            // MRC dropped — self.table accessible again.

            // An enum is not in `nominal_types` — see `set_nominal_types` for
            // why a union and a bitfield are not either — so `unify` accepts
            // one where a bitvector or a number is asked for, and a candidate
            // taking `(bits(64), WordType)` matched `(width, addr)` written
            // the wrong way round. `enum_conflict` is the check that knows
            // better; it was reached only where an expected type meets an
            // expression, never at a call's arguments, which is half of what
            // the argument-order mutants swap.
            let enum_mismatch = table_ok
                .then(|| {
                    params_owned.iter().zip(arg_types.iter()).position(|(param, actual)| {
                        !param.is_error()
                            && !actual.is_error()
                            && (self.enum_conflict(param, actual)
                                || self.bitfield_conflict(param, actual))
                    })
                })
                .flatten();
            if let Some(index) = enum_mismatch {
                self.table.rollback_to(snap);
                let expected = &params_owned[index];
                let actual = &arg_types[index];
                let err_span = arg_spans.get(index).copied().unwrap_or(callee_span);
                let sig_text = format_scheme_signature(&candidate);
                candidate_errors.push((
                    callee_name.clone(),
                    err_span,
                    Box::new(TypeError::FunctionArg {
                        span: err_span,
                        ty: expected.display_text(),
                        error: Box::new(TypeError::Subtype {
                            lhs: crate::display::diagnostic_type_text(actual),
                            rhs: expected.display_text(),
                            constraint: Some(format!(
                                "in call to `{callee_name}` with signature: {sig_text}"
                            )),
                        }),
                    }),
                ));
                continue;
            }

            // The context determines quantifiers the arguments cannot.
            // `foo()` for `forall 'n. implicit('n) -> bits(8 * 'n)` has no
            // arguments at all: `'n` is fixed by the type the caller asked
            // for, and nothing else. Unifying that only *after* the
            // constraint check meant the check saw `'n` unbound, called it
            // unresolved, and threw the candidate away — so the call came
            // back `error()` and the binding that would have compared the
            // types skipped itself.
            //
            // Speculative: a failure here is not reported from this line.
            // The candidate simply carries on and fails, or does not, on
            // its own terms.
            if let Some(ref exp_ret) = expected_ret {
                if table_ok
                    && !exp_ret.is_error()
                    && !candidate.ret.is_error()
                    && !self.table.unify(exp_ret, &candidate.ret)
                {
                    // The context asked for something this candidate
                    // cannot produce. Remember it: the candidate is
                    // about to be dropped for having an unresolved
                    // quantifier — which is true, and is a consequence
                    // of this, not a separate fact — and the call would
                    // otherwise come back `error()` with nothing said.
                    ret_conflict
                        .get_or_insert((exp_ret.clone(), self.table.deep_resolve(&candidate.ret)));
                }
            }

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
                                    lhs: crate::display::diagnostic_type_text(actual),
                                    rhs: expected.display_text(),
                                    constraint: Some(format!(
                                        "in call to `{callee_name}` with signature: {sig_text}"
                                    )),
                                }),
                            }),
                        ));
                        break;
                    }
                }
                self.table.rollback_to(diag_snap);
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
            for (param, actual) in params_owned.iter().zip(arg_types.iter()) {
                self.bind_width_through_shape_alias(param, actual);
            }
            // And the same for what the context asks the call to return.
            // `F(i) = nan_box(data)` fixes `'m` from the setter's parameter
            // and from nowhere else, and that parameter is `flenbits`.
            if let Some(ref exp_ret) = expected_ret {
                let ret = candidate.ret.clone();
                self.bind_width_through_shape_alias(&ret, exp_ret);
            }
            // Keep unknown goals with the candidate until selection. The fresh
            // placeholders predate `snap`, so rollback preserves their identities.
            let mut deferred = Vec::new();
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
                &mut deferred,
            );
            // Collect viable info before rollback (rollback consumes snap).
            let ret_ty = self.table.deep_resolve(&candidate.ret);
            self.table.rollback_to(snap);
            if let Some(error) = constraint_error {
                if std::env::var("SAIL_DBG_R").is_ok() {
                    eprintln!("REJECT {callee_name} {error:?}");
                }
                // Only when the return type is fully known — no variable, no
                // quantifier left in it. A return that *does* mention the
                // unresolved quantifier is unknown, and saying otherwise
                // would be inventing a width.
                //
                // Through `insert_type_vars`, because a constraint is not a
                // quantifier: `bool('p)` is a boolean whatever `'p` is, and
                // that arm already says so. `int('n)` is not — it becomes
                // `int(?k)` and stays unknown, which is right.
                if matches!(
                    error,
                    TypeError::UnresolvedQuants { .. } | TypeError::FailedConstraint { .. }
                ) && !ret_ty.is_error()
                {
                    let normalised = self.table.insert_type_vars(&ret_ty);
                    if normalised.is_fully_known() {
                        quant_only_ret.get_or_insert((normalised, deferred));
                    }
                }
                candidate_errors.push((callee_name.clone(), callee_span, Box::new(error)));
                continue;
            }
            // Candidate is viable — collect for specificity ranking
            viable.push((ci, ret_ty, subst, deferred, freshen_info));
        }

        if std::env::var("SAIL_DBG_R").is_ok() {
            eprintln!(
                "VIABLE {callee_name} n={} rets={:?}",
                viable.len(),
                viable.iter().map(|(_, r, _, _, _)| r.display_text()).collect::<Vec<_>>()
            );
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
                    .map(|(vi, (ci, _, _, _, _))| {
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
            let (ci, ret_ty, _subst, deferred, freshening) = &viable[best];
            for goal in deferred {
                self.table.push_subtype_obligation(Obligation::NumericConstraint(goal.clone()));
            }
            // Re-apply the winning candidate's unification to the table
            let winner = &candidates[*ci];
            if let Some(name) = self.env.selected_overload_name(&callee_name, winner) {
                self.result.selected_overloads.insert(callee_id, name);
            }
            if !freshening.entries.is_empty() {
                self.pending_call_results.push(PendingCallResult {
                    callee: callee_id,
                    name: callee_name.clone(),
                    ret: ret_ty.clone(),
                    freshening: freshening.clone(),
                    signature: format_scheme_signature(winner),
                });
            }
            // Now that overload resolution has picked one, the callee has a
            // type. Written here rather than up front because an overloaded
            // name has none until the arguments decide.
            let winner_sig =
                crate::ty::FnSig { params: winner.params.clone(), ret: winner.ret.clone() };
            self.result.write_expr_ty(callee_id, Ty::new(TyKind::FnPtr(winner_sig)));
            // Probe variables were rolled back. Confirm with a fresh instance,
            // not the uninstantiated signature's universally quantified names.
            let (winner, confirmed_freshening) = self.freshen_scheme(winner);
            // Deferred goals and nested call results still reference the
            // probe's variables. Preserve that identity when confirming the
            // winner, rather than binding a disconnected second instance.
            for (probed, confirmed) in freshening.entries.iter().zip(&confirmed_freshening.entries)
            {
                debug_assert_eq!(probed.orig_name, confirmed.orig_name);
                let _ = self.table.unify(&probed.infer_ty, &confirmed.infer_ty);
            }
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
            // The context may determine an argument through the return type,
            // e.g. fill(zeros()) : vector(4, bits(8)). The resolved probe
            // result alone no longer contains the nested argument variable.
            let _ = self.table.unify(&winner.ret, ret_ty);
            if let Some(ref exp_ret) = expected_ret {
                if !ret_ty.is_error() && !exp_ret.is_error() {
                    let confirmed_ret = self.table.deep_resolve(&winner.ret);
                    let _ = self.table.unify(exp_ret, &confirmed_ret);
                }
            }
            // Resolve Bidir return types from mapping val specs.
            self.defer_implicit_arguments(callee_id, &winner, arg_types.len(), locals);
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

        // Every candidate was dropped, and one of them was dropped because
        // it cannot return what the context asked for. That is the error —
        // more useful than the unresolved quantifier it caused, and it is
        // the only thing said about a call with no arguments, where there is
        // no parameter to name.
        if let Some((expected, actual)) = ret_conflict.take() {
            if !is_partial_overload {
                self.result.record_type_mismatch(callee_id, &expected, &actual);
                return Ty::error();
            }
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
                    TypeError::FailedConstraint { constraint, derived_from, proved_false } => {
                        self.push_inference_diagnostic(
                            InferenceDiagnostic::CallConstraintViolation {
                                call_expr: callee_id,
                                constraint: constraint.clone(),
                                derived_from: derived_from.clone(),
                                proved_false: *proved_false,
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
        } else if !is_partial_overload && self.env.report_unknown_calls && candidates.is_empty() {
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
                // Zero candidates is upstream's `Err_no_function_type`, not
                // `Err_no_overloading` — "Function foo_baz not found",
                // optionally with the nearest name it does know
                // (`type_error.ml:316-322`). The two were one message here,
                // and the suggestion is most of what makes the first one
                // worth reading.
                let closest = self.closest_function_name(&callee_name);
                self.push_inference_diagnostic(InferenceDiagnostic::NoOverloading {
                    call_expr: callee_id,
                    name: callee_name,
                    no_function: true,
                    closest,
                });
            }
        }
        // See `quant_only_ret`: the quantifier is genuinely unresolved and
        // the diagnostic above says so, but the *type* of the call is not
        // unknown when the return mentions no quantifier.
        match quant_only_ret {
            Some((ret, deferred)) => {
                for goal in deferred {
                    self.table.push_subtype_obligation(Obligation::NumericConstraint(goal));
                }
                ret
            }
            None => Ty::error(),
        }
    }

    /// Two *different* bitfields.
    ///
    /// A bitfield is deliberately absent from `nominal_types`, because it
    /// converts to and from its underlying bitvector and so does accept
    /// something other than itself — see `set_nominal_types`. It does not
    /// accept a *different bitfield*, and `unify`'s permissive `Named` tail
    /// let it: the accessors a bitfield synthesises are one overload set per
    /// field name, so `update_rvfi_halt` has a candidate for each of the two
    /// bitfields that declare `rvfi_halt`, and resolution took whichever
    /// came first. That is where `rvfi_exec.bits` came back `bits(192)` for
    /// a 704-bit packet — an old note about `update_Field0(b2, ..).bits`
    /// giving the other one's width, with no check behind it until now.
    ///
    /// Only names this file's env knows to be bitfields, so a name from a
    /// file out of scope still goes through untouched.
    fn bitfield_conflict(&self, expected: &Ty, actual: &Ty) -> bool {
        let bitfield_name = |t: &Ty| match t.kind() {
            TyKind::Named(n, args) if args.is_empty() && self.env.bitfields.contains_key(n) => {
                Some(n.clone())
            }
            _ => None,
        };
        match (bitfield_name(expected), bitfield_name(actual)) {
            (Some(a), Some(b)) => a != b,
            _ => false,
        }
    }

    /// Whether one side is a known enum and the other cannot be it.
    ///
    /// Deliberately one-sided about what it knows: a name absent from
    /// `env.enums` says nothing, because the definition may be in a file
    /// this handle does not reach.
    pub(super) fn enum_conflict(&self, expected: &Ty, actual: &Ty) -> bool {
        // A *plain* enum, and `env.unions` is what says so. A union lands
        // in `env.enums` as well — `scattered union instruction` does —
        // and judging against it reported two valid sail-riscv lines where
        // a mapping had resolved in the wrong direction. A union carries a
        // payload and says nothing about the shape of a value here.
        let enum_name = |t: &Ty| match t.kind() {
            TyKind::Named(n, args)
                if args.is_empty()
                    && !self.env.unions.contains_key(n)
                    && self.env.enums.get(n).is_some_and(|m| !m.is_empty()) =>
            {
                Some(n.clone())
            }
            _ => None,
        };
        let other_is_incompatible = |t: &Ty| match t.kind() {
            // A bitvector, a tuple or a function is never an enum member.
            TyKind::App { name, .. } => matches!(
                name.as_str(),
                "bits"
                    | "bitvector"
                    | "vector"
                    | "atom"
                    | "int"
                    | "nat"
                    | "range"
                    | "implicit"
                    | "bool"
                    | "atom_bool"
            ),
            TyKind::Tuple(_) | TyKind::FnPtr(..) => true,
            TyKind::Scalar(s) => !matches!(s, Scalar::Unit),
            _ => false,
        };
        match (enum_name(expected), enum_name(actual)) {
            (Some(_), None) => other_is_incompatible(actual),
            (None, Some(_)) => other_is_incompatible(expected),
            (Some(a), Some(b)) => a != b,
            (None, None) => false,
        }
    }

    /// Distinct declared nominal types cannot select the same overload.
    /// Unresolved aliases still use the conservative shape filter.
    fn overload_types_plausible(&self, param: &Ty, arg: &Ty) -> bool {
        let normalized_param = self.table.normalize_alias_deep(param);
        let normalized_arg = self.table.normalize_alias_deep(arg);
        let nominal = |ty: &Ty| {
            ty.ctor_name()
                .filter(|name| {
                    self.env.unions.contains_key(*name)
                        || self.env.enums.contains_key(*name)
                        || self.env.records.contains_key(*name)
                        || self.env.bitfields.contains_key(*name)
                })
                .map(str::to_owned)
        };
        if let (Some(a), Some(b)) = (nominal(&normalized_param), nominal(&normalized_arg)) {
            return a == b;
        }
        types_plausibly_compatible(param, arg)
    }

    /// Constructor payloads do not coerce primitive values into declared
    /// unions/enums. Keep this stricter filter out of ordinary overloads,
    /// whose mapping/coercion paths have their own candidate handling.
    pub(super) fn constructor_types_plausible(&self, expected: &Ty, actual: &Ty) -> bool {
        let expected = self.table.normalize_alias_deep(expected);
        let actual = self.table.normalize_alias_deep(actual);
        let nominal = |ty: &Ty| {
            ty.ctor_name().is_some_and(|name| {
                self.env.unions.contains_key(name)
                    || self.env.enums.contains_key(name)
                    || self.env.records.contains_key(name)
            })
        };
        if (nominal(&expected) && type_kind(&actual).is_some())
            || (nominal(&actual) && type_kind(&expected).is_some())
        {
            return false;
        }
        self.overload_types_plausible(&expected, &actual)
    }

    /// Infer with optional expected type (bidirectional).
    /// `InferenceContext::infer_expr_coerce` pattern. After inference,
    /// if there's an expected type we check for a mismatch.
    /// Report an integer literal that cannot be in the range it is being
    /// checked against.
    ///
    /// A literal is typed `int` here, where upstream gives it the singleton
    /// `atom(3)` — doing the same draws 381 false positives on sail-riscv,
    /// because two singletons never join and `if c then 1 else 2` starts
    /// disagreeing with itself. Comparing the value needs no join, and works
    /// wherever an expected type is known: an annotation, an ascription, an
    /// assignment target, a match arm.
    /// Tighten a local's concrete range from `assert(x <op> k)`.
    fn narrow_local_from_assert(
        &mut self,
        body: &hir_def::Body,
        cond: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) {
        use hir_def::hir::{BinaryOp, CmpOp, HirBinaryOp, Ordering};
        let Some(hir_def::Expr::BinaryOp { lhs, op, rhs }) = body.expr(cond) else { return };
        let HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Ord { ordering, strict })) = op else {
            return;
        };
        let Some(hir_def::Expr::Ident(name)) = body.expr(*lhs) else { return };
        let Some(k) = integer_literal_value(body, *rhs) else { return };
        let Some(ty) = self.env.lookup_value(locals, name) else { return };
        let Some((lo, hi)) = concrete_range_bounds(&ty) else { return };
        let (lo, hi) = match (ordering, strict) {
            (Ordering::Greater, true) => (lo.max(k + 1), hi),
            (Ordering::Greater, false) => (lo.max(k), hi),
            (Ordering::Less, true) => (lo, hi.min(k - 1)),
            (Ordering::Less, false) => (lo, hi.min(k)),
        };
        if lo > hi {
            return;
        }
        let name = name.clone();
        locals.define(
            &name,
            Ty::app(
                "range",
                vec![TyArg::numeric(lo.to_string()), TyArg::numeric(hi.to_string())],
                format!("range({lo}, {hi})"),
            ),
        );
    }

    /// A constructor's existential constraint, under the witnesses its
    /// arguments just supplied.
    ///
    /// `Ctor : {'d 'n, datasize('d) & 0 <= 'n < 'd. (int('d), int('n))}`
    /// applied the wrong way round asks for `'d < 'n` where the caller's
    /// annotation says `'n < 'd`. Nothing evaluated it.
    ///
    /// Every witness must be known, and only *this* call's: reading the
    /// whole placeholder table mixes in bindings from unrelated calls and
    /// invents contradictions. Only a definite failure is reported —
    /// "cannot prove" is the solver's, and saying it costs ten wrong
    /// answers for three right ones.
    fn check_payload_constraint(&mut self, locals: &LocalEnv) {
        let Some((constraint, witnesses)) = self.pending_payload.take() else { return };
        let mut subst = Subst::default();
        for (var, placeholder) in &witnesses {
            let value =
                self.table.placeholder_value(placeholder).unwrap_or_else(|| placeholder.clone());
            subst.values.insert(var.clone(), value);
        }
        let goal = super::renormalise_constraint(super::constraint::apply_subst_constraint_expr(
            &constraint,
            &subst,
        ));
        let mut raw = Vec::new();
        self.fill_assumptions(locals, &mut raw);
        // Flattened on both sides. An assumption that arrives as one `And`
        // matches no goal and yields no fact, so `0 <= 'n & 'n < 'd` proved
        // nothing about `'n` at all.
        let mut assumptions = Vec::new();
        for a in &raw {
            flatten_conjuncts(&super::renormalise_constraint(a.clone()), &mut assumptions);
        }
        // Conjunct by conjunct: one of them being false settles the whole.
        let mut conjuncts = Vec::new();
        flatten_conjuncts(&goal, &mut conjuncts);
        let statuses: Vec<_> = conjuncts
            .iter()
            .map(|c| super::solver::solve_without_smt(c, &Subst::default(), &assumptions))
            .collect();
        let failed = statuses.contains(&ConstraintStatus::Failed);
        if !failed && statuses.contains(&ConstraintStatus::Unknown) {
            let start = self.table.obligation_count();
            self.table.push_subtype_obligation(super::Obligation::NumericConstraint(goal.clone()));
            self.scope_obligations(start, self.body.root(), locals);
        }
        if failed {
            self.push_inference_diagnostic(InferenceDiagnostic::ConstraintViolation {
                expr: self.body.root(),
                constraint: self.table.display_constraint(&goal),
                derived_from: Vec::new(),
            });
        }
    }

    /// A quantifier the body mentions must be *determined* by a named
    /// parameter.
    ///
    /// `sizeof('n)` — and a bare `'n` in an expression, which upstream
    /// desugars to the same thing — has to be rewritten to a runtime value,
    /// and the only place that value can come from is a parameter the
    /// function actually binds. `function test(_, n)` against
    /// `(implicit('n), unit)` puts `'n` behind a wildcard, and upstream says
    /// "Cannot re-write sizeof('n)". Nothing here said anything.
    ///
    /// Only for a quantifier that *some* parameter type mentions and that
    /// every such parameter binds to a wildcard. One reachable through the
    /// return type, a constraint or an enclosing scope says nothing.
    fn check_quantifiers_are_determined(&mut self, body: &hir_def::Body, param_tys: &[Ty]) {
        use hir_def::hir::Pat;
        let mut mentioned: HashSet<String> = HashSet::new();
        let mut determined: HashSet<String> = HashSet::new();
        for (pat_id, ty) in body.params.iter().zip(param_tys.iter()) {
            let mut vars = Vec::new();
            collect_type_vars(ty, &mut vars);
            let binds_a_name = !matches!(body.pat(*pat_id), Some(Pat::Wild) | None);
            for v in vars {
                mentioned.insert(v.clone());
                if binds_a_name {
                    determined.insert(v);
                }
            }
        }
        if mentioned.len() == determined.len() {
            return;
        }
        for (expr_id, expr) in body.iter_exprs() {
            let used = match expr {
                hir_def::Expr::SizeOf { nexp } => nexp.clone(),
                // `'n` on its own is `Expr::TypeVar`; upstream desugars it
                // to `sizeof('n)` and needs the same runtime value.
                hir_def::Expr::TypeVar(n) => n.clone(),
                hir_def::Expr::Ident(n) if n.starts_with('\'') => n.clone(),
                _ => continue,
            };
            for q in mentioned.difference(&determined) {
                if !used
                    .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\''))
                    .any(|w| w == q)
                {
                    continue;
                }
                self.push_inference_diagnostic(InferenceDiagnostic::ConstraintViolation {
                    expr: expr_id,
                    constraint: format!("{q} is not determined by any parameter"),
                    derived_from: Vec::new(),
                });
                break;
            }
        }
    }

    fn immutable_value_identity(
        &self,
        body: &hir_def::Body,
        pat: hir_def::PatId,
        value: hir_def::ExprId,
        ty: Ty,
        locals: &LocalEnv,
    ) -> Ty {
        if let Some((var, predicate, inner)) = super::refinement::numeric_value(&ty) {
            if matches!(body.expr(value), Some(hir_def::Expr::Ident(_)))
                && !matches!(body.pat(pat), Some(hir_def::Pat::TypeVar(_)))
            {
                return Ty::exist(vec![var], predicate, inner);
            }
        }
        if constraint::is_numeric_scalar_ty(&ty)
            && !matches!(body.pat(pat), Some(hir_def::Pat::Typed { .. }))
        {
            if let Some(n) =
                super::refinement::number(body, value, &|n| self.refinement_value(locals, n))
            {
                let n = super::substitution::numeric(&n, &locals.type_substitution());
                return Ty::app("atom", vec![TyArg::Nexp(n)], "");
            }
        }
        if matches!(ty.kind(), TyKind::Scalar(Scalar::Int)) {
            let name = "'_value".to_owned();
            return Ty::exist(
                vec![name.clone()],
                ConstraintExpr::Bool(true),
                Ty::app("atom", vec![TyArg::Nexp(NumericExpr::Var(name))], ""),
            );
        }
        self.immutable_boolean_identity(ty)
    }

    fn numeric_operand(&mut self, ty: Ty, locals: &mut LocalEnv) -> Ty {
        let ty = self.table.deep_resolve(&ty);
        let ty = self.table.normalize_alias_ty(&ty);
        if let TyKind::Exist { vars, constraint, inner } = ty.kind() {
            if constraint::is_numeric_scalar_ty(inner) {
                let opened = self.table.open_existential(vars, constraint, inner);
                locals.add_constraint(opened.constraint);
                return opened.inner_ty;
            }
        }
        if matches!(ty.kind(), TyKind::Scalar(Scalar::Int)) {
            let var = "'_argument".to_owned();
            let opened = self.table.open_existential(
                &[var.clone()],
                &ConstraintExpr::Bool(true),
                &Ty::app("atom", vec![TyArg::Nexp(NumericExpr::Var(var))], ""),
            );
            return opened.inner_ty;
        }
        if let Some((var, predicate, inner)) = super::refinement::numeric_value(&ty) {
            let opened = self.table.open_existential(&[var], &predicate, &inner);
            locals.add_constraint(opened.constraint);
            return opened.inner_ty;
        }
        ty
    }

    /// One evaluation of a coarse Boolean has one unknown runtime value.
    /// Introduce it before freshening a callee so flexible indices may refer
    /// to it, while unrelated evaluations remain distinct rigid values.
    fn boolean_operand(&mut self, ty: Ty, locals: &mut LocalEnv) -> Ty {
        let ty = self.table.deep_resolve(&ty);
        let ty = self.table.normalize_alias_ty(&ty);
        let ty = self.immutable_boolean_identity(ty);
        if let TyKind::Exist { vars, constraint, inner } = ty.kind() {
            if super::subtype::bool_value(inner).is_some() {
                let opened = self.table.open_existential(vars, constraint, inner);
                locals.add_constraint(opened.constraint);
                return opened.inner_ty;
            }
        }
        ty
    }

    fn boolean_guard(
        &self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        locals: &LocalEnv,
    ) -> Option<ConstraintExpr> {
        self.result
            .expr_ty(id)
            .and_then(super::subtype::bool_value)
            .or_else(|| super::refinement::guard(body, id, &|n| self.refinement_value(locals, n)))
    }

    pub(super) fn immutable_boolean_identity(&self, ty: Ty) -> Ty {
        if let TyKind::Tuple(items) = ty.kind() {
            return Ty::tuple(
                items.iter().map(|t| self.immutable_boolean_identity(t.clone())).collect(),
            );
        }
        // Legacy generic payload substitutions can carry the concrete type
        // name in Param. It still denotes bool, not a quantified value.
        if !matches!(ty.kind(), TyKind::Scalar(Scalar::Bool))
            && !matches!(ty.kind(), TyKind::Param(name) if name == "bool")
        {
            return ty;
        }
        let name = "'_flow_bool".to_string();
        Ty::exist(
            vec![name.clone()],
            ConstraintExpr::Bool(true),
            Ty::app("bool", vec![TyArg::Bool(ConstraintExpr::BoolVar(name))], ""),
        )
    }

    fn check_literal_against_range(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expected_ty: &Ty,
    ) -> bool {
        let Some(n) = super::refinement::number(body, id, &|_| None)
            .and_then(|n| super::integer::eval(&n, &Subst::default(), &[]))
        else {
            return false;
        };
        let expected = self.table.normalize_alias_deep(expected_ty);
        let TyKind::App { name, args, .. } = expected.kind() else { return false };
        if !matches!(name.as_str(), "range" | "atom" | "int") || args.is_empty() {
            return false;
        }
        let eval = |a: &TyArg| {
            super::integer::eval(&NumericExpr::parse(&a.as_value_str()?)?, &Subst::default(), &[])
        };
        let (Some(lo), Some(hi)) = (eval(&args[0]), eval(args.last().unwrap())) else {
            return false;
        };
        if n >= lo && n <= hi {
            return false;
        }
        self.result.record_type_mismatch(
            id,
            expected_ty,
            &Ty::app("atom", vec![TyArg::numeric(n.to_string())], format!("atom({n})")),
        );
        true
    }

    fn infer_expr_hir_with(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expected: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        let start = self.table.obligation_count();
        let ty = self.infer_expr_hir_with_inner(body, id, expected, locals);
        self.scope_obligations(start, id, locals);
        ty
    }

    fn infer_expr_hir_with_inner(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expected: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        // The expectation flows *into* inference as well as being checked
        // afterwards: a call in tail position needs it to resolve its
        // quantifiers, which it cannot do once the type is already built.
        let unreachable = locals.is_contradictory();
        let ty = self.infer_expr_expecting(
            body,
            id,
            if unreachable { &Expectation::None } else { expected },
            locals,
        );

        // Write inferred type to InferenceResult.
        self.result.write_expr_ty(id, ty.clone());

        if let Expectation::HasType(ref expected_ty) = expected {
            if !ty.is_error() && !expected_ty.is_error() {
                if locals.is_contradictory() {
                    return expected_ty.clone();
                }
                // An integer literal is typed `int`, which no range can
                // disagree with. Upstream gives it the singleton `atom(3)`
                // and the range check follows from that; doing the same here
                // draws 381 false positives on sail-riscv, because two
                // singletons never join — `if c then 1 else 2` makes
                // `atom(1)` and `atom(2)` disagree where upstream
                // generalises. Comparing the literal's own value against a
                // concrete annotation needs no join and no `atom` anywhere.
                if self.check_literal_against_range(body, id, expected_ty) {
                    return ty;
                }
                // Literals have a coarse `int` inference type. Their value
                // supplies the proof required by a `nat` expectation.
                if matches!(expected_ty.kind(), TyKind::Scalar(crate::ty::Scalar::Nat))
                    && integer_literal_value(body, id).is_some_and(|n| n >= 0)
                {
                    return expected_ty.clone();
                }
                if numeric_evidence_target(&self.table.normalize_alias_deep(expected_ty)) {
                    let evidence = literal_evidence(body, id, &ty);
                    if evidence != ty {
                        if !self.table.unify(expected_ty, &evidence) {
                            self.result.record_type_mismatch(id, expected_ty, &evidence);
                        }
                        return expected_ty.clone();
                    }
                }
                // Use InferenceTable::unify for type checking
                if std::env::var_os("SAIL_DBG_UNIFY").is_some() {
                    eprintln!(
                        "CHK exp={} act={} known={}",
                        expected_ty.display_text(),
                        ty.display_text(),
                        ty.is_fully_known()
                    );
                }
                if !self.table.unify(expected_ty, &ty) {
                    self.result.record_type_mismatch(id, expected_ty, &ty);
                    // Reported, so the expression stands in as the type it
                    // was asked for. rust-analyzer's `infer_expr_coerce`
                    // yields the coercion *target*, and the reason is the
                    // same one: an enclosing check that sees the rejected
                    // type reports it again, one node wider and less
                    // precisely. `if x < 32 then x + 32 else x - 32` said
                    // `expected range(0, 62), found range(0, 63)` against
                    // the whole `if` where upstream says `x + 32`.
                    return expected_ty.clone();
                } else if self.enum_conflict(expected_ty, &ty) {
                    // `unify` lets a non-primitive `Named` accept anything —
                    // its tail returns true whenever either side is not a
                    // primitive (unify.rs, `!is_expected_primitive ||
                    // !is_actual_primitive`). That is deliberate: a `Named`
                    // is also how a *type alias* arrives, and `xlenbits`
                    // has to accept `bits(64)`.
                    //
                    // An enum is not an alias. `let y : E = 0x0` and
                    // `tr(A, 0x0)` against `(bits(4), E)` both went
                    // unreported, and the second is why an `argorder` swap
                    // involving an enum member survives. Only a name this
                    // file's env knows to be an enum is judged here, so an
                    // alias — or a name from a file out of scope — still
                    // goes through untouched.
                    self.result.record_type_mismatch(id, expected_ty, &ty);
                    return expected_ty.clone();
                }
            }
        }
        ty
    }

    /// An implicit declaration evaluates its initializer before introducing any
    /// of its names. The scope projection decides whether it is a declaration
    /// or a write to an existing local/global, including tuple targets.
    fn infer_implicit_assignment(
        &mut self,
        body: &hir_def::Body,
        target: hir_def::ExprId,
        value: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) -> bool {
        fn declares(
            scopes: &hir_def::expr_store::scope::ExprScopes,
            body: &hir_def::Body,
            id: hir_def::ExprId,
        ) -> bool {
            match body.expr(id) {
                Some(hir_def::Expr::Ident(_)) => scopes.implicit_binding(id).is_some(),
                Some(hir_def::Expr::Cast { expr, .. }) => declares(scopes, body, *expr),
                Some(hir_def::Expr::Tuple(items)) => {
                    items.iter().any(|id| declares(scopes, body, *id))
                }
                _ => false,
            }
        }
        if !declares(&self.expr_scopes, body, target) {
            return false;
        }

        fn annotation(body: &hir_def::Body, id: hir_def::ExprId, locals: &LocalEnv) -> Option<Ty> {
            match body.expr(id)? {
                hir_def::Expr::Cast { ty, .. } => Some(constraint::apply_subst(
                    &type_from_type_text(ty),
                    &locals.type_substitution(),
                )),
                hir_def::Expr::Tuple(items) => items
                    .iter()
                    .map(|id| annotation(body, *id, locals))
                    .collect::<Option<Vec<_>>>()
                    .map(Ty::tuple),
                _ => None,
            }
        }
        let expected = annotation(body, target, locals);
        let inferred = self.infer_expr_hir_with(
            body,
            value,
            &expected.clone().map_or(Expectation::None, Expectation::HasType),
            locals,
        );
        let ty = expected
            .unwrap_or_else(|| literal_evidence(body, value, &self.table.deep_resolve(&inferred)));
        self.bind_assignment_target(body, target, &ty, locals);
        true
    }

    fn bind_assignment_target(
        &mut self,
        body: &hir_def::Body,
        target: hir_def::ExprId,
        ty: &Ty,
        locals: &mut LocalEnv,
    ) {
        match body.expr(target) {
            Some(hir_def::Expr::Ident(_)) => {
                if let Some(pat) = self.expr_scopes.implicit_binding(target) {
                    self.bind_mutable_pattern_hir(body, pat, ty, locals);
                } else {
                    // A mixed existing/new tuple is diagnosed by assignment
                    // validation, but still check the existing component.
                    let expected = self.infer_expr_hir(body, target, locals);
                    if !self.table.unify(&expected, ty) {
                        self.result.record_type_mismatch(target, &expected, ty);
                    }
                }
            }
            Some(hir_def::Expr::Cast { expr, ty: annotation }) => {
                let expected = constraint::apply_subst(
                    &type_from_type_text(annotation),
                    &locals.type_substitution(),
                );
                if !self.table.unify(&expected, ty) {
                    self.result.record_type_mismatch(target, &expected, ty);
                }
                self.bind_assignment_target(body, *expr, &expected, locals);
            }
            Some(hir_def::Expr::Tuple(items)) => {
                let normalized = self.table.normalize_alias_deep(ty);
                for (i, &item) in items.iter().enumerate() {
                    let part = match normalized.kind() {
                        TyKind::Tuple(parts) => parts.get(i).cloned().unwrap_or_else(Ty::error),
                        _ => Ty::error(),
                    };
                    self.bind_assignment_target(body, item, &part, locals);
                }
            }
            _ => {}
        }
        self.result.write_expr_ty(target, ty.clone());
    }

    /// Check and bind either syntactic form of an explicit mutable local.
    fn infer_mutable_binding(
        &mut self,
        body: &hir_def::Body,
        pat: hir_def::PatId,
        value: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) {
        let mut binding = pat;
        while let Some(hir_def::Pat::Typed { inner, .. }) = body.pat(binding) {
            binding = *inner;
        }
        if let Some(hir_def::Pat::Bind(name)) = body.pat(binding) {
            if locals.lookup(name).is_some() {
                self.push_inference_diagnostic(InferenceDiagnostic::InvalidTypeUse {
                    node: hir_def::ExprOrPatId::PatId(pat),
                    message:
                        "var expression can only be used to declare new variables, not update them"
                            .into(),
                });
            }
        }
        // A `var` is annotated as often as a `let`, and
        // for the same reason: `var ys : bits('n) =
        // zeros()` is the only place the width is
        // written. Reading the annotation for `let` and
        // not for `var` left `zeros()` with nothing to
        // fill its implicit from and `undefined` with no
        // type at all — 8 of sail-riscv's 22 remaining
        // untyped expressions were this one line shape.
        let annotated_ty = match body.pat(pat) {
            Some(hir_def::Pat::Typed { ty: ty_text, .. }) => {
                let ty = constraint::apply_subst(
                    &type_from_type_text(ty_text),
                    &locals.type_substitution(),
                );
                (!ty.is_error()).then_some(ty)
            }
            _ => None,
        };
        let val_ty = match annotated_ty {
            Some(ref ann_ty) => {
                self.infer_expr_hir_with(body, value, &Expectation::HasType(ann_ty.clone()), locals)
            }
            None => {
                if matches!(body.expr(value), Some(hir_def::Expr::Struct { name: None, .. })) {
                    self.invalid_expression(value, "Cannot infer type of struct literal");
                }
                self.infer_expr_hir(body, value, locals)
            }
        };
        let val_ty = annotated_ty.unwrap_or_else(|| self.table.deep_resolve(&val_ty));
        self.bind_mutable_pattern_hir(body, pat, &val_ty, locals);
        self.check_pattern_duplicates(body, pat);
    }

    /// Infer an expression with no expectation from its context.
    pub(super) fn infer_expr_hir(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        locals: &mut LocalEnv,
    ) -> Ty {
        self.infer_expr_expecting(body, id, &Expectation::None, locals)
    }

    /// Infer an expression against what its context expects of it.
    ///
    /// The expectation is a parameter rather than a field on `LocalEnv`,
    /// following rust-analyzer (`hir-ty/src/infer.rs`, `Expectation`). As an
    /// ambient field it reached expressions it had no business reaching —
    /// the operands of `@`, the initializer of an unannotated `let` — and
    /// each leak had to be plugged by hand with a save/restore.
    pub(super) fn infer_expr_expecting(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expectation: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        // Fuel guard: prevent stack overflow on deeply recursive expressions.
        self.inference_fuel = self.inference_fuel.saturating_sub(1);
        if self.inference_fuel == 0 {
            return Ty::error();
        }
        if self.inference_fuel % 256 == 0 {
            if let Some(db) = self.db {
                db.unwind_if_revision_cancelled();
            }
        }
        // Cooperative cancellation (checked each fuel decrement).
        if self.cancel.is_cancelled() {
            return Ty::error();
        }
        let start = self.table.obligation_count();
        let universe = self.table.universe();
        let ty = self.infer_expr_kind(body, id, expectation, locals);
        let ty = if matches!(ty.kind(), TyKind::Scalar(Scalar::Bool)) {
            super::refinement::guard(body, id, &|n| self.refinement_value(locals, n))
                .map(|c| Ty::app("bool", vec![TyArg::Bool(c)], ""))
                .unwrap_or(ty)
        } else {
            ty
        };

        // A compound expression can introduce runtime witnesses while
        // evaluating its operands. Package them before leaving its universe;
        // a caller can then open the result in its own scope. Returning free
        // younger witnesses made `negate(get() | get())` impossible to bind.
        let ty = self.table.close_universe(&ty, universe);
        self.scope_obligations(start, id, locals);
        self.table.restore_universe(universe);
        // Record every expression's type here, where nothing can bypass it.
        // The arms below return early in a dozen places — a resolved field, a
        // vector builtin, `__FILE__` — and each of those used to skip the
        // write at the end of the match, leaving the expression with no type
        // at all. `analysis_stats` counted 880 of them on sail-riscv.
        self.result.write_expr_ty(id, ty.clone());
        ty
    }

    fn infer_expr_kind(
        &mut self,
        body: &hir_def::Body,
        id: hir_def::ExprId,
        expectation: &Expectation,
        locals: &mut LocalEnv,
    ) -> Ty {
        use hir_def::Expr;

        let expr_span = self.expr_span(body, id).unwrap_or(Span::new(0, 0));
        let Some(hir) = body.expr(id) else {
            return Ty::error();
        };

        let ty = match hir {
            // A hole in the tree is "not known", not "wrong". A fresh
            // variable unifies with whatever the context needs; `error()`
            // spreads to everything that reads it, which is why an
            // incomplete expression used to untype the statements around it.
            Expr::Missing | Expr::Error { .. } => self.table.new_type_var(),

            // `undefined` is whatever the context asked for. Upstream
            // checks `L_undef` against the expected type rather than
            // inferring one (`check_exp`, type_check.ml), and there is
            // nothing to infer: the literal names a value of *some* type
            // and says nothing about which. Without this, `var mask :
            // bits('n) = undefined` left the initialiser untyped — seven of
            // sail-riscv's remaining untyped expressions, all one line.
            Expr::Literal(hir_def::hir::Literal::Undefined) => match expectation.to_option() {
                Some(expected) if !expected.is_error() => expected,
                _ => Ty::error(),
            },

            Expr::Literal(lit) => infer_literal_type(lit),

            Expr::Ident(name) => {
                self.used_bindings.insert(name.clone());

                // Upstream rewrites these three to literals while building the
                // AST (initial_check.ml:1417), so they never reach the type
                // checker as identifiers at all.
                match name.as_str() {
                    "__LOC__" | "__FILE__" => return Ty::named("string".to_string()),
                    "__LINE__" => return Ty::named("int".to_string()),
                    _ => {}
                }

                // Resolution order: locals → top-level env → ExprScopes →
                // pattern_constants → cross-file → unresolved.

                if let Some(ty) = self.env.lookup_value(locals, name) {
                    if let Some(hidden) = self.unavailable_nominal_type(std::iter::once(&ty)) {
                        self.invalid_expression(id, format!("Well-formedness check failed: type {hidden} in this value is not visible in the current module"));
                    }
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
                //
                // There used to be a sixth: "the name is bound by *some*
                // pattern somewhere in this body". It was permissiveness, not
                // an answer — it accepted a name the scope tree could not
                // place, anywhere in the body — and it is gone. What it was
                // covering for was `foreach (i from 0 to 32 by 1 in inc)`,
                // where the parser ended the loop at the `in` and left the
                // body outside it; with the header parsed as upstream's
                // `loop_exp` has it, `i` resolves in the loop's own scope and
                // removing the fallback costs nothing on any of the four
                // probes.
                else if self.env.top_level_symbol_exists(name)
                    || self.expr_scopes.scope_for(id).is_some_and(|scope_id| {
                        self.expr_scopes
                            .resolve_name_in_scope(scope_id, &hir_def::name::Name::new(name))
                            .is_some()
                    })
                    || self.pattern_constants.contains(name.as_str())
                {
                    self.report_untyped_ident(id, name);
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
                let subst = locals.type_substitution();
                if let Some(value) = subst.values.get(name) {
                    if let Some(value) = NumericExpr::parse(value) {
                        return Ty::app("atom", vec![TyArg::Nexp(value)], "");
                    }
                }
                // A type variable in expression position is the *singleton*
                // `atom('m)`, not `int`: its value is exactly that variable,
                // which is how upstream types a kid in expression position.
                // Plain `int` names no variable, so `canonical_NaN('m)`
                // against `forall 'n, 'n in {16, 32, 64, 128}. implicit('n)
                // -> bits('n)` gave the checker nothing to bind `'n` to.
                //
                // This is the one safe case of the singleton idea. A
                // *literal* typed as its own singleton draws 381 false
                // positives — two singletons never join, so `if c then 1
                // else 2` disagrees with itself. A type variable has no such
                // problem: there is one value and it already has a name.
                Ty::app(
                    "atom",
                    vec![crate::ty::TyArg::Nexp(crate::ty::NumericExpr::Var(name.clone()))],
                    format!("int({name})"),
                )
            }

            Expr::BinaryOp { lhs, op, rhs } => {
                // An operand's type is not the expression's type, so the
                // expectation does not reach it: `let x : bits(5) = zeros() @
                // 0b1` must not offer `bits(5)` to `zeros : forall 'n.
                // implicit('n) -> bits('n)`, which would pin `'n = 5` and
                // type the concatenation as `bits(6)`.
                use hir_def::hir::{BinaryOp, CmpOp, HirBinaryOp};
                let equality = matches!(op, HirBinaryOp::Known(BinaryOp::CmpOp(CmpOp::Eq { .. })));
                let empty_list =
                    |id| matches!(body.expr(id), Some(Expr::List(xs)) if xs.is_empty());
                let (lhs_ty, rhs_ty) = if equality && empty_list(*lhs) {
                    let rhs_ty = self.infer_expr_hir(body, *rhs, locals);
                    let lhs_ty = self.infer_expr_hir_with(
                        body,
                        *lhs,
                        &Expectation::HasType(rhs_ty.clone()),
                        locals,
                    );
                    (lhs_ty, rhs_ty)
                } else {
                    let lhs_ty = self.infer_expr_hir(body, *lhs, locals);
                    let same_bit_width = matches!(op.as_str(), "&" | "|" | "^")
                        && self.table.normalize_alias_deep(&lhs_ty).is_bits();
                    let rhs_ty = if (equality && empty_list(*rhs)) || same_bit_width {
                        self.infer_expr_hir_with(
                            body,
                            *rhs,
                            &Expectation::HasType(lhs_ty.clone()),
                            locals,
                        )
                    } else {
                        self.infer_expr_hir(body, *rhs, locals)
                    };
                    (lhs_ty, rhs_ty)
                };
                // `2 ^ e` is not an infix operator at all. Upstream's lexer
                // matches `"2" wsc* "^"` as a single `TwoCaret` token
                // (lexer.mll:201) that the grammar turns into the prefix
                // operator `pow2` (parser.mly:337, 708) — which is why
                // lib/vector.sail warns that `overload operator ^` "won't
                // work for lengths ending in 2". sail-lsp lexes the two
                // tokens separately, so recognise the shape here instead:
                // otherwise `2 ^ n` resolves against the `^` overload group
                // and types as a bitvector.
                if matches!(
                    op,
                    HirBinaryOp::Known(BinaryOp::ArithOp(hir_def::hir::ArithOp::BitXor))
                ) && matches!(
                    body.expr(*lhs),
                    Some(Expr::Literal(hir_def::hir::Literal::Number(n))) if n == "2"
                ) {
                    return super::bounded_arithmetic::infer(
                        &hir_def::hir::ArithOp::BitXor,
                        &literal_evidence(body, *lhs, &lhs_ty),
                        &self.table.normalize_alias_deep(&rhs_ty),
                        &locals.constraints,
                    )
                    .unwrap_or_else(|| Ty::named("int"));
                }
                match op {
                    HirBinaryOp::Known(BinaryOp::CmpOp(_)) => {
                        // Equality couples bitvector widths in both operand
                        // orders, including a width still inferred by a nested
                        // call. Merely comparing two literal widths left
                        // `x == zeros()`'s result variable unconstrained.
                        let left = self.table.normalize_alias_deep_for_shape(&lhs_ty);
                        let right = self.table.normalize_alias_deep_for_shape(&rhs_ty);
                        let same_union = left.ctor_name().is_some_and(|name| {
                            right.ctor_name() == Some(name) && self.env.unions.contains_key(name)
                        });
                        if equality && same_union {
                            // Generic equality is a library overload. When
                            // available, its shared parameter instantiates a
                            // nullary constructor such as None() from the
                            // other operand, in either order.
                            self.resolve_overloaded_binop(op.as_str(), &left, &right);
                        }
                        let shapes_checked = equality && left.is_bits() && right.is_bits();
                        if shapes_checked && !self.table.unify(&left, &right) {
                            self.result.record_type_mismatch(*rhs, &lhs_ty, &rhs_ty);
                        }
                        // A comparison's operands were never compared to each
                        // other — the result is `bool` whatever they are.
                        // Upstream reaches `==` through
                        // `eq_bits : forall 'n. (bits('n), bits('n)) -> bool`,
                        // so the two widths have to agree. Only two concrete
                        // widths are decided here; anything with a quantifier
                        // in it is the solver's business.
                        // ...and only when neither width came out of a call.
                        // Overload resolution can pick the wrong candidate —
                        // two bitfields sharing a field name give
                        // `update_Field0(b2, ..).bits` the other one's width —
                        // and a comparison is not the place to find that out.
                        // Same conservatism as the cross-file schemes: bind
                        // from them, do not accuse with them.
                        let from_a_call = |e: hir_def::ExprId| {
                            let mut cur = e;
                            loop {
                                match body.expr(cur) {
                                    Some(Expr::Call { .. }) => return true,
                                    Some(Expr::Field { expr, .. }) => cur = *expr,
                                    _ => return false,
                                }
                            }
                        };
                        if let (Some(l), Some(r)) =
                            (concrete_bits_width(&lhs_ty), concrete_bits_width(&rhs_ty))
                        {
                            if !shapes_checked && l != r && !from_a_call(*lhs) && !from_a_call(*rhs)
                            {
                                self.result.record_type_mismatch(*rhs, &lhs_ty, &rhs_ty);
                            }
                        }
                        // Operands from *disjoint* kinds have no overload to
                        // reach: upstream answers `y == 32` with `y : bool`
                        // by saying there is no possible overloading for
                        // `==`, and this said nothing — which is why a `val`
                        // whose return type is changed from `int` to `bool`
                        // survives, the change showing only at a use.
                        //
                        // By kind, not by name: `nat` and `int` are both
                        // numeric and compare fine, and anything with a
                        // quantifier or a variable in it is the solver's.
                        // A primitive is `TyKind::Scalar`, not
                        // `TyKind::Named` — matching only the latter is why
                        // an earlier attempt never fired.
                        let class = |t: &Ty, e: hir_def::ExprId| operand_kind(body, t, e);
                        if let (Some(a), Some(b)) = (class(&lhs_ty, *lhs), class(&rhs_ty, *rhs)) {
                            if a != b {
                                self.result.record_type_mismatch(*rhs, &lhs_ty, &rhs_ty);
                            }
                        }
                        if let (Some(a), Some(b)) = (
                            super::subtype::bool_value(&lhs_ty),
                            super::subtype::bool_value(&rhs_ty),
                        ) {
                            if matches!(op.as_str(), "==" | "!=") {
                                let equal = ConstraintExpr::And(vec![
                                    ConstraintExpr::Or(vec![
                                        ConstraintExpr::Not(Box::new(a.clone())),
                                        b.clone(),
                                    ]),
                                    ConstraintExpr::Or(vec![ConstraintExpr::Not(Box::new(b)), a]),
                                ]);
                                return Ty::app(
                                    "bool",
                                    vec![TyArg::Bool(if op.as_str() == "!=" {
                                        negate_constraint(equal)
                                    } else {
                                        equal
                                    })],
                                    "",
                                );
                            }
                        }
                        Ty::named("bool".to_string())
                    }
                    HirBinaryOp::Known(BinaryOp::LogicOp(logic_op)) => {
                        let lhs_ty = self.boolean_operand(lhs_ty, locals);
                        let rhs_ty = self.boolean_operand(rhs_ty, locals);
                        let op_str = match logic_op {
                            hir_def::hir::LogicOp::Or => "|",
                            hir_def::hir::LogicOp::And => "&",
                        };
                        if let Some(ret_ty) = self.resolve_overloaded_binop(
                            op_str,
                            &literal_evidence(body, *lhs, &lhs_ty),
                            &literal_evidence(body, *rhs, &rhs_ty),
                        ) {
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
                            // Not knowing an operand is not the same as it
                            // being a `bool`. `|` and `&` are `or_bool` on
                            // booleans and `or_vec` on bitvectors — nothing
                            // above chose, so with no known operand there is
                            // no answer, and saying `bool` is a claim the
                            // callers downstream then check against.
                            // `test/c/pointer_assign.sail`: both operands of
                            // `c | STATUS_1` are top-level `let`s with no
                            // annotation, so both arrive as `error()`, and
                            // the resulting `bool` was reported against the
                            // assignment target's `bits(32)` on a file
                            // upstream accepts.
                            if lhs_ty.is_error() && rhs_ty.is_error() {
                                Ty::error()
                            } else if let (Some(a), Some(b)) = (
                                super::subtype::bool_value(&lhs_ty),
                                super::subtype::bool_value(&rhs_ty),
                            ) {
                                let c = if op_str == "&" {
                                    ConstraintExpr::And(vec![a, b])
                                } else {
                                    ConstraintExpr::Or(vec![a, b])
                                };
                                Ty::app("bool", vec![TyArg::Bool(c)], "")
                            } else {
                                bool_ty
                            }
                        }
                    }
                    HirBinaryOp::Known(BinaryOp::Concat) => {
                        // `@` is `append`, and every overload of it takes
                        // two bitvectors (or two lists, which have no kind
                        // here and so say nothing). A number or a boolean on
                        // either side has no overloading to reach at all —
                        // upstream: "No possible overloading for append" —
                        // and nothing said so, which is how a swapped
                        // `(implicit('N), bits(1))` pair survived.
                        for (ty, e) in [(&lhs_ty, *lhs), (&rhs_ty, *rhs)] {
                            let Some(kind) = type_kind(ty) else { continue };
                            if kind != 4 {
                                let bits = Ty::app(
                                    "bits",
                                    vec![TyArg::Type(self.table.new_type_var())],
                                    "bits(_)".to_string(),
                                );
                                self.result.record_type_mismatch(e, &bits, ty);
                            }
                        }
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
                        // The same kind check the comparison arm makes, for
                        // the same reason: upstream reaches `*` through
                        // `overload operator * = {mult_atom, mult_int, ..}`
                        // and none of those takes a `bool` or a `string`, so
                        // `4 * pow2(..)` where `pow2` is declared to return
                        // `bool` has no possible overloading. That is what a
                        // changed return type looks like at a *use*, which is
                        // the only place it can be seen, and nothing said it.
                        //
                        // Only `+ - * / %`. `& | ^` are bitwise on a
                        // bitvector and `^` is `string_append` on a string,
                        // so for those a mixed pair is not by itself wrong.
                        if matches!(
                            arith_op,
                            hir_def::hir::ArithOp::Add
                                | hir_def::hir::ArithOp::Sub
                                | hir_def::hir::ArithOp::Mul
                                | hir_def::hir::ArithOp::Div
                                | hir_def::hir::ArithOp::Rem
                        ) {
                            if let (Some(a), Some(b)) = (
                                operand_kind(body, &lhs_ty, *lhs),
                                operand_kind(body, &rhs_ty, *rhs),
                            ) {
                                if a != b {
                                    self.result.record_type_mismatch(*rhs, &lhs_ty, &rhs_ty);
                                }
                            }
                        }
                        let lhs_evidence = literal_evidence(body, *lhs, &lhs_ty);
                        let rhs_evidence = literal_evidence(body, *rhs, &rhs_ty);
                        if matches!(
                            arith_op,
                            hir_def::hir::ArithOp::Add
                                | hir_def::hir::ArithOp::Sub
                                | hir_def::hir::ArithOp::Mul
                        ) {
                            if let Some(ty) = super::bounded_arithmetic::infer(
                                arith_op,
                                &self.table.normalize_alias_deep(&lhs_evidence),
                                &self.table.normalize_alias_deep(&rhs_evidence),
                                &locals.constraints,
                            ) {
                                return ty;
                            }
                        }
                        // Preserve nominal operand types for overload selection.
                        if let Some(ret_ty) =
                            self.resolve_overloaded_binop(op_str, &lhs_evidence, &rhs_evidence)
                        {
                            ret_ty
                        } else if lhs_ty.is_bits_like() || rhs_ty.is_bits_like() {
                            // Bitwise operation on bitvectors.
                            if !lhs_ty.is_error() {
                                lhs_ty
                            } else {
                                rhs_ty
                            }
                        } else if constraint::is_numeric_scalar_ty(&lhs_ty)
                            && constraint::is_numeric_scalar_ty(&rhs_ty)
                        {
                            // A missing overload must not turn `n + 3` into
                            // the singleton type of `n`. Retain the operation
                            // when its operands have numeric identities.
                            super::refinement::number(body, id, &|n| {
                                self.refinement_value(locals, n)
                            })
                            .map(|n| Ty::app("atom", vec![TyArg::Nexp(n)], ""))
                            .unwrap_or_else(|| Ty::named("int"))
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
                        if let Some(ret_ty) = self.resolve_overloaded_binop(
                            op_str,
                            &literal_evidence(body, *lhs, &lhs_ty),
                            &literal_evidence(body, *rhs, &rhs_ty),
                        ) {
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
                use hir_def::hir::{HirUnaryOp, UnaryOp};
                let inner_ty = if matches!(op, HirUnaryOp::Known(UnaryOp::Not))
                    && expectation
                        .to_option()
                        .is_some_and(|t| self.table.normalize_alias_deep(&t).is_bits())
                {
                    self.infer_expr_hir_with(body, *expr, expectation, locals)
                } else {
                    self.infer_expr_hir(body, *expr, locals)
                };
                match op {
                    HirUnaryOp::Known(UnaryOp::Not) => {
                        // ~ is logical NOT on bool, bitwise NOT on bits.
                        if inner_ty.is_bits_like() {
                            inner_ty
                        } else {
                            let operand = self.boolean_operand(inner_ty, locals);
                            super::subtype::bool_value(&operand)
                                .map(|c| {
                                    Ty::app(
                                        "bool",
                                        vec![TyArg::Bool(ConstraintExpr::Not(Box::new(c)))],
                                        "",
                                    )
                                })
                                .unwrap_or_else(|| Ty::named("bool"))
                        }
                    }
                    HirUnaryOp::Known(UnaryOp::Neg) => inner_ty,
                    // `*r` reads through a register reference: `register(T)`
                    // dereferences to `T`. It typed as `error()`, so an
                    // assignment through one — `(*r) = 10` — had no target
                    // type to check anything against.
                    HirUnaryOp::Custom(op) if op == "*" => {
                        let shape = self.table.normalize_alias_deep_for_shape(&inner_ty);
                        if self.in_assign_target
                            && shape.ctor_name() != Some("register")
                            && self.known_type_shape(&shape)
                        {
                            self.invalid_expression(id, format!("Cannot dereference a value without register reference type, found {}", shape.display_text()));
                        }
                        match shape.kind() {
                            TyKind::App { name, args, .. } | TyKind::Named(name, args)
                                if name == "register" =>
                            {
                                match args.first() {
                                    Some(TyArg::Type(inner)) => inner.clone(),
                                    _ => Ty::error(),
                                }
                            }
                            _ => Ty::error(),
                        }
                    }
                    HirUnaryOp::Custom(_) => Ty::error(),
                }
            }

            Expr::Call { callee, args } => {
                let universe = self.table.universe();
                locals.push_scope();
                let start = self.table.obligation_count();
                let ty = self.infer_call_hir(body, *callee, args, expectation, locals);
                let ty = if self.table.universe() != universe {
                    self.scope_obligations(start, id, locals);
                    self.table.close_universe(&ty, universe)
                } else {
                    ty
                };
                locals.pop_scope();
                self.table.restore_universe(universe);
                ty
            }

            Expr::If { cond, then_branch, else_branch } => {
                let saved_diverges = self.diverges;
                // With no else, the missing branch is unit. Supply that
                // context even when this conditional is a discarded statement,
                // so polymorphic failure calls are instantiated consistently.
                let unit = Expectation::HasType(Ty::named("unit"));
                let branch_expectation = if else_branch.is_none() { &unit } else { expectation };
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
                // Each branch is checked against the expectation, as
                // rust-analyzer's `infer_expr_coerce` checks each arm of an
                // `if`. Only the join used to be, so `let x : int = if c
                // then 3 else true` said nothing: the then branch satisfied
                // the annotation and the else branch was inferred with the
                // expectation flowing in and never compared against it.
                // Upstream reports it, at the branch.

                // Then branch: apply positive flow narrowings
                let predicate = self.boolean_guard(body, *cond, locals).map(|c| {
                    constraint::apply_subst_constraint_expr(&c, &locals.type_substitution())
                });
                let then_ty = {
                    let interval = super::refinement::range_guard(body, *cond, false, &|n| {
                        self.refinement_value(locals, n)
                    });
                    locals.push_scope();
                    if let Some((name, ty)) = interval {
                        locals.define(&name, ty);
                    }
                    if let Some(c) = &predicate {
                        locals.add_constraint(c.clone());
                    }
                    for (name, narrowed_ty) in flow.iter() {
                        let ty = intersect_narrowing(
                            self.env.lookup_value(locals, name.as_str()).as_ref(),
                            narrowed_ty,
                        );
                        locals.define(name.as_str(), ty);
                    }
                    let ty =
                        self.infer_expr_hir_with(body, *then_branch, branch_expectation, locals);
                    locals.pop_scope();
                    ty
                };

                let then_diverges = self.diverges;

                if let Some(else_id) = else_branch {
                    self.diverges = Diverges::Maybe;
                    let neg = crate::flow::narrow_from_guard_negated(body, *cond);
                    let else_ty = {
                        let interval = super::refinement::range_guard(body, *cond, true, &|n| {
                            self.refinement_value(locals, n)
                        });
                        locals.push_scope();
                        if let Some((name, ty)) = interval {
                            locals.define(&name, ty);
                        }
                        if let Some(c) = &predicate {
                            locals.add_constraint(negate_constraint(c.clone()));
                        }
                        for (name, narrowed_ty) in neg.iter() {
                            let ty = intersect_narrowing(
                                self.env.lookup_value(locals, name.as_str()).as_ref(),
                                narrowed_ty,
                            );
                            locals.define(name.as_str(), ty);
                        }
                        let ty = self.infer_expr_hir_with(body, *else_id, expectation, locals);
                        locals.pop_scope();
                        ty
                    };
                    let else_diverges = self.diverges;
                    // Both branches must diverge for the if to diverge.
                    self.diverges = saved_diverges | (then_diverges & else_diverges);
                    // Each branch has already been checked against this target.
                    // Their inferred types need not be subtypes of each other.
                    if let Expectation::HasType(target) = expectation {
                        return target.clone();
                    }
                    if then_diverges.is_always() {
                        return else_ty;
                    }
                    if else_diverges.is_always() {
                        return then_ty;
                    }
                    if let Some(cond) = &predicate {
                        if let Some(joined) = super::refinement::conditional_join(
                            cond,
                            &literal_evidence(body, *then_branch, &then_ty),
                            &literal_evidence(body, *else_id, &else_ty),
                        ) {
                            return joined;
                        }
                    }
                    if let Some(joined) = super::refinement::join(
                        &literal_evidence(body, *then_branch, &then_ty),
                        &literal_evidence(body, *else_id, &else_ty),
                    ) {
                        return joined;
                    }
                    // Unify branches: prefer non-unknown
                    if !then_ty.is_error() && !else_ty.is_error() {
                        let compatible = self.table.unify(&then_ty, &else_ty);
                        if !compatible
                            && !then_diverges.is_always()
                            && !else_diverges.is_always()
                            && !is_dependent_width_mismatch(&then_ty, &else_ty)
                            && union_of_ranges(&then_ty, &else_ty).is_none()
                            && !types_plausibly_compatible(&then_ty, &else_ty)
                        {
                            self.result.record_type_mismatch(id, &then_ty, &else_ty);
                        }
                    }
                    // Two numeric ranges join rather than one winning. An
                    // `if` produces either branch's value, so its range is
                    // the union: `if x > 31 then x - 32 else x + 32` over
                    // `range(0, 63)` is `range(0, 63)`, not the `range(0,
                    // 31)` the then branch alone would say.
                    if let Some(joined) = union_of_ranges(&then_ty, &else_ty) {
                        joined
                    } else if !then_ty.is_error() {
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
                // Through a type alias before matching. `type PTW_Result('v)
                // = result((PTW_Output('v), ext_ptw), ..)` is what
                // `vmem.sail` matches on, and a pattern cannot see the
                // `result` through the name — so `Ok(struct {ppn, pte, ..},
                // ext_ptw)` bound nothing, and `pbmt`, `ppn`, `pte` and
                // `pteAddr` are in the untyped list because of it. Aliases
                // are expanded here to classify the scrutinee; preserve the
                // original spelling separately for type-pattern matching.
                let original_scrutinee_ty = scrutinee_ty.clone();
                let scrutinee_ty = self.table.normalize_alias_ty(&scrutinee_ty);
                // `TR_Result(physaddr, ExceptionType)` is a *parameterised*
                // alias for `result((physaddr, page_based_mem_type,
                // ext_ptw), ..)`, and one step does not reach the union, so
                // `Ok(paddr, pbmt, _)` bound `pbmt` to nothing. Going deep
                // unconditionally is not the fix — that expands `xlenbits`
                // to `bits(xlen)` and the concat-width checks stop firing.
                // Expand only when it turns something that is *not* a union
                // into one, which is the case the arms need.
                let head_union = |me: &Self, t: &Ty| match t.kind() {
                    TyKind::Named(n, _) | TyKind::App { name: n, .. } => {
                        me.env.unions.contains_key(n)
                    }
                    _ => false,
                };
                let scrutinee_ty = if head_union(self, &scrutinee_ty) {
                    scrutinee_ty
                } else {
                    let deep = self.table.normalize_alias_deep(&scrutinee_ty);
                    if head_union(self, &deep) {
                        deep
                    } else {
                        scrutinee_ty
                    }
                };
                // Use InferTy for match result — will be refined by arm types
                let mut result_ty = self.table.new_type_var();
                // A match diverges only when every arm does. Leaving
                // `self.diverges` at whatever the last arm set made a single
                // `return` in one arm end the enclosing block, and the
                // statements after it were never inferred at all.
                let saved_diverges = self.diverges;
                let mut all_arms_diverge = !arms.is_empty();
                for arm in arms {
                    let arm_universe = self.table.universe();
                    self.diverges = Diverges::Maybe;
                    locals.push_scope();
                    // Matching a singleton integer against a literal refines
                    // its type variable only inside this arm.
                    if let (
                        Some(var),
                        Some(hir_def::Pat::Literal(hir_def::hir::Literal::Number(text))),
                    ) = (super::refinement::singleton(&scrutinee_ty), body.pat(arm.pat))
                    {
                        if let Some(value) = NumericExpr::integer_literal(text) {
                            locals.add_constraint(ConstraintExpr::Compare {
                                lhs: var,
                                op: CompareOp::Eq,
                                rhs: value,
                            });
                        }
                    }
                    // Check for duplicate bindings in pattern tree
                    self.check_pattern_duplicates(body, arm.pat);
                    // Bind pattern variables with scrutinee type
                    let binding_ty = if super::pat::contains_type_pattern(body, arm.pat, 0) {
                        &original_scrutinee_ty
                    } else {
                        &scrutinee_ty
                    };
                    let pattern_obligations = self.table.obligation_count();
                    self.bind_pattern_hir(body, arm.pat, binding_ty, locals);
                    self.scope_obligations(pattern_obligations, *scrutinee, locals);
                    // Infer guard — must be bool.

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
                        if let Some(c) = super::refinement::guard(body, guard_id, &|n| {
                            self.refinement_value(locals, n)
                        }) {
                            locals.add_constraint(constraint::apply_subst_constraint_expr(
                                &c,
                                &locals.type_substitution(),
                            ));
                        }
                        let flow_env = crate::flow::narrow_from_guard(body, guard_id);
                        for (name, narrowed_ty) in flow_env.iter() {
                            let ty = intersect_narrowing(
                                self.env.lookup_value(locals, name.as_str()).as_ref(),
                                narrowed_ty,
                            );
                            locals.define(name.as_str(), ty);
                        }
                    }
                    // Infer arm body
                    // Checked against the expectation, not merely inferred
                    // with it flowing in — the same discipline as an `if`'s
                    // branches. `Some(y) => 1` in a function returning
                    // `range(0, 0)` used to need its own literal-range check
                    // here, which said nothing about `Some(y) => true`.
                    let arm_ty = self.infer_expr_hir_with(body, arm.body, expectation, locals);
                    let arm_ty = if matches!(expectation, Expectation::None) {
                        literal_evidence(body, arm.body, &arm_ty)
                    } else {
                        arm_ty
                    };
                    let arm_ty = constraint::apply_subst(&arm_ty, &locals.type_substitution());
                    let arm_ty = super::refinement::normalize(
                        &self.table.normalize_alias_deep(&arm_ty),
                        &locals.type_substitution(),
                        &locals.constraints,
                    );
                    let arm_ty = self.table.close_universe(&arm_ty, arm_universe);
                    if let Expectation::HasType(target) = expectation {
                        result_ty = target.clone();
                        all_arms_diverge &= self.diverges.is_always();
                        locals.pop_scope();
                        self.table.restore_universe(arm_universe);
                        continue;
                    }
                    // Diverging arms contribute no value to the result join.
                    if self.diverges.is_always() {
                        locals.pop_scope();
                        self.table.restore_universe(arm_universe);
                        continue;
                    }
                    let resolved_result = self.table.resolve(&result_ty);
                    if let Some(joined) = super::refinement::join(&resolved_result, &arm_ty) {
                        result_ty = joined;
                    } else if result_ty.is_error() && !arm_ty.is_error() {
                        result_ty = arm_ty;
                    } else if !result_ty.is_error()
                        && !arm_ty.is_error()
                        && !self.table.unify(&result_ty, &arm_ty)
                    {
                        // Check if this is a dependent-type width mismatch
                        // (same outer constructor, different args) vs a real
                        // type error (completely different types).
                        //
                        // Sail dependent match pattern:
                        //   match 'm { 8 => bits(8), 16 => bits(16) }
                        // Upstream: each arm constrains 'm in its scope.
                        // Without an expected type, this legacy join still
                        // recovers same-constructor arms with different args.
                        //
                        // Real mismatch (int vs bool): always report.
                        let resolved_result = self.table.resolve(&result_ty);
                        if is_dependent_width_mismatch(&resolved_result, &arm_ty) {
                            result_ty = Ty::error();
                        } else {
                            self.result.record_type_mismatch(arm.body, &result_ty, &arm_ty);
                        }
                    }
                    all_arms_diverge &= self.diverges.is_always();
                    locals.pop_scope();
                    self.table.restore_universe(arm_universe);
                }
                self.diverges = if all_arms_diverge {
                    saved_diverges | Diverges::Always
                } else {
                    saved_diverges
                };
                // Match result type is determined by arm unification above.
                // Do NOT compare with self.return_ty here — the match may
                // be in a let-binding where the expected type differs.

                // Exhaustiveness checking
                self.check_match_exhaustiveness_hir(id, &scrutinee_ty, body, arms);
                result_ty
            }

            Expr::Block(stmts) => {
                let outer_universe = self.table.universe();
                locals.push_scope();
                let saved_diverges = self.diverges;
                self.diverges = Diverges::Maybe;
                let mut last_ty = Ty::named("unit".to_string());
                for (i, stmt) in stmts.iter().enumerate() {
                    // after a diverging expression (return/throw/exit).
                    if self.diverges.is_always() {
                        for rest in &stmts[i..] {
                            let id = match rest {
                                hir_def::Statement::Expr(id) => *id,
                                hir_def::Statement::Let { value, .. }
                                | hir_def::Statement::Var { value, .. } => *value,
                            };
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::UnreachableStatement { expr: id },
                            );
                        }
                        break;
                    }
                    match stmt {
                        hir_def::Statement::Let { pat, value } => {
                            // Extract type annotation from Typed pattern
                            let annotated_ty = match body.pat(*pat) {
                                Some(hir_def::Pat::Typed { ty: ty_text, .. }) => {
                                    let ty = constraint::apply_subst(
                                        &type_from_type_text(ty_text),
                                        &locals.type_substitution(),
                                    );
                                    if !ty.is_error() {
                                        Some(ty)
                                    } else {
                                        None
                                    }
                                }
                                _ => None,
                            };
                            // The annotation is the expectation; without one
                            // there is none. Nothing else may reach here —
                            // the enclosing function's return type used to,
                            // and made `let c = read_register(C)` inside
                            // `main() -> unit` bind the call's result to
                            // `unit`.
                            let val_ty = match annotated_ty {
                                Some(ref ann_ty) => self.infer_expr_hir_with(
                                    body,
                                    *value,
                                    &Expectation::HasType(ann_ty.clone()),
                                    locals,
                                ),
                                None => {
                                    if matches!(
                                        body.expr(*value),
                                        Some(Expr::Struct { name: None, .. })
                                    ) {
                                        self.invalid_expression(
                                            *value,
                                            "Cannot infer type of struct literal",
                                        );
                                    }
                                    self.infer_expr_hir(body, *value, locals)
                                }
                            };
                            // Resolved before it is bound: a `match` hands
                            // back the variable its arms bound, so an
                            // unresolved-looking type is what the binding
                            // recorded and what hover then showed —
                            // `let other : _` for something inference knows
                            // is a `Cfg`.
                            let bind_ty =
                                annotated_ty.unwrap_or_else(|| self.table.deep_resolve(&val_ty));
                            let bind_ty =
                                self.immutable_value_identity(body, *pat, *value, bind_ty, locals);
                            self.bind_pattern_hir(body, *pat, &bind_ty, locals);
                            self.check_pattern_duplicates(body, *pat);
                            last_ty = Ty::named("unit".to_string());
                        }
                        hir_def::Statement::Var { pat, value } => {
                            self.infer_mutable_binding(body, *pat, *value, locals);
                            last_ty = Ty::named("unit".to_string());
                        }
                        hir_def::Statement::Expr(e) => {
                            // A block's value is its last statement, so only
                            // that one inherits the block's expectation.
                            let tail = std::ptr::eq(stmt, stmts.last().unwrap());
                            last_ty = if tail {
                                self.infer_expr_hir_with(body, *e, expectation, locals)
                            } else {
                                self.infer_expr_hir(body, *e, locals)
                            };
                            // Propagate constraints from assert/if-throw
                            self.propagate_hir_constraints(body, *e, locals);
                        }
                    }
                }
                if let Some(target) = expectation
                    .to_option()
                    .filter(|_| matches!(stmts.last(), Some(hir_def::Statement::Expr(_))))
                {
                    // The target belongs to the caller's scope. A local type
                    // binder with the same spelling must not rewrite it.
                    last_ty = target;
                } else {
                    last_ty = constraint::apply_subst(&last_ty, &locals.type_substitution());
                    last_ty = self.table.close_universe(&last_ty, outer_universe);
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
                    // Recorded, because the type says `error()` and means
                    // *never*: a reader of the result — `analysis_stats`
                    // counting what inference could not type — would
                    // otherwise chase a block that cannot fall through.
                    self.result.diverging_exprs.insert(id);
                    Ty::error()
                } else {
                    last_ty
                }
            }

            Expr::Let { pat, value, body: let_body } => {
                let annotated_ty = match body.pat(*pat) {
                    Some(hir_def::Pat::Typed { ty: ty_text, .. }) => {
                        let ty = constraint::apply_subst(
                            &type_from_type_text(ty_text),
                            &locals.type_substitution(),
                        );
                        if !ty.is_error() {
                            Some(ty)
                        } else {
                            None
                        }
                    }
                    _ => None,
                };
                let val_ty = match annotated_ty {
                    Some(ref ann_ty) => self.infer_expr_hir_with(
                        body,
                        *value,
                        &Expectation::HasType(ann_ty.clone()),
                        locals,
                    ),
                    None => {
                        if matches!(body.expr(*value), Some(Expr::Struct { name: None, .. })) {
                            self.invalid_expression(*value, "Cannot infer type of struct literal");
                        }
                        self.infer_expr_hir(body, *value, locals)
                    }
                };
                let bind_ty = annotated_ty.unwrap_or_else(|| self.table.deep_resolve(&val_ty));
                let bind_ty = self.immutable_value_identity(body, *pat, *value, bind_ty, locals);
                let outer_universe = self.table.universe();
                locals.push_scope();
                self.bind_pattern_hir(body, *pat, &bind_ty, locals);
                self.check_pattern_duplicates(body, *pat);

                // Pattern binding opened any existential with fresh rigid
                // variables and added only its renamed predicate to this scope.

                let ty = self.infer_expr_hir_with(body, *let_body, expectation, locals);
                let ty = if let Some(target) = expectation.to_option() {
                    target
                } else {
                    let ty = constraint::apply_subst(&ty, &locals.type_substitution());
                    self.table.close_universe(&ty, outer_universe)
                };
                locals.pop_scope();
                ty
            }

            Expr::Var { pat, value, body: var_body } => {
                // Both forms of `var` share the pattern identity, annotation
                // and mutability rules. The expression form scopes its binding
                // to the explicit continuation.
                locals.push_scope();
                let outer_universe = self.table.universe();
                self.infer_mutable_binding(body, *pat, *value, locals);
                let ty = self.infer_expr_hir_with(body, *var_body, expectation, locals);
                let ty = if let Some(target) = expectation.to_option() {
                    target
                } else {
                    let ty = constraint::apply_subst(&ty, &locals.type_substitution());
                    self.table.close_universe(&ty, outer_universe)
                };
                locals.pop_scope();
                ty
            }

            // `return e` has whatever type is expected of it — upstream
            // annotates it with the checking type (type_check.ml:2380). An
            // inference variable says that and still unifies; `error()` says
            // "give up" and poisons whatever reads it.
            Expr::Return(e) => {
                let ret = self.return_ty.clone();
                let expected =
                    if ret.is_error() { Expectation::None } else { Expectation::HasType(ret) };
                // `infer_expr_hir_with`, not `infer_expr_expecting`: the
                // latter only feeds the expectation *into* inference, and
                // the comparison afterwards lives in the former. So a
                // `return` was never checked against the type the function
                // declares — the tail expression was, and every function
                // that returns early was not.
                self.infer_expr_hir_with(body, *e, &expected, locals);
                self.diverges = Diverges::Always;
                self.table.new_type_var()
            }
            // A `throw` carries an `exception`, not the return type. It
            // shared the arm above, so the value thrown was checked against
            // whatever the function returns.
            Expr::Throw(e) => {
                self.infer_expr_hir(body, *e, locals);
                self.diverges = Diverges::Always;
                self.table.new_type_var()
            }

            Expr::Exit(value) => {
                if let Some(value) = value {
                    self.infer_expr_hir_with(
                        body,
                        *value,
                        &Expectation::HasType(Ty::named("unit")),
                        locals,
                    );
                }
                self.diverges = Diverges::Always;
                self.table.new_type_var()
            }

            Expr::Assert { cond, message } => {
                self.infer_expr_hir(body, *cond, locals);
                if let Some(m) = message {
                    self.infer_expr_hir(body, *m, locals);
                }
                // Everything after the assert holds under it. Upstream adds
                // the condition to the environment (`E_assert`), which is how
                // `let x = signed(f); assert(x > -4); x` returns a
                // `range(-3, 3)` where `signed` gave `range(-4, 3)`.
                //
                // The narrowest useful case, and the only one that needs no
                // solver: a comparison between a local and an integer
                // literal, tightening one end of a concrete range.
                self.narrow_local_from_assert(body, *cond, locals);
                Ty::named("unit".to_string())
            }

            Expr::Tuple(items) => {
                if let Some(target) = expectation.to_option() {
                    let target = self.table.normalize_alias_deep(&target);
                    if let TyKind::Tuple(expected_items) = target.kind() {
                        if expected_items.len() == items.len() {
                            for (&item, expected) in items.iter().zip(expected_items) {
                                self.infer_expr_hir_with(
                                    body,
                                    item,
                                    &Expectation::HasType(expected.clone()),
                                    locals,
                                );
                            }
                            return target;
                        }
                    }
                }
                let tys: Vec<Ty> =
                    items.iter().map(|&i| self.infer_expr_hir(body, i, locals)).collect();
                Ty::tuple(tys)
            }

            Expr::List(items) => {
                // Try to get expected element type from function return
                let expected_elem = expectation
                    .to_option()
                    .map(|ret| self.table.normalize_alias_deep(&ret))
                    .as_ref()
                    .and_then(|ret| match ret.kind() {
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
                let elem = elem_ty.or(expected_elem).unwrap_or_else(|| {
                    if items.is_empty() {
                        self.table.new_type_var()
                    } else {
                        Ty::error()
                    }
                });
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
                // `type Foo32 = Foo(32)` names a record through an alias, and
                // the lookup below matches on the type's own name. Expand
                // first, or `foo.field` reports "no field `field` on type
                // `Foo32`".
                let base_ty = {
                    let expanded = self.table.normalize_alias_ty(&base_ty);
                    if self.record_info_for_type(&base_ty).is_none()
                        && self.record_info_for_type(&expanded).is_some()
                    {
                        expanded
                    } else {
                        base_ty
                    }
                };
                // Record field lookup with generic type argument instantiation
                if let Some(field_ty) = self.record_field_type(&base_ty, field) {
                    // Record which struct this field access resolves to.
                    // Enables goto-def for `x.field` → navigate to struct definition.
                    if let Some((record_name, _, _)) = self.record_info_for_type(&base_ty) {
                        // Look up the struct's DefIdx from the DefMap
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
                    TyKind::Named(name, _) => Some(name.clone()),
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
                    // Whether the record's declaration is in reach. A
                    // record named from another file resolves by name and
                    // not by body, so a missing field says nothing there.
                    let receiver_fields_known = self.record_info_for_type(&base_ty).is_some()
                        || base_name.as_ref().is_some_and(|n| self.env.bitfields.contains_key(n));
                    // Positively not a record: this handle knows the head
                    // name and knows it names something else. A record
                    // from another file is *unknown*, not "not a record",
                    // which is the distinction `receiver_fields_known`
                    // draws and this one must not blur.
                    let receiver_is_not_a_record = match base_name.as_ref() {
                        Some(n) => {
                            self.env.unions.contains_key(n) || self.env.enums.contains_key(n)
                        }
                        None => matches!(
                            base_ty.kind(),
                            crate::ty::TyKind::Scalar(_) | crate::ty::TyKind::Tuple(_)
                        ),
                    };
                    self.push_inference_diagnostic(InferenceDiagnostic::UnresolvedField {
                        expr: id,
                        receiver: base_ty.clone(),
                        name: field.clone(),
                        method_with_same_name_exists,
                        receiver_fields_known,
                        receiver_is_not_a_record,
                        in_assign_target: self.in_assign_target,
                    });
                }
                self.table.new_type_var()
            }

            Expr::Assign { target, value } => {
                if self.infer_implicit_assignment(body, *target, *value, locals) {
                    return Ty::named("unit");
                }
                // `LE_app(f, xs) = exp` desugars to `E_app(f, xs @ [exp])`
                // (initial_check.ml). Inferring the pieces separately left
                // the target call itself with no type — `X(rd) = value` was
                // the most common unvisited expression in sail-riscv — and
                // checked nothing about the assignment.
                if let Some(hir_def::Expr::Call { callee, args }) = body.expr(*target) {
                    // `LE_app(f, xs) = exp` desugars upstream to
                    // `E_app(f, xs @ [exp])`, but resolving it that way here
                    // reports valid code: `2.GPR() = 1.GPR()` and
                    // `(*p) = v` are call targets whose callee is not a
                    // function, and a setter's first parameter often needs a
                    // coercion this checker does not perform. The pieces are
                    // inferred without resolving the assignment, and the
                    // target is given a type so it is not left unvisited.
                    // The callee still gets the setter's signature. Reading
                    // it off the arity the desugaring implies — `args + 1` —
                    // is the part of `E_app(f, xs @ [exp])` that cannot
                    // report anything, and without it the name in
                    // `F_or_X_D(rd) = v` had no type at all: an overload
                    // name is not a value, so inferring it as a bare
                    // identifier yields `error()`.
                    let setter_sig = match body.expr(*callee) {
                        Some(hir_def::Expr::Ident(name)) => {
                            let candidates = self.env.lookup_functions(name);
                            let arity_matches =
                                |s: &&Arc<TypeScheme>| s.params.len() == args.len() + 1;
                            // A setter returns `unit`; a getter in the same
                            // overload set can match the arity and does not.
                            // `PC() = addr` for `overload PC = {rPC, wPC}`
                            // took `rPC : unit -> bits(64)` — one parameter
                            // by the zero-argument convention — and then
                            // checked `addr` against `unit`. Upstream never
                            // sees the pair: it desugars to `PC(addr)` and
                            // resolves the overload against the whole
                            // argument list.
                            candidates
                                .iter()
                                .filter(arity_matches)
                                .find(|s| s.ret.display_text() == "unit")
                                .or_else(|| candidates.iter().find(arity_matches))
                                .map(|s| crate::ty::FnSig {
                                    params: s.params.clone(),
                                    ret: s.ret.clone(),
                                })
                        }
                        _ => None,
                    };
                    // The setter's last parameter is what the assigned
                    // value has to be, and for `F(i) = nan_box(data)` it is
                    // the only thing that says what `nan_box`'s implicit
                    // width is: `nan_box : forall 'n 'm, 'n <= 'm.
                    // (implicit('m), bits('n)) -> bits('m)` has nowhere else
                    // to get `'m` from.
                    let assigned_to = setter_sig.as_ref().and_then(|sig| sig.params.last());
                    let expected_value = match assigned_to {
                        Some(ty) if !ty.is_error() => Some(Expectation::HasType(ty.clone())),
                        _ => None,
                    };
                    match setter_sig {
                        Some(sig) => {
                            self.result.write_expr_ty(*callee, Ty::new(TyKind::FnPtr(sig)))
                        }
                        None => {
                            self.infer_expr_hir(body, *callee, locals);
                        }
                    }
                    for &a in args {
                        self.infer_expr_hir(body, a, locals);
                    }
                    match expected_value {
                        Some(exp) => {
                            self.infer_expr_hir_with(body, *value, &exp, locals);
                        }
                        None => {
                            self.infer_expr_hir(body, *value, locals);
                        }
                    }
                    self.result.write_expr_ty(*target, Ty::named("unit".to_string()));
                } else {
                    // Upstream words a field on a non-record differently
                    // on the two sides of an `=`: `LE_field` says "Type
                    // set using field l-expression must be a struct type"
                    // (`type_check.ml:3611`) where the read says what the
                    // type *is* (`:3678`). The field arm cannot see which
                    // side it is on, so the assignment says so.
                    let outer = std::mem::replace(&mut self.in_assign_target, true);
                    let target_ty = self.infer_expr_hir(body, *target, locals);
                    self.in_assign_target = outer;
                    // The target supplies context before the RHS is inferred:
                    // both a register and a bitfield slice can determine the
                    // result of `zeros()` or `zero_extend(x)`. The ordinary
                    // checking path also retains directional numeric goals.
                    self.infer_expr_hir_with(
                        body,
                        *value,
                        &Expectation::HasType(target_ty.clone()),
                        locals,
                    );
                }
                Ty::named("unit".to_string())
            }

            Expr::While { cond, body: lb } | Expr::Repeat { body: lb, until: cond } => {
                // A loop body may not run, so a `return` inside it does not
                // make the loop diverge — and must not end the enclosing
                // block, which would leave everything after it uninferred.
                let saved_diverges = self.diverges;
                self.infer_expr_hir(body, *cond, locals);
                self.infer_expr_hir(body, *lb, locals);
                self.diverges = saved_diverges;
                Ty::named("unit".to_string())
            }

            // Foreach now has pat: PatId.
            Expr::Foreach { descending, pat, start, end, step, body: lb } => {
                let saved_diverges = self.diverges;
                let start_ty = self.infer_expr_hir(body, *start, locals);
                let end_ty = self.infer_expr_hir(body, *end, locals);
                let (positive_step, unit_step) = if let Some(step_id) = step {
                    let step_ty = self.infer_expr_hir(body, *step_id, locals);
                    let value =
                        super::refinement::singleton(&literal_evidence(body, *step_id, &step_ty))
                            .and_then(|n| super::integer::eval(&n, &Subst::default(), &[]));
                    (
                        value.as_ref().is_some_and(|n| n > &0.into()),
                        value.is_some_and(|n| n == 1.into()),
                    )
                } else {
                    (true, true)
                };
                let endpoint = |id, ty: &Ty| {
                    super::refinement::number_from_types(
                        body,
                        id,
                        &|n| self.refinement_value(locals, n),
                        &|id| self.result.expr_ty(id).cloned(),
                    )
                    .or_else(|| super::refinement::singleton(ty))
                    .map(|n| super::substitution::numeric(&n, &locals.type_substitution()))
                    .filter(|n| !super::subtype::contains_erased_numeric(n))
                };
                let (from, to) = (endpoint(*start, &start_ty), endpoint(*end, &end_ty));
                locals.push_scope();
                let iter_ty = if positive_step && descending.is_some() {
                    let complete = unit_step && from.is_some() && to.is_some();
                    let bound = |exact: Option<NumericExpr>, ty: &Ty, lower: bool| {
                        exact.or_else(|| {
                            let ty = self.table.normalize_alias_ty(ty);
                            if lower && matches!(ty.kind(), TyKind::Scalar(Scalar::Nat)) {
                                return Some(NumericExpr::Const(0));
                            }
                            if let TyKind::App { name, args, .. } = ty.kind() {
                                if name == "range" && args.len() == 2 {
                                    return NumericExpr::parse(
                                        &args[if lower { 0 } else { 1 }].as_value_str()?,
                                    );
                                }
                            }
                            None
                        })
                    };
                    let (lo, hi) = if *descending == Some(true) {
                        (bound(to, &end_ty, true), bound(from, &start_ty, false))
                    } else {
                        (bound(from, &start_ty, true), bound(to, &end_ty, false))
                    };
                    let mut used = HashSet::new();
                    for endpoint in lo.iter().chain(hi.iter()) {
                        super::substitution::numeric_names(endpoint, &mut used);
                    }
                    let mut var = "'_loop_index".to_owned();
                    while used.contains(var.trim_start_matches('\'')) {
                        var.push('_');
                    }
                    let index = NumericExpr::Var(var.clone());
                    // Partial bounds or an omitted stride are sufficient for
                    // proofs, but cannot justify a counterexample using values
                    // that the loop might never visit.
                    let mut facts =
                        if complete { Vec::new() } else { vec![ConstraintExpr::Unsupported] };
                    if let Some(lo) = lo {
                        facts.push(ConstraintExpr::Compare {
                            lhs: lo,
                            op: CompareOp::Lte,
                            rhs: index.clone(),
                        });
                    }
                    if let Some(hi) = hi {
                        facts.push(ConstraintExpr::Compare {
                            lhs: index.clone(),
                            op: CompareOp::Lte,
                            rhs: hi,
                        });
                    }
                    Ty::exist(
                        vec![var],
                        ConstraintExpr::And(facts),
                        Ty::app("atom", vec![TyArg::Nexp(index)], ""),
                    )
                } else {
                    Ty::named("int")
                };
                self.bind_pattern_hir(body, *pat, &iter_ty, locals);
                self.infer_expr_hir(body, *lb, locals);
                locals.pop_scope();
                self.diverges = saved_diverges;
                Ty::named("unit")
            }

            Expr::Cast { expr, ty } => {
                // `e : T` is an ascription — its type is `T`. The span of the
                // written type is on the node, read the same way a `let`
                // annotation is.
                // The ascription flows into the expression, as upstream's
                // `check_exp` on `E_typ` does. Without it an anonymous
                // `struct { .. } : foo` had no idea which record it was, so
                // none of its field values were checked.
                let ascribed = Some(constraint::apply_subst(
                    &type_from_type_text(ty),
                    &locals.type_substitution(),
                ))
                .filter(|t| !t.is_error());
                let inner = match &ascribed {
                    Some(ty) => self.infer_expr_expecting(
                        body,
                        *expr,
                        &Expectation::HasType(ty.clone()),
                        locals,
                    ),
                    None => self.infer_expr_hir(body, *expr, locals),
                };
                match ascribed {
                    Some(ty) => {
                        // The ascription was taken as the answer without
                        // ever being compared to what was ascribed, so
                        // `10 : range(0, 9)` typed as `range(0, 9)` and said
                        // nothing. An ascription is a claim, and upstream
                        // checks it (`check_exp` on `E_typ`).
                        //
                        // Only the literal, and this is why: checking the
                        // whole expression against the ascription draws 318
                        // false positives on sail-riscv, every one of them a
                        // mapping clause where the ascribed type is the
                        // constructor's payload and the body's type is the
                        // union. The expectation is sound to *flow into* an
                        // ascription and not sound to *judge with*.
                        self.check_literal_against_range(body, *expr, &ty);
                        ty
                    }
                    None => inner,
                }
            }

            Expr::Try { scrutinee, arms } => {
                // Catch patterns destructure the built-in exception union.
                // Keep its nominal type so payload matching does not recover
                // from an artificial unknown scrutinee.
                let body_ty = self.infer_expr_hir_with(body, *scrutinee, expectation, locals);
                let mut result_ty =
                    if body_ty.is_error() { self.table.new_type_var() } else { body_ty.clone() };

                for arm in arms {
                    let universe = self.table.universe();
                    locals.push_scope();
                    let start = self.table.obligation_count();
                    self.bind_pattern_hir(body, arm.pat, &Ty::named("exception"), locals);

                    if let Some(guard_id) = arm.guard {
                        self.infer_expr_hir_with(
                            body,
                            guard_id,
                            &Expectation::HasType(Ty::named("bool")),
                            locals,
                        );
                        if let Some(predicate) = self.boolean_guard(body, guard_id, locals) {
                            locals.add_constraint(predicate);
                        }
                    }

                    // Infer catch handler body
                    let arm_ty = self.infer_expr_hir_with(body, arm.body, expectation, locals);

                    // Unify with result type (catch must return same type as try body)
                    if let Expectation::HasType(target) = expectation {
                        result_ty = target.clone();
                    } else if result_ty.is_error() && !arm_ty.is_error() {
                        result_ty = arm_ty;
                    } else if !result_ty.is_error()
                        && !arm_ty.is_error()
                        && !self.table.unify(&result_ty, &arm_ty)
                    {
                        self.result.record_type_mismatch(arm.body, &result_ty, &arm_ty);
                    }
                    self.scope_obligations(start, arm.body, locals);
                    result_ty = self.table.close_universe(&result_ty, universe);
                    locals.pop_scope();
                    self.table.restore_universe(universe);
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
                // A config expression takes its type from the context —
                // upstream annotates `E_config key` with whatever type it is
                // being checked against (type_check.ml:2387). That is what an
                // inference variable is for.
                self.table.new_type_var()
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
                // Try to get type arguments from the expected type.
                let expected_ret = match expectation {
                    Expectation::HasType(ty) if !ty.is_error() => Some(ty.clone()),
                    _ => None,
                };
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
                // An anonymous `struct { field1 = .., field2 = .. }` names no
                // record, so none of the checking below could reach it — the
                // field values and the missing-field list both went
                // unexamined. The record it is comes from the context that
                // asked for it, which is what `: foo` and `let s : foo` are.
                // A named struct literal says which record it is; the
                // missing-field list is a claim about that record. An
                // anonymous one inherits its record from whatever asked for
                // it, and that context is not always right — sail-riscv
                // builds a write request where the expectation reads
                // `Mem_read_request`, and "missing fields" there is noise.
                // So the inherited record checks the values it was given and
                // says nothing about the ones it was not.
                let named_record = record_resolved.is_some();
                let record_resolved = record_resolved.or_else(|| {
                    let ret_ty = expected_ret.as_ref()?;
                    self.record_info_for_type(ret_ty)
                });
                // Sail's struct literal does not name its type — `struct {
                // a = 0x1 }` — so with no expectation to read it from, the
                // literal had no type and neither did the binding it
                // initialises, nor anything reached through that binding.
                // The fields say which struct it is: the one declaring every
                // field written, and only if exactly one does.
                //
                // Every field, and only those fields: upstream's `E_struct`
                // is the whole record written out — `E_struct_update` is the
                // form that names some of them — so a literal listing every
                // field of exactly one record *is* that record. Asking only
                // that the record *contain* the written fields made a
                // one-field literal ambiguous wherever a larger record
                // happened to declare that field too, and ambiguity here
                // means no type at all: `AK_arch(struct { variety = .. })`
                // matched both `RISCV_strong_access` and
                // `Explicit_access_kind`, while the two-field literal beside
                // it matched one and typed cleanly.
                //
                // The looser reading stays as a second pass. A literal that
                // does *not* list every field is not Sail, but this checker
                // seeing one is more often a sign that a field escaped the
                // record's own parse than that the source is wrong.
                let record_resolved = record_resolved.or_else(|| {
                    let written: HashSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                    let unique = |exact: bool| {
                        let mut hits = self.env.records.iter().filter(|(_, info)| {
                            !info.fields.is_empty()
                                && written.iter().all(|w| info.fields.contains_key(*w))
                                && (!exact || info.fields.len() == written.len())
                        });
                        let (rname, info) = hits.next()?;
                        if hits.next().is_some() {
                            return None;
                        }
                        Some((rname.clone(), info.clone(), Subst::default()))
                    };
                    unique(true).or_else(|| unique(false))
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
                    if !missing.is_empty() && named_record {
                        missing.sort();
                        self.push_inference_diagnostic(InferenceDiagnostic::MissingFields {
                            expr: id,
                            record_name: sname.clone(),
                            missing,
                        });
                    }
                    expected_ret
                        .filter(|t| {
                            self.record_info_for_type(t).is_some_and(|(name, _, _)| name == sname)
                        })
                        .unwrap_or_else(|| Ty::named(sname))
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
                    TyKind::Named(name, _) => Some(name.clone()),
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

            Expr::Attribute { name, expr } => {
                // `type_check.ml:2205`: a match that says `$[complete]` or
                // `$[incomplete]` is not checked for completeness. It is
                // upstream's sanctioned way to silence exactly the warning
                // this checker emits, and honouring it is the point of
                // keeping the attribute in the HIR at all.
                let saved = self.suppress_incomplete_match;
                self.suppress_incomplete_match =
                    saved || matches!(name.as_str(), "complete" | "incomplete");
                let ty = self.infer_expr_hir(body, *expr, locals);
                self.suppress_incomplete_match = saved;
                ty
            }

            Expr::Index { base, index } => {
                use hir_def::hir::{BinaryOp, HirBinaryOp};
                // Resolved: a `let` bound to a `match` yields the match's
                // *variable*, which the arms bind to the real type — so
                // `cfg` in `let cfg = match .. { .. }; cfg[A]` arrives here
                // as `_` that resolves to `Cfg`, and `bitfield_info_for_type`
                // matches on `TyKind::Named` and saw a variable. Every
                // bitfield read off such a binding was untyped, and so was
                // everything downstream of it — which is how
                // `pmpAddrMatchType_encdec(cfg[A])` came to have no type.
                let base_ty = self.infer_expr_hir(body, *base, locals);
                let base_ty = self.table.resolve(&base_ty);
                // `menvcfg[LPE]` is a *field* read, not an element read: the
                // index is a bitfield field name, and the result is that
                // field's width. Nothing typed it, so every bitfield read
                // off a register was `error()` — and so was everything
                // computed from one, `bool_bit(menvcfg[LPE])` among them.
                // `bitfield_field_type` has been sitting here unused since
                // it was written.
                if let Some(hir_def::Expr::Ident(field)) = body.expr(*index) {
                    if let Some(ty) = self.bitfield_field_type(&base_ty, field) {
                        self.result.write_expr_ty(*index, Ty::named("int".to_string()));
                        return ty;
                    }
                    // Only an unresolved base is interesting: a vector
                    // index whose index is a name is ordinary, and a
                    // bitfield read whose *base* has no type is how
                    // `pmpAddrMatchType_encdec(cfg[A])` came out untyped —
                    // the mapping was fine, `cfg` was not.
                    if matches!(self.table.resolve(&base_ty).kind(), TyKind::Infer(_))
                        && std::env::var("SAIL_UNTYPED_CALL").is_ok()
                    {
                        eprintln!("[index] base is unresolved, field={field}");
                    }
                }
                if self.in_assign_target {
                    let shape = self.table.normalize_alias_deep_for_shape(&base_ty);
                    if self.bitfield_info_for_type(&base_ty).is_some() {
                        self.invalid_expression(id, "Index is not a bitfield accessor");
                    } else if !matches!(shape.ctor_name(), Some("bits" | "bitvector" | "vector"))
                        && self.known_type_shape(&shape)
                    {
                        self.invalid_expression(
                            id,
                            "Invalid vector assignment l-expression: expected a vector or bitfield",
                        );
                    }
                }
                let index_ty = self.infer_expr_hir(body, *index, locals);
                // Check index is int
                if !index_ty.is_error() {
                    let int_ty = Ty::named("int".to_string());
                    if !self.table.unify(&int_ty, &index_ty) {
                        self.result.record_type_mismatch(*index, &int_ty, &index_ty);
                    }
                }
                // The index's *type* has to fit the vector, not just a
                // literal written at the call. `v[unsigned(n)]` for an
                // `n : bits(32)` indexes with a `range(0, 4294967295)`,
                // which no two- or three-bit vector can hold — upstream
                // proves `0 <= i < 'n` and rejects it, and nothing here
                // looked past a literal.
                //
                // Both ends decided, or nothing is said: a width that is a
                // constant or a quantifier this signature bounds
                // (`forall 'n, 'n in {2, 3}` caps it at 3), and an index
                // range with a concrete upper bound.
                //
                // Not for an index that is *computed*: `pmpcfg_n[n * 4 + 7]`
                // for an `n : range(0, 15)` has an interval of [7, 67] by
                // this checker's own arithmetic, and upstream proves it fits
                // 64 from an `assert(n % 2 == 0)` beside it. An interval this
                // checker derives is an over-approximation; a range read
                // straight off a call's declared return type is exact, and
                // only the second is proof.
                let index_is_computed = matches!(
                    body.expr(*index),
                    Some(Expr::BinaryOp { op: HirBinaryOp::Known(BinaryOp::ArithOp(_)), .. })
                );
                // A symbolic index the signature *bounds*: `GPRs[r]` for an
                // `r : implicit('d)` under `'d in {32, 64}` reaches 64, and
                // no four-element vector holds that. Upstream says "Could
                // not resolve quantifiers for plain_vector_access:
                // (0 <= 'd & 'd < 4)".
                //
                // Only a bound this checker read for itself. A quantifier
                // constrained by a predicate it does not parse — `regno('r)`
                // is `'r >= 0 & 'r < 4` — is *not* unbounded, it is unknown,
                // and treating the two alike reported the file this mutant
                // comes from.
                let idx_bounds = concrete_range_bounds(&index_ty).or_else(|| {
                    let quant = single_symbolic_index_var(&index_ty)?;
                    self.quant_max.get(&quant).map(|max| (0, *max))
                });
                if let (Some(info), Some((_, idx_hi))) =
                    (self.concat_operand_info(&base_ty).filter(|_| !index_is_computed), idx_bounds)
                {
                    let width = info
                        .width
                        .parse::<i128>()
                        .ok()
                        .or_else(|| self.quant_max.get(info.width.trim()).copied());
                    if let Some(width) = width {
                        if idx_hi >= width {
                            self.push_inference_diagnostic(
                                InferenceDiagnostic::ConstraintViolation {
                                    expr: id,
                                    constraint: format!("0 <= index < {width}"),
                                    derived_from: Vec::new(),
                                },
                            );
                        }
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
                self.concat_operand_info(&base_ty)
                    .map(|info| info.elem)
                    .unwrap_or_else(|| self.table.new_type_var())
            }

            Expr::Subrange { base, hi, lo } => {
                let base_ty = self.infer_expr_hir(body, *base, locals);
                let shape = self.table.normalize_alias_deep_for_shape(&base_ty);
                if self.in_assign_target
                    && !matches!(shape.ctor_name(), Some("bits" | "bitvector" | "vector"))
                    && self.known_type_shape(&shape)
                {
                    self.invalid_expression(id, "Cannot assign slice of non vector type");
                }
                let _hi_ty = self.infer_expr_hir(body, *hi, locals);
                let _lo_ty = self.infer_expr_hir(body, *lo, locals);
                // A symbolic slice has a known expression for its length.
                // Keep endpoint identities rather than inventing a flexible
                // width that can be chosen from the expected result type.
                let endpoint = |id, ty: &Ty| {
                    super::refinement::number_from_types(
                        body,
                        id,
                        &|n| self.refinement_value(locals, n),
                        &|id| self.result.expr_ty(id).cloned(),
                    )
                    .or_else(|| super::refinement::singleton(ty))
                };
                let symbolic_endpoints = (endpoint(*hi, &_hi_ty), endpoint(*lo, &_lo_ty));
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
                            node: hir_def::ExprOrPatId::ExprId(id),
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
                } else if let (Some(first), Some(last)) = symbolic_endpoints {
                    use NumericExpr as N;
                    let (upper, lower) = match self.env.vector_order {
                        hir_def::type_error::VectorOrder::Dec => (first, last),
                        hir_def::type_error::VectorOrder::Inc => (last, first),
                    };
                    let width = N::Add(
                        Box::new(N::Sub(Box::new(upper), Box::new(lower))),
                        Box::new(N::Const(1)),
                    );
                    let width = super::substitution::numeric(&width, &locals.type_substitution());
                    let width = super::nexp_simplify::simplify(&width);
                    Ty::app("bits", vec![TyArg::Nexp(width)], "")
                } else if !base_ty.is_error() {
                    // A runtime width is rigid: the expected type cannot choose
                    // its value. Keep the vector shape and its nonnegative bound.
                    let width = "'_slice_width".to_owned();
                    let value = NumericExpr::Var(width.clone());
                    let opened = self.table.open_existential(
                        &[width],
                        &ConstraintExpr::Compare {
                            lhs: NumericExpr::Const(0),
                            op: CompareOp::Lte,
                            rhs: value.clone(),
                        },
                        &Ty::app("bits", vec![TyArg::Nexp(value)], ""),
                    );
                    locals.add_constraint(opened.constraint);
                    opened.inner_ty
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
        let _budget =
            crate::solver_budget::SolverBudget::enter(self.cancel.clone(), Default::default());
        // body and source_map are set at construction (new_for_body).
        // No reset_table needed — fresh context per callable.
        // Clone the Arc to get &Body without borrowing self.
        let body_arc = self.body.clone();
        let body: &hir_def::Body = &body_arc;

        // Look up expected scheme from env (val spec or inline signature)
        let expected_scheme: Option<Arc<TypeScheme>> = {
            let initializer = callable_body.initializer_type.as_deref().map(|text| {
                Arc::new(TypeScheme {
                    quantifiers: Vec::new(),
                    kind_bounds: HashMap::new(),
                    constraints: Vec::new(),
                    params: Vec::new(),
                    implicit_params: Vec::new(),
                    ret: type_from_type_text(text),
                    declared_effects: Vec::new(),
                    is_declared_pure: false,
                })
            });
            let local = initializer
                .or_else(|| {
                    self.env.functions.get(name).and_then(|schemes| schemes.first().cloned())
                })
                // A top-level `let x : T = e` is a body too, and its annotated
                // type is what `e` must check against. It lives in `values`,
                // not `functions`.
                .or_else(|| {
                    // Only when the annotation checks by plain unification.
                    // A numeric range or an existential needs coercion and
                    // instantiation, which this path does not perform, so
                    // using it as the expected type invents errors.
                    let simple = |t: &Ty| match t.kind() {
                        TyKind::Exist { .. } => false,
                        // `implicit` is never a real expected type. A
                        // `range` or `atom` was excluded for the same reason
                        // as the existential — its bounds can mention
                        // quantifiers this path does not instantiate — but a
                        // concrete one has none to instantiate, and
                        // excluding it meant `let v : range(0, 0) = f()`
                        // was checked against nothing at all.
                        TyKind::App { name, .. } if name == "implicit" => false,
                        TyKind::App { name, .. } if name == "range" || name == "atom" => {
                            t.is_fully_known()
                        }
                        TyKind::App { .. } => true,
                        TyKind::Named(name, _) => {
                            !self.env.alias_schemes.contains_key(name.as_str())
                        }
                        _ => true,
                    };
                    self.env.values.get(name).filter(|t| !t.is_error() && simple(t)).map(|t| {
                        Arc::new(TypeScheme {
                            quantifiers: Vec::new(),
                            kind_bounds: HashMap::new(),
                            constraints: Vec::new(),
                            params: Vec::new(),
                            implicit_params: Vec::new(),
                            ret: t.clone(),
                            declared_effects: Vec::new(),
                            is_declared_pure: false,
                        })
                    })
                });
            // If local scheme has error return type (scattered clause without
            // inline return type), try cross-file val spec lookup.
            let needs_cross_file = local.as_ref().map(|s| s.ret.is_error()).unwrap_or(true);
            if needs_cross_file {
                // A later body can own the symbol-index entry while its
                // explicit `val` lives elsewhere. Read the shared signatures
                // first instead of asking that body for its missing type again.
                if let Some(cross) = self
                    .env
                    .cross_file_schemes
                    .as_ref()
                    .and_then(|all| all.get(name))
                    .and_then(|schemes| schemes.iter().find(|s| !s.ret.is_error()))
                {
                    Some(cross.clone())
                } else if let Some(db) = self.db {
                    if let Some(file) = self.env.symbol_index.get_file(name) {
                        let env_data = super::dependencies::file_symbol(db, file, name.to_owned());
                        if let Some(cross) =
                            env_data.data.functions.get(name).and_then(|s| s.first().cloned())
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

        let mut locals = LocalEnv::new();
        if let Some(scheme) = &expected_scheme {
            let mut vars = scheme.quantifiers.clone();
            for ty in scheme.params.iter().chain(std::iter::once(&scheme.ret)) {
                collect_type_vars(ty, &mut vars);
            }
            for name in vars {
                locals.define_type_variable(&name);
            }
        }

        // A body is checked *under* its own quantifier constraints. Upstream
        // seeds them the same way — `check_funcl` runs `Env.add_typquant`,
        // whose `QI_constraint` arm is `add_constraint` (type_env.ml:1143) —
        // and without them the assumption set held only top-level
        // `constraint` definitions and local existentials. So a call from
        // `forall 'n, 'n in {16, 32, 64}. bits('n) -> bool` to a function
        // declared with *the same* constraint could not discharge it: the
        // fact was written on the caller and never read.
        if let Some(scheme) = &expected_scheme {
            for c in &scheme.constraints {
                // `forall 'width, is_mem_width('width)` carries the only
                // fact there is about `'width`, and folded up it carries
                // none.
                locals.add_constraint(constraint::expand_constraint_synonyms(
                    &c.expr,
                    &self.env.constraint_synonyms,
                    0,
                ));
            }
        }

        // The largest value each quantifier can take, where the signature
        // says so. `forall 'n, 'n in {2, 3}` bounds `'n` at 3, which is what
        // makes an index into a `bits('n)` decidable at all.
        if let Some(scheme) = &expected_scheme {
            self.quant_max.clear();
            for c in &scheme.constraints {
                for (var, max) in c.quantifier_upper_bounds() {
                    let slot = self.quant_max.entry(var).or_insert(max);
                    *slot = (*slot).max(max);
                }
            }
        }

        // Bind parameter patterns with expected types
        if let Some(scheme) = &expected_scheme {
            let mut param_tys = scheme.params.clone();
            if param_tys.len() == 1 && body.params.len() > 1 {
                if let TyKind::Tuple(items) = param_tys[0].kind() {
                    if items.len() == body.params.len() {
                        param_tys = items.clone();
                    }
                }
                // `function clause execute VVTYPE(funct6, vm, vs2)` declares
                // the constructor's payload, not `execute`'s parameter, and
                // the head lowers to `[VVTYPE, funct6, vm, vs2]` — the
                // constructor arrives as the first parameter. The scheme says
                // `instruction -> ..`, so zipping bound `VVTYPE` to
                // `instruction` and left the rest untyped, which is how `vd`,
                // `vs2`, `vm`, `rs1` and everything derived from them lost
                // their types across sail-riscv. Upstream matches the whole
                // head as one pattern against the union type instead
                // (`P_app (f, pats) when is_union_constructor`,
                // type_check.ml:2974).
                let ctor = body.params.first().and_then(|p| match body.pat(*p) {
                    Some(hir_def::Pat::Bind(n)) => Some(n.clone()),
                    _ => None,
                });
                if let Some(payload) = ctor
                    .as_deref()
                    .filter(|c| self.env.constructor_names().any(|k| k == c))
                    .and_then(|c| self.env.lookup_functions(c).first().cloned())
                    .and_then(|c| c.params.first().cloned())
                {
                    let payload_tys = match payload.kind() {
                        TyKind::Tuple(items) => items.clone(),
                        _ => vec![payload.clone()],
                    };
                    if payload_tys.len() + 1 == body.params.len() {
                        let mut tys = vec![param_tys[0].clone()];
                        tys.extend(payload_tys);
                        param_tys = tys;
                    }
                }
            }
            if body.params.len() == param_tys.len() {
                for (pat_id, expected_ty) in body.params.iter().zip(param_tys.iter()) {
                    let expected_ty = self.immutable_boolean_identity(expected_ty.clone());
                    self.bind_pattern_hir(body, *pat_id, &expected_ty, &mut locals);
                }
                self.check_quantifiers_are_determined(body, &param_tys);
            } else {
                // Fewer patterns than declared types is the definition
                // disagreeing with its `val`, and upstream rejects it. More
                // patterns is not: a nested pattern such as
                // `((instr :: rest), i)` lowers to three patterns against
                // two declared types, and a `@` concatenation head lowers
                // flat — both legitimately outnumber the declared types.
                // One pattern stands for the whole parameter tuple, and its
                // type is that tuple. Binding it to a fresh variable instead
                // — which is what happened — left every use of it
                // unchecked, because a fresh variable unifies with anything.
                //
                // This was measured once before the parameter lowering was
                // fixed and produced 171 errors on sail-riscv, all of them
                // the lowering's. It produces none now.
                if body.params.len() == 1 && param_tys.len() > 1 {
                    let tuple = Ty::tuple(param_tys.clone());
                    self.bind_pattern_hir(body, body.params[0], &tuple, &mut locals);
                } else if body.params.len() >= 2 && body.params.len() < param_tys.len() {
                    self.push_inference_diagnostic(
                        crate::infer::InferenceDiagnostic::MismatchedParamCount {
                            body_expr: body.body_expr,
                            expected: param_tys.len(),
                            found: body.params.len(),
                        },
                    );
                }
                // Fewer patterns than declared types, and the list is
                // positional to begin with: the definition disagrees with
                // the `val` that declares it, and upstream rejects it.
                //
                // The `>= 2` is not caution, it is meaning. One pattern
                // stands for the whole tuple —
                // `function mem_write_callback(_) = ()` against a `val`
                // declaring four — and none is `function f()` against
                // `val f : unit -> ..`, which declares `unit` as a
                // parameter. Both are legal Sail and both are common.
                //
                // What this reported on sail-riscv, as the parameter
                // lowering was fixed one shape at a time, is the record of
                // how wrong the counts were: **287, 18, 9, 4, 0** — a
                // wildcard parameter that was dropped, a binding that
                // arrives as a bare token in a guarded head, a `@` group
                // beside commas, and a guard's second pair of parentheses.
                // The check was written first and held back four times
                // until the thing it counts was true.
                // The counts disagree, so position says nothing. Bind the
                // patterns to variables rather than to types known to be
                // wrong.
                for &pat_id in body.params.iter() {
                    let var = self.table.new_type_var();
                    self.bind_pattern_hir(body, pat_id, &var, &mut locals);
                }
            }
        } else {
            for &pat_id in body.params.iter() {
                self.bind_pattern_hir(body, pat_id, &Ty::error(), &mut locals);
            }
        }

        // A clause's guard is an expression like any other, and it has to be
        // a `bool`. It was never lowered, so nothing in one was ever checked
        // — `f (A(v) if v == 0x00000000)` compared a `bits(33)` against a
        // `bits(32)` in silence.
        if let Some(guard_id) = body.guard {
            let guard_ty = self.infer_expr_hir(body, guard_id, &mut locals);
            let bool_ty = Ty::named("bool".to_string());
            if !guard_ty.is_error() && !self.table.unify(&bool_ty, &guard_ty) {
                self.result.record_type_mismatch(guard_id, &bool_ty, &guard_ty);
            }
            if let Some(predicate) = self.boolean_guard(body, guard_id, &locals) {
                locals.add_constraint(predicate);
            }
        }

        // Infer the body expression with expected return type
        let expectation = match &expected_scheme {
            Some(scheme) if !scheme.ret.is_error() => Expectation::HasType(scheme.ret.clone()),
            _ => Expectation::None,
        };
        // This entry point checks the declared return expectation and scopes
        // any deferred goal with the body's facts. A second unification here
        // duplicates diagnostics and can lose those local assumptions.
        self.infer_expr_hir_with(body, body.root(), &expectation, &mut locals);

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
        let _budget =
            crate::solver_budget::SolverBudget::enter(self.cancel.clone(), Default::default());
        // body and source_map set at construction.
        let body_arc = self.body.clone();
        let body: &hir_def::Body = &body_arc;

        // Look up mapping scheme for input/output types
        let mapping_scheme = self.env.lookup_mappings(name).first().cloned();
        let (input_ty, output_ty) = match &mapping_scheme {
            Some(ms) => (ms.lhs.clone(), ms.rhs.clone()),
            None => (Ty::error(), Ty::error()),
        };

        if std::env::var_os("SAIL_DBG_M").is_some() {
            eprintln!("MAPARMS {} n={}", name, body.mapping_arms.len());
        }
        for arm in &body.mapping_arms {
            let mut locals = LocalEnv::new();

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

            // Each direction is checked in its own scope. Upstream is
            // explicit about this — "types from one side of the mapping will
            // have been checked in a different environment"
            // (type_check.ml:1673) — and the two sides really do disagree:
            // in `MUL(rs2, rs1, rd, is_unsigned) <-> .. @ bool_bit(is_unsigned) @ ..`
            // the left pattern binds `is_unsigned : bool` while the right one
            // binds it from a bit slice. Sharing one scope let the bit
            // binding win, so the right-hand side read `bool_bit(bits(1))`,
            // resolved the mapping backwards, and reported "cannot
            // concatenate non-vector type bool".
            //
            // Forwards binds the left pattern and checks the right side as an
            // expression; backwards does the reverse. Each expression is
            // still inferred exactly once.
            let mut backwards = LocalEnv::new();
            if let Some(scheme) = &mapping_scheme {
                let mut vars = scheme.quantifiers.clone();
                collect_type_vars(&scheme.lhs, &mut vars);
                collect_type_vars(&scheme.rhs, &mut vars);
                for name in vars {
                    locals.define_type_variable(&name);
                    backwards.define_type_variable(&name);
                }
            }
            if let Some(pat_id) = arm.lhs_pat {
                self.bind_pattern_hir(body, pat_id, lhs_expected, &mut locals);
            }
            backwards.fill_from_other_side(&locals);
            if let Some(pat_id) = arm.rhs_pat {
                self.bind_pattern_hir(body, pat_id, rhs_expected, &mut backwards);
            }
            let bindings_start = self.table.obligation_count();
            for (name, lhs) in locals.bindings.clone() {
                if let Some(rhs) = backwards.lookup(&name).cloned() {
                    // Both patterns bind the same runtime value. Independently
                    // opened existential indices name that one value, rather
                    // than two universally unrelated integers.
                    if let (Some(a), Some(b)) =
                        (super::refinement::singleton(&lhs), super::refinement::singleton(&rhs))
                    {
                        let identity =
                            ConstraintExpr::Compare { lhs: a, op: CompareOp::Eq, rhs: b };
                        locals.add_constraint(identity.clone());
                        backwards.add_constraint(identity);
                    }
                    if !lhs.is_error()
                        && !rhs.is_error()
                        && (!self.constructor_types_plausible(&lhs, &rhs)
                            || !self.table.try_unify(&lhs, &rhs))
                    {
                        // This compares one shared binding, not the type of
                        // the entire mapping side. Point at that binding.
                        let location = self
                            .source_map
                            .as_ref()
                            .and_then(|map| {
                                let span = map.expr_syntax(arm.rhs_expr)?;
                                body.iter_exprs().find_map(|(id, expr)| {
                                    let hir_def::Expr::Ident(bound) = expr else { return None };
                                    let part = map.expr_syntax(id)?;
                                    (bound == &name
                                        && span.start <= part.start
                                        && part.end <= span.end)
                                        .then_some(hir_def::ExprOrPatId::ExprId(id))
                                })
                            })
                            .unwrap_or(hir_def::ExprOrPatId::ExprId(arm.rhs_expr));
                        self.result.record_type_mismatch_at(location, &lhs, &rhs);
                    }
                }
            }
            self.scope_obligations(bindings_start, arm.rhs_expr, &locals);
            // Copied bindings can refer to witnesses introduced by the other
            // pattern. Their bounds travel with them in both directions.
            let facts: Vec<_> =
                locals.constraints.iter().chain(&backwards.constraints).cloned().collect();
            for fact in facts {
                if !locals.constraints.contains(&fact) {
                    locals.add_constraint(fact.clone());
                }
                if !backwards.constraints.contains(&fact) {
                    backwards.add_constraint(fact);
                }
            }
            // Separate scopes, but not sealed ones. Upstream binds each side
            // independently and then checks it with the *other* side's
            // environment as fallback (type_check.ml:4682). Without that,
            // `VCLMUL_VV(vm, vs2) <-> 0b001100 @ vm @ encdec_vreg(vs2) @ ..`
            // left every identifier under a `@` untyped in the backwards
            // scope: a vector-concat pattern cannot recover an operand's
            // width on its own, and only the constructor payload on the
            // other side knows `vm` is one bit.
            let forwards_bindings = locals.clone();
            backwards.fill_from_other_side(&forwards_bindings);
            locals.fill_from_other_side(&backwards);

            // The guard may mention either side, so it sees both.
            if let Some(guard_id) = arm.guard {
                let mut guard_env = locals.clone();
                guard_env.merge_bindings(&backwards);
                let guard_ty = self.infer_expr_hir(body, guard_id, &mut guard_env);
                if !guard_ty.is_error()
                    && !self.table.unify(&Ty::named("bool".to_string()), &guard_ty)
                {
                    let bool_ty = Ty::named("bool".to_string());
                    self.result.record_type_mismatch(guard_id, &bool_ty, &guard_ty);
                }
            }

            // Each side is checked against the type the mapping declares for
            // it. Inferring them without an expectation meant a clause could
            // produce anything at all: `val m : bits(2) <-> bits(17)` with
            // `0b00 <-> 0xFFFF` went unreported.
            //
            // Only a bidirectional clause has two real sides. A `forwards p
            // => e` arm lowers with both `lhs_expr` and `rhs_expr` pointing
            // at the body root, so checking those against the mapping's
            // types would compare the whole body against each side.
            let two_sided = arm.direction == hir_def::MappingDirection::Bidirectional
                && arm.lhs_expr != arm.rhs_expr;
            let expect = |ty: &Ty| {
                if ty.is_error() || !two_sided {
                    Expectation::None
                } else {
                    Expectation::HasType(ty.clone())
                }
            };
            // A side that has a pattern is checked as that pattern, above.
            // Both views are the same syntax, so expecting a type of the
            // expression too reported every mismatch twice.
            let mirror = |pat: Option<hir_def::PatId>, ty: &Ty| {
                if arm.mirrored && pat.is_some() {
                    Expectation::None
                } else {
                    expect(ty)
                }
            };
            let lhs_expectation = mirror(arm.lhs_pat, lhs_expected);
            let rhs_expectation = mirror(arm.rhs_pat, rhs_expected);
            let saved_in_mapping_arm = self.in_mapping_arm;
            self.in_mapping_arm = true;
            if arm.lhs_expr == arm.rhs_expr {
                // A one-directional clause has one expression, not two:
                // `forwards Regidx(r) => zero_extend(r)` lowers with both
                // fields pointing at the body. Inferring it twice ran it
                // first against the *other* side's environment, which does
                // not hold what the head bound — so `r` came out untyped,
                // and the second pass found the expression already recorded.
                let mut both = locals.clone();
                both.merge_bindings(&backwards);
                let output = if rhs_expected.is_error() {
                    Expectation::None
                } else {
                    Expectation::HasType(rhs_expected.clone())
                };
                self.infer_expr_hir_with(body, arm.lhs_expr, &output, &mut both);
            } else {
                // Each syntax side keeps its own pattern bindings. The
                // opposite side has already supplied missing names through
                // fill_from_other_side; it must not overwrite a known type.
                self.infer_expr_hir_with(body, arm.lhs_expr, &lhs_expectation, &mut locals);
                self.infer_expr_hir_with(body, arm.rhs_expr, &rhs_expectation, &mut backwards);
            }
            self.in_mapping_arm = saved_in_mapping_arm;

            // Mapping-clause symmetry is checked on the CST, in
            // `ide_diagnostics::semantic::check_mapping_clause_symmetry`. The
            // HIR version that used to live here was gated behind a flag
            // nothing ever set, because it could not tell a constructor from a
            // binding without workspace context and did not understand
            // vector-concatenation patterns.
        }

        // The `<->` node is the clause, not an expression — upstream has a
        // separate `MCL_bidir` AST node for it and never gives it a type.
        // Lowering it as an infix operator leaves it in the expression arena,
        // where it was the single most common untyped expression in
        // sail-riscv (2,507 of them). A clause carries no value, which Sail
        // spells `unit`.
        let unit = Ty::named("unit".to_string());
        let clause_ids: Vec<hir_def::ExprId> = body
            .store
            .exprs
            .iter()
            .filter(|(_, expr)| {
                matches!(expr, hir_def::Expr::BinaryOp { op, .. } if op.as_str() == "<->")
            })
            .map(|(id, _)| id)
            .collect();
        for id in clause_ids {
            self.result.write_expr_ty(id, unit.clone());
        }
        // The body of a mapping is a list of clauses, not an expression, and
        // inference walks the arms rather than the block that holds them. It
        // still has to carry a type or it counts as one inference never
        // reached.
        self.result.write_expr_ty(body.root(), unit);
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
                if let Some(c) = self.boolean_guard(body, *cond, locals) {
                    locals.add_constraint(constraint::apply_subst_constraint_expr(
                        &c,
                        &locals.type_substitution(),
                    ));
                }
                // Use narrow_from_guard (HIR-based) for type narrowing.
                let flow = crate::flow::narrow_from_guard(body, *cond);
                for (name, narrowed_ty) in flow.iter() {
                    let ty = intersect_narrowing(
                        self.env.lookup_value(locals, name.as_str()).as_ref(),
                        narrowed_ty,
                    );
                    locals.define(name.as_str(), ty);
                }
                // Also try text-based constraint extraction for Z3 solver
                if let Some(cond_span) = self.expr_span(body, *cond) {
                    if let Some(cond_text) = self.source.get(cond_span.start..cond_span.end) {
                        if let Some(constraint) = constraint_expr_from_expr_text(cond_text) {
                            locals.add_constraint(constraint::apply_subst_constraint_expr(
                                &constraint,
                                &locals.type_substitution(),
                            ));
                        }
                    }
                }
            }
            Expr::If { cond, then_branch, else_branch: None } => {
                // if cond then throw/exit — negate the condition
                let is_diverging = self.result.diverging_exprs.contains(then_branch)
                    || matches!(
                        body.expr(*then_branch),
                        Some(Expr::Throw(_) | Expr::Exit(_) | Expr::Return(_))
                    );
                if is_diverging {
                    let predicate = super::refinement::guard(body, *cond, &|n| {
                        self.refinement_value(locals, n)
                    });
                    if predicate.is_none() {
                        // The continuing path has a fact we cannot encode.
                        // A model of the remaining facts is not necessarily
                        // a reachable counterexample.
                        locals.add_constraint(ConstraintExpr::Unsupported);
                    }
                    if let Some(c) = predicate {
                        locals.add_constraint(negate_constraint(
                            constraint::apply_subst_constraint_expr(
                                &c,
                                &locals.type_substitution(),
                            ),
                        ));
                    }
                    if let Some(cond_span) = self.expr_span(body, *cond) {
                        if let Some(cond_text) = self.source.get(cond_span.start..cond_span.end) {
                            if let Some(constraint) = constraint_expr_from_expr_text(cond_text) {
                                locals.add_constraint(constraint::apply_subst_constraint_expr(
                                    &negate_constraint(constraint),
                                    &locals.type_substitution(),
                                ));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Numeric expectations consume literal values without changing unconstrained
/// literal inference or the element types chosen by collection joins.
fn numeric_evidence_target(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Exist { .. })
        || matches!(ty.kind(), TyKind::Scalar(Scalar::Int | Scalar::Nat))
        || matches!(ty.kind(), TyKind::App { name, .. }
            if matches!(name.as_str(), "atom" | "int" | "nat" | "range" | "implicit"))
}

/// Supply concrete witnesses when an existential expectation reaches a tuple.
/// Also supplies branch joins with values that coarse literal inference omits.
fn literal_evidence(body: &hir_def::Body, id: hir_def::ExprId, inferred: &Ty) -> Ty {
    if let Some(value) = super::refinement::number(body, id, &|_| None)
        .and_then(|n| super::integer::eval(&n, &Subst::default(), &[]))
    {
        return Ty::app("atom", vec![TyArg::numeric(value.to_string())], "");
    }
    if let (Some(hir_def::Expr::Tuple(items)), TyKind::Tuple(types)) =
        (body.expr(id), inferred.kind())
    {
        if items.len() == types.len() {
            return Ty::tuple(
                items.iter().zip(types).map(|(id, ty)| literal_evidence(body, *id, ty)).collect(),
            );
        }
    }
    inferred.clone()
}

/// The value of an integer literal expression, if that is what this is.
///
/// A negation is folded in: `-1` lowers to a prefix operator over `1`, and
/// `range(-1, 0)` has to see the sign.
fn integer_literal_value(body: &hir_def::Body, id: hir_def::ExprId) -> Option<i128> {
    match body.expr(id)? {
        hir_def::Expr::Literal(hir_def::hir::Literal::Number(text)) => {
            text.replace('_', "").parse::<i128>().ok()
        }
        hir_def::Expr::UnaryOp {
            op: hir_def::hir::HirUnaryOp::Known(hir_def::hir::UnaryOp::Neg),
            expr,
        } => integer_literal_value(body, *expr).map(|n| -n),
        _ => None,
    }
}

/// The bounds of a `range(lo, hi)` or `atom(n)` whose arguments are numbers.
/// The lone type variable an `int('d)` / `implicit('d)` index stands for.
fn single_symbolic_index_var(ty: &Ty) -> Option<String> {
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    if !matches!(name.as_str(), "int" | "atom" | "nat" | "implicit") || args.len() != 1 {
        return None;
    }
    let text = match &args[0] {
        TyArg::Nexp(n) => n.to_string_repr(),
        TyArg::Bool(_) | TyArg::Order(_) => return None,
        TyArg::Value(v) => v.trim().to_string(),
        TyArg::Type(t) => match t.kind() {
            TyKind::Param(p) => p.clone(),
            _ => return None,
        },
    };
    text.starts_with('\'').then_some(text)
}

/// The conjuncts of a constraint, flattened. An `Or` is left whole.
fn flatten_conjuncts(c: &ConstraintExpr, out: &mut Vec<ConstraintExpr>) {
    match c {
        ConstraintExpr::And(items) => items.iter().for_each(|i| flatten_conjuncts(i, out)),
        other => out.push(other.clone()),
    }
}

/// The one value a numeric type admits, when it admits one.
///
/// `int(8)` and `atom(8)` do; `int('n)` and `range(0, 3)` do not.
fn concrete_singleton_value(ty: &Ty) -> Option<i128> {
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    if !matches!(name.as_str(), "int" | "atom" | "implicit") || args.len() != 1 {
        return None;
    }
    match &args[0] {
        TyArg::Nexp(n) => n.to_string_repr().parse().ok(),
        TyArg::Bool(_) | TyArg::Order(_) => None,
        TyArg::Value(v) => v.parse().ok(),
        TyArg::Type(t) => t.display_text().parse().ok(),
    }
}

pub(super) fn concrete_range_bounds(ty: &Ty) -> Option<(i128, i128)> {
    // `(T)` is `T` — a parenthesised assignment target such as `(*r) = 10`
    // arrives as a one-element tuple.
    if let TyKind::Tuple(items) = ty.kind() {
        if items.len() == 1 {
            return concrete_range_bounds(&items[0]);
        }
    }
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    // `(*r) = 10` assigns through a `register(range(0, 10))`; the bounds
    // being asked about are the referent's.
    if name == "register" {
        if let Some(TyArg::Type(inner)) = args.first() {
            return concrete_range_bounds(inner);
        }
        return None;
    }
    let num = |a: &TyArg| -> Option<i128> {
        match a {
            TyArg::Nexp(n) => n.to_string_repr().parse::<i128>().ok(),
            TyArg::Value(v) => v.trim().parse::<i128>().ok(),
            TyArg::Type(_) | TyArg::Bool(_) | TyArg::Order(_) => None,
        }
    };
    match name.as_str() {
        "range" => Some((num(args.first()?)?, num(args.get(1)?)?)),
        "atom" => {
            let n = num(args.first()?)?;
            Some((n, n))
        }
        _ => None,
    }
}

/// The argument type to unify against a parameter, using the literal's own
/// value where the parameter is numeric and open.
///
/// `sail_zeros : forall 'n. int('n) -> bits('n)` learns its width from the 8
/// in `sail_zeros(8)` and from nowhere else. A `bool` parameter learns
/// nothing from it, and must keep seeing the `int` that does not fit.
fn singleton_for_numeric_param(param: &Ty, actual: &Ty, literal: Option<i128>) -> Ty {
    let Some(n) = literal else { return actual.clone() };
    if !matches!(actual.kind(), TyKind::Scalar(Scalar::Int)) {
        return actual.clone();
    }
    if matches!(param.kind(), TyKind::Scalar(Scalar::Nat)) && n >= 0 {
        return Ty::named("nat");
    }
    let TyKind::App { name, args, .. } = param.kind() else { return actual.clone() };
    if !matches!(name.as_str(), "int" | "nat" | "atom" | "range") {
        return actual.clone();
    }
    if !args.iter().any(|a| matches!(a, TyArg::Type(_))) {
        return actual.clone();
    }
    Ty::app("atom", vec![TyArg::numeric(n.to_string())], format!("atom({n})"))
}

/// The width of a `bits(N)`/`bitvector(N, _)` whose size is a literal number.
fn concrete_bits_width(ty: &Ty) -> Option<usize> {
    let (name, args) = match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
        _ => return None,
    };
    if name != "bits" && name != "bitvector" {
        return None;
    }
    match args.first()? {
        TyArg::Nexp(n) => n.to_string_repr().parse().ok(),
        TyArg::Value(v) => v.trim().parse().ok(),
        TyArg::Type(_) | TyArg::Bool(_) | TyArg::Order(_) => None,
    }
}

/// Combine a flow narrowing with what the binding was already known to be.
///
/// `x < 32` narrows to `range(min_int, 31)` — the comparison says nothing
/// about the lower end. Replacing the binding's type with that loses the `0`
/// in `x : range(0, 63)` and leaves an interval no arithmetic can use. The
/// intersection is what the branch actually knows: `range(0, 31)`.
fn intersect_narrowing(current: Option<&Ty>, narrowed: &Ty) -> Ty {
    let Some(current) = current else { return narrowed.clone() };
    // The branch predicate already constrains this value. Replacing its
    // identity with an interval would disconnect it from earlier facts.
    if super::refinement::singleton(current).is_some() {
        return current.clone();
    }
    let (Some((a, b)), Some((c, d))) =
        (concrete_range_bounds(current), open_range_bounds(narrowed))
    else {
        return narrowed.clone();
    };
    let lo = a.max(c.unwrap_or(a));
    let hi = b.min(d.unwrap_or(b));
    if lo > hi {
        return narrowed.clone();
    }
    Ty::app(
        "range",
        vec![TyArg::numeric(lo.to_string()), TyArg::numeric(hi.to_string())],
        format!("range({lo}, {hi})"),
    )
}

/// Bounds of a `range`, with `min_int`/`max_int` reported as "no bound".
fn open_range_bounds(ty: &Ty) -> Option<(Option<i128>, Option<i128>)> {
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    if name != "range" {
        return None;
    }
    let bound = |a: Option<&TyArg>| -> Option<i128> {
        match a? {
            TyArg::Nexp(n) => n.to_string_repr().parse().ok(),
            TyArg::Value(v) => v.trim().parse().ok(),
            TyArg::Type(_) | TyArg::Bool(_) | TyArg::Order(_) => None,
        }
    };
    Some((bound(args.first()), bound(args.get(1))))
}

/// The smallest range containing both, when both are concrete ranges.
fn union_of_ranges(a: &Ty, b: &Ty) -> Option<Ty> {
    let (alo, ahi) = concrete_range_bounds(a)?;
    let (blo, bhi) = concrete_range_bounds(b)?;
    let (lo, hi) = (alo.min(blo), ahi.max(bhi));
    Some(Ty::app(
        "range",
        vec![TyArg::numeric(lo.to_string()), TyArg::numeric(hi.to_string())],
        format!("range({lo}, {hi})"),
    ))
}

/// Decide an equality constraint when everything but one variable is known.
///
/// `Some(false)` when no integer satisfies it — `'m == 8 * 'n` with `'m` at
/// 65. `Some(true)` when one does. `None` when this cannot tell, which is
/// every other shape.
fn solve_equation_constraint(
    expr: &ConstraintExpr,
    known: &std::collections::HashMap<String, i64>,
) -> Option<bool> {
    use crate::ty::CompareOp;
    let parts: Vec<&ConstraintExpr> = match expr {
        ConstraintExpr::And(items) => items.iter().collect(),
        other => vec![other],
    };
    let substitute = |e: &crate::ty::NumericExpr| -> crate::ty::NumericExpr {
        fn go(
            e: &crate::ty::NumericExpr,
            known: &std::collections::HashMap<String, i64>,
        ) -> crate::ty::NumericExpr {
            use crate::ty::NumericExpr as N;
            match e {
                N::Var(v) | N::Symbol(v) => match known.get(v.as_str()) {
                    Some(&n) => N::Const(n),
                    None => e.clone(),
                },
                N::Neg(i) => N::Neg(Box::new(go(i, known))),
                N::Add(a, b) => N::Add(Box::new(go(a, known)), Box::new(go(b, known))),
                N::Sub(a, b) => N::Sub(Box::new(go(a, known)), Box::new(go(b, known))),
                N::Mul(a, b) => N::Mul(Box::new(go(a, known)), Box::new(go(b, known))),
                other => other.clone(),
            }
        }
        go(e, known)
    };
    for part in parts {
        let ConstraintExpr::Compare { lhs, op: CompareOp::Eq, rhs } = part else { continue };
        let (l, r) = (substitute(lhs), substitute(rhs));
        let (expr_side, value) = match (
            crate::infer::nexp_simplify::try_eval_const(&l),
            crate::infer::nexp_simplify::try_eval_const(&r),
        ) {
            (Some(a), Some(b)) => return Some(a == b),
            (Some(a), None) => (r, a),
            (None, Some(b)) => (l, b),
            (None, None) => continue,
        };
        match crate::infer::nexp_simplify::solve_linear(&expr_side, value) {
            Some(Err(())) => return Some(false),
            Some(Ok(_)) => return Some(true),
            None => {}
        }
    }
    None
}

/// The kind of a *type*, for a comparison that needs no solver.
///
/// Sail has no coercion between a bitvector and a number — `unsigned` and
/// `to_bits` exist because there is none — so a parameter of one kind
/// meeting an argument of the other is wrong whatever the widths turn out
/// to be. Anything with a quantifier or a variable still in it answers
/// `None`: that is the solver's business, not this one's.
fn type_kind(t: &Ty) -> Option<u8> {
    match t.kind() {
        TyKind::Scalar(s) => match s {
            Scalar::Bool => Some(0),
            Scalar::String => Some(1),
            Scalar::Int | Scalar::Nat => Some(2),
            Scalar::Unit => Some(3),
            // `bit` is `bits(1)`.
            Scalar::Bit => Some(4),
            Scalar::Real => Some(5),
        },
        TyKind::App { name, .. } => match name.as_str() {
            "bits" | "bitvector" => Some(4),
            "int" | "nat" | "atom" | "range" | "implicit" => Some(2),
            "bool" | "atom_bool" => Some(0),
            _ => None,
        },
        _ => None,
    }
}

/// What kind of value an operand is, for an operator whose overload set
/// admits only one of them.
///
/// By kind, not by name: `nat` and `int` are both numeric and compare
/// fine, and anything with a quantifier or a variable in it is the
/// solver's business — `None` means "say nothing". A primitive is a
/// `TyKind::Scalar`, not a `TyKind::Named`; matching only the latter is
/// why an earlier attempt at this never fired.
fn operand_kind(body: &hir_def::Body, t: &Ty, e: hir_def::ExprId) -> Option<u8> {
    let from_ty = match t.kind() {
        TyKind::Scalar(s) => match s {
            Scalar::Bool => Some(0),
            Scalar::String => Some(1),
            Scalar::Int | Scalar::Nat => Some(2),
            Scalar::Unit => Some(3),
            _ => None,
        },
        TyKind::App { name, .. } if matches!(name.as_str(), "bool" | "atom_bool") => Some(0),
        _ => None,
    };
    from_ty.or_else(|| match body.expr(e) {
        // A number literal is numeric whatever the solver calls its type:
        // `32` arrives as an `atom`/`range`, not as `int`.
        Some(hir_def::Expr::Literal(hir_def::hir::Literal::Number(_))) => Some(2),
        Some(hir_def::Expr::Literal(hir_def::hir::Literal::String(_))) => Some(1),
        Some(hir_def::Expr::Literal(hir_def::hir::Literal::Bool(_))) => Some(0),
        _ => None,
    })
}

#[cfg(test)]
mod check_positions {
    /// A value position with an expectation checks against it.
    ///
    /// rust-analyzer keeps two functions and a rule about which to call:
    /// `infer_expr_inner` lets the expectation flow in, `infer_expr_coerce`
    /// flows it in *and* coerces — and every value position with an
    /// expected type calls the second. Ours are `infer_expr_expecting` and
    /// `infer_expr_hir_with`, and the rule was not written down anywhere,
    /// so `if`'s branches, `match`'s arms, a block's tail and a `let … in`
    /// body all flowed the expectation in and never compared against it.
    /// `let x : int = if c then 3 else true` reported nothing; upstream
    /// reports it, at the branch.
    ///
    /// This is a ratchet, not a proof: it lists the exceptions and
    /// names each one, so the next value position added has to argue for
    /// itself rather than pick the wrong function by accident.
    #[test]
    fn a_value_position_with_an_expectation_is_checked() {
        const SRC: &str = include_str!("expr.rs");
        // Each exception, with the reason it is one.
        let allowed: &[(&str, &str)] = &[
            (
                "self.infer_expr_expecting(",
                "call argument hint; the selected signature checks the actual argument type. Coercing to this hint early would erase evidence and move a result mismatch into a valid nested argument",
            ),
            (
                "let ty = self.infer_expr_expecting(",
                "inside the checking wrapper; unreachable branches omit the hint and discharge it from contradictory assumptions",
            ),
            (
                "self.infer_expr_expecting(body, id, &Expectation::None, locals)",
                "`infer_expr_hir`: no expectation, so there is nothing to check against",
            ),
            (
                "Some(ty) => self.infer_expr_expecting(",
                "`e : T`. Checking the expression against the ascription draws 318 \
                 false positives on sail-riscv, all mapping clauses where the ascribed \
                 type is a constructor's payload and the body's type is the union. \
                 Sound to flow in, not sound to judge with — `check_literal_against_range` \
                 covers the part that is.",
            ),
        ];

        // Up to this module: the allowlist below quotes the very lines it
        // is describing, and a scan that read them would match itself.
        let source = SRC.split("mod check_positions").next().unwrap();
        let sites: Vec<&str> = source
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("self.infer_expr_expecting("))
            .collect();
        for site in &sites {
            assert!(
                allowed.iter().any(|(text, _)| site.starts_with(text)),
                "`{site}` lets an expectation flow in without checking against it.\n\
                 Call `infer_expr_hir_with` instead, or add it to the list above with \
                 the reason it cannot be checked."
            );
        }
        assert_eq!(
            sites.len(),
            allowed.len(),
            "the exception list names {} call sites and the source has {}",
            allowed.len(),
            sites.len()
        );
    }
}

/// Unsupported relations remain in the result without an unhelpful warning.
fn has_reportable_constraint(c: &ConstraintExpr) -> bool {
    match c {
        ConstraintExpr::Unsupported => false,
        ConstraintExpr::And(parts) | ConstraintExpr::Or(parts) => {
            parts.iter().any(has_reportable_constraint)
        }
        ConstraintExpr::Not(c) => has_reportable_constraint(c),
        _ => true,
    }
}
