//! Type unification and inference variable management.
//!
//! Uses ena union-find with snapshot/rollback for speculative unification.

use std::collections::HashMap;

use ena::unify::{self as ut, InPlace, NoError, UnifyKey, UnifyValue};

use super::coerce;
use super::constraint;
use super::{Obligation, Ty, TyArg, TyKind};
use crate::ty::{ConstraintExpr, NumericExpr};

/// ena type variable key. Wraps InferTy's u32 ID.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct TyVarKey(pub u32);

impl UnifyKey for TyVarKey {
    type Value = TyVarValue;
    fn index(&self) -> u32 {
        self.0
    }
    fn from_index(i: u32) -> Self {
        TyVarKey(i)
    }
    fn tag() -> &'static str {
        "TyVarKey"
    }
    fn order_roots(a: Self, _: &Self::Value, b: Self, _: &Self::Value) -> Option<(Self, Self)> {
        if a.0 < b.0 {
            Some((a, b))
        } else {
            Some((b, a))
        }
    }
}

/// ena type variable value.
#[derive(Clone, Debug)]
pub enum TyVarValue {
    Unknown,
    Known(Ty),
}

impl UnifyValue for TyVarValue {
    type Error = NoError;
    fn unify_values(v1: &Self, v2: &Self) -> Result<Self, NoError> {
        match (v1, v2) {
            // invariant check — two known types should never reach
            // unify_values because bind() checks equality first.
            // If this fires, a code path bypassed bind().
            (TyVarValue::Known(_), TyVarValue::Known(_)) => {
                debug_assert!(
                    false,
                    "unify_values called with two Known values — bind() should have caught this"
                );
                Ok(v1.clone())
            }
            (TyVarValue::Known(_), TyVarValue::Unknown) => Ok(v1.clone()),
            (TyVarValue::Unknown, TyVarValue::Known(_)) => Ok(v2.clone()),
            (TyVarValue::Unknown, TyVarValue::Unknown) => Ok(TyVarValue::Unknown),
        }
    }
}

/// Manages inference variables during type inference.
#[derive(Debug)]
pub struct InferenceTable {
    pub(super) table: ut::UnificationTable<InPlace<TyVarKey>>,
    pub(super) var_count: u32,
    pending_obligations: Vec<Obligation>,
    type_aliases: HashMap<String, Ty>,
    alias_schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    /// Freshened placeholder names → InferTy ids for deep_resolve.
    placeholder_to_infer: HashMap<String, u32>,
}

impl Default for InferenceTable {
    fn default() -> Self {
        Self {
            table: ut::UnificationTable::new(),
            var_count: 0,
            pending_obligations: Vec::new(),
            type_aliases: HashMap::new(),
            alias_schemes: HashMap::new(),
            placeholder_to_infer: HashMap::new(),
        }
    }
}

impl InferenceTable {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a table with type alias context for normalization.
    pub(super) fn with_aliases(type_aliases: HashMap<String, Ty>) -> Self {
        Self { type_aliases, ..Self::default() }
    }

    pub(super) fn set_alias_schemes(
        &mut self,
        schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    ) {
        self.alias_schemes = schemes;
    }

    /// Look up an alias scheme by name (excludes config-dependent aliases).
    pub(super) fn alias_scheme(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<super::env::AliasScheme>> {
        let sch = self.alias_schemes.get(name)?.clone();
        if sch.config_dependent {
            return None;
        }
        Some(sch)
    }

    /// Number of type variables allocated so far.
    pub(super) fn var_count(&self) -> u32 {
        self.var_count
    }

    /// Probe a type variable, returning Unknown if out of range.
    pub(super) fn safe_probe_by_id(&mut self, id: u32) -> TyVarValue {
        if id < self.var_count {
            self.table.probe_value(TyVarKey(id))
        } else {
            TyVarValue::Unknown
        }
    }

    /// Expand type aliases eagerly, with cycle detection.
    pub(super) fn normalize_alias_ty(&self, ty: &Ty) -> Ty {
        let mut current = ty.clone();
        let mut seen = std::collections::HashSet::new();
        loop {
            let name = match current.kind() {
                TyKind::Adt(n, _) => n.clone(),
                TyKind::App { name: n, .. } => n.clone(),
                _ => return current,
            };
            if !seen.insert(name.clone()) {
                return current; // cycle detected
            }
            match self.type_aliases.get(&name) {
                Some(expanded) => {
                    current = expanded.clone();
                }
                None => return current,
            }
        }
    }

    /// Queue a constraint obligation for later batch solving.
    #[allow(dead_code)]
    pub fn push_obligation(&mut self, ob: Obligation) {
        self.pending_obligations.push(ob);
    }

    /// Take all pending obligations for batch solving.
    #[allow(dead_code)]
    pub(super) fn drain_obligations(&mut self) -> Vec<Obligation> {
        std::mem::take(&mut self.pending_obligations)
    }

    /// Create a fresh inference variable.
    pub fn new_type_var(&mut self) -> Ty {
        let key = self.table.new_key(TyVarValue::Unknown);
        self.var_count += 1;
        Ty::new(TyKind::Infer(crate::ty::InferTy(key.0)))
    }

    /// Replace `Ty::Error` and `Ty::Param` with fresh inference variables.
    pub fn insert_type_vars(&mut self, ty: &Ty) -> Ty {
        use crate::ty::Scalar;
        match ty.kind() {
            TyKind::Error => self.new_type_var(),
            TyKind::Param(_) => self.new_type_var(),
            // Scalar types with constraint args: strip args.
            // bool(not('p)) → bool, int('n) → int, atom(N) → int, range(lo,hi) → int
            TyKind::App { name, .. } if name == "bool" || name == "atom_bool" => {
                Ty::scalar(Scalar::Bool)
            }
            // atom(N), range(lo,hi): these contain numeric constraints
            // from the callee's scope. Replace with fresh var — the
            // caller's context will determine the actual type.
            TyKind::App { name, .. } if name == "atom" || name == "range" => self.new_type_var(),
            // int(constraint), nat(constraint): strip constraint, keep base
            TyKind::App { name, .. } if name == "int" || name == "nat" => Ty::named(name.clone()),
            TyKind::App { name, args, text } => {
                // Structural types (bits, vector, etc.): recurse into args
                let new_args: Vec<crate::ty::TyArg> = args
                    .iter()
                    .map(|a| match a {
                        crate::ty::TyArg::Type(t) => {
                            crate::ty::TyArg::Type(self.insert_type_vars(t))
                        }
                        // Value args containing param refs: replace with fresh var
                        crate::ty::TyArg::Value(s) if s.contains('\'') => {
                            crate::ty::TyArg::Type(self.new_type_var())
                        }
                        other => other.clone(),
                    })
                    .collect();
                Ty::app(name.clone(), new_args, text.clone())
            }
            TyKind::Tuple(items) => {
                Ty::tuple(items.iter().map(|t| self.insert_type_vars(t)).collect())
            }
            TyKind::Exist { inner, .. } => self.insert_type_vars(inner),
            _ => ty.clone(),
        }
    }

    /// Resolve a type, following Infer bindings to their final value.
    /// Non-Infer types are returned as-is (shallow).
    pub fn shallow_resolve(&mut self, ty: &Ty) -> Ty {
        self.resolve_depth(ty, 0)
    }

    /// Backward-compat alias for `shallow_resolve`.
    pub(super) fn resolve(&mut self, ty: &Ty) -> Ty {
        self.shallow_resolve(ty)
    }

    fn resolve_depth(&mut self, ty: &Ty, depth: usize) -> Ty {
        if depth > 64 {
            return ty.clone();
        }
        match ty.kind() {
            TyKind::Infer(crate::ty::InferTy(id)) => {
                let key = TyVarKey(*id);
                match self.table.probe_value(key) {
                    TyVarValue::Known(bound) => self.resolve_depth(&bound, depth + 1),
                    TyVarValue::Unknown => ty.clone(),
                }
            }
            TyKind::Tuple(items) => {
                let resolved: Vec<Ty> = items.iter().map(|t| self.resolve(t)).collect();
                Ty::tuple(resolved)
            }
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                let resolved_params: Vec<Ty> = params.iter().map(|t| self.resolve(t)).collect();
                let resolved_ret = self.resolve(ret);
                Ty::function(resolved_params, resolved_ret)
            }
            TyKind::App { name, args, text } => {
                let name = name.clone();
                let text = text.clone();
                let args = args.clone();
                let resolved_args: Vec<TyArg> = args
                    .iter()
                    .map(|a| match a {
                        TyArg::Type(t) => TyArg::Type(self.resolve(t)),
                        other => other.clone(),
                    })
                    .collect();
                Ty::app(name, resolved_args, text)
            }
            TyKind::Exist { vars, constraint, inner } => {
                let vars = vars.clone();
                let constraint = constraint.clone();
                let inner = inner.clone();
                let resolved_inner = self.resolve(&inner);
                Ty::exist(vars, constraint, resolved_inner)
            }
            TyKind::Bidir { lhs, rhs } => {
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                let resolved_lhs = self.resolve(&lhs);
                let resolved_rhs = self.resolve(&rhs);
                Ty::bidir(resolved_lhs, resolved_rhs)
            }
            _ => ty.clone(),
        }
    }

    /// Resolve all Infers in a Ty to Error (for final output).
    pub(super) fn resolve_or_unknown(&mut self, ty: &Ty) -> Ty {
        match ty.kind() {
            TyKind::Infer(crate::ty::InferTy(id)) => {
                let key = TyVarKey(*id);
                match self.table.probe_value(key) {
                    TyVarValue::Known(bound) => self.resolve_or_unknown(&bound),
                    TyVarValue::Unknown => Ty::error(),
                }
            }
            TyKind::Tuple(items) => {
                let resolved: Vec<Ty> = items.iter().map(|t| self.resolve_or_unknown(t)).collect();
                Ty::tuple(resolved)
            }
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                let resolved_params: Vec<Ty> =
                    params.iter().map(|t| self.resolve_or_unknown(t)).collect();
                let resolved_ret = self.resolve_or_unknown(ret);
                Ty::function(resolved_params, resolved_ret)
            }
            TyKind::App { name, args, text } => {
                let name = name.clone();
                let text = text.clone();
                let args = args.clone();
                let resolved_args: Vec<TyArg> = args
                    .iter()
                    .map(|a| match a {
                        TyArg::Type(t) => TyArg::Type(self.resolve_or_unknown(t)),
                        other => other.clone(),
                    })
                    .collect();
                Ty::app(name, resolved_args, text)
            }
            TyKind::Exist { vars, constraint, inner } => {
                let vars = vars.clone();
                let constraint = constraint.clone();
                let inner = inner.clone();
                let resolved_inner = self.resolve_or_unknown(&inner);
                Ty::exist(vars, constraint, resolved_inner)
            }
            TyKind::Bidir { lhs, rhs } => {
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                let resolved_lhs = self.resolve_or_unknown(&lhs);
                let resolved_rhs = self.resolve_or_unknown(&rhs);
                Ty::bidir(resolved_lhs, resolved_rhs)
            }
            _ => ty.clone(),
        }
    }

    /// Unwrap an existential by replacing bound vars with fresh InferTys.
    fn freshen_existential(
        &mut self,
        vars: &[String],
        constraint: &ConstraintExpr,
        inner: &Ty,
    ) -> Ty {
        let mut subst = std::collections::HashMap::new();
        for var in vars {
            let fresh = self.new_type_var();
            subst.insert(var.clone(), fresh);
        }

        // Step 2: substitute vars in constraint and push as obligation,
        // but only if the constraint resolves to something decidable
        // (no unresolved inference variables). Constraints with ?N vars
        // can't be checked yet — they'll be verified when the caller
        // resolves the unification result and checks obligations.
        if !matches!(constraint, ConstraintExpr::Bool(true) | ConstraintExpr::Unsupported) {
            let subst_constraint = substitute_constraint(constraint, &subst);
            // Only push if the constraint doesn't contain inference variable
            // references (Symbol("?N")) — those can't be resolved yet.
            let text = subst_constraint.to_text();
            if !text.contains('?') {
                self.push_obligation(Obligation::NumericConstraint(subst_constraint));
            }
        }

        substitute_params(inner, &subst)
    }

    /// Bind an inference variable to a type (with occurs check).
    pub(super) fn bind(&mut self, var_id: u32, ty: Ty) -> bool {
        // Range guard: var from a different InferenceTable
        if var_id >= self.var_count {
            return false;
        }
        let key = TyVarKey(var_id);
        // Occurs check — reject binding ?N = T if T contains ?N
        if self.ty_contains_infer_var(&ty, var_id) {
            return false;
        }
        match self.safe_probe_by_id(var_id) {
            TyVarValue::Known(existing) => existing == ty,
            TyVarValue::Unknown => {
                self.table.union_value(key, TyVarValue::Known(ty));
                true
            }
        }
    }

    /// Check if a type (after resolution) contains the given Infer.
    fn ty_contains_infer_var(&mut self, ty: &Ty, var_id: u32) -> bool {
        let ty = self.resolve(ty);
        match ty.kind() {
            TyKind::Infer(crate::ty::InferTy(id)) => *id == var_id,
            TyKind::Tuple(items) => {
                let items = items.clone();
                items.iter().any(|t| self.ty_contains_infer_var(t, var_id))
            }
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => {
                let params = params.clone();
                let ret = ret.clone();
                params.iter().any(|t| self.ty_contains_infer_var(t, var_id))
                    || self.ty_contains_infer_var(&ret, var_id)
            }
            TyKind::App { args, .. } => {
                let args = args.clone();
                args.iter().any(|a| match a {
                    TyArg::Type(t) => self.ty_contains_infer_var(t, var_id),
                    _ => false,
                })
            }
            TyKind::Exist { inner, .. } => {
                let inner = inner.clone();
                self.ty_contains_infer_var(&inner, var_id)
            }
            TyKind::Bidir { lhs, rhs } => {
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                self.ty_contains_infer_var(&lhs, var_id) || self.ty_contains_infer_var(&rhs, var_id)
            }
            _ => false,
        }
    }

    /// Unify two types, with Infer binding and coercion fallback.
    pub fn unify(&mut self, t1: &Ty, t2: &Ty) -> bool {
        let t1 = self.shallow_resolve(t1);
        let t2 = self.shallow_resolve(t2);
        let t1 = self.normalize_alias_ty(&t1);
        let t2 = self.normalize_alias_ty(&t2);

        // Error types are transparent — always unify successfully
        // without binding. This prevents `?0 = error` from cross-file
        // lookups poisoning downstream unification.
        if t1.is_error() || t2.is_error() {
            return true;
        }

        // Same inference variable on both sides → trivially equal
        match (t1.kind(), t2.kind()) {
            (TyKind::Infer(crate::ty::InferTy(a)), TyKind::Infer(crate::ty::InferTy(b)))
                if a == b =>
            {
                return true
            }
            _ => {}
        }

        // Infer on either side → bind
        match (t1.kind(), t2.kind()) {
            (TyKind::Infer(crate::ty::InferTy(id)), _) => return self.bind(*id, t2),
            (_, TyKind::Infer(crate::ty::InferTy(id))) => return self.bind(*id, t1),
            _ => {}
        }

        // Existential type unwrapping: fresh-substitute bound vars
        // with InferTy, then unify the inner type.
        match (t1.kind(), t2.kind()) {
            (TyKind::Exist { vars, constraint, inner }, _) => {
                let unwrapped = self.freshen_existential(vars, constraint, inner);
                return self.unify(&unwrapped, &t2);
            }
            (_, TyKind::Exist { vars, constraint, inner }) => {
                let unwrapped = self.freshen_existential(vars, constraint, inner);
                return self.unify(&t1, &unwrapped);
            }
            _ => {}
        }

        // Sail: `(T)` is equivalent to `T` — unwrap before unification.
        match (t1.kind(), t2.kind()) {
            (TyKind::Tuple(items), _) if items.len() == 1 => {
                return self.unify(&items[0], &t2);
            }
            (_, TyKind::Tuple(items)) if items.len() == 1 => {
                return self.unify(&t1, &items[0]);
            }
            _ => {}
        }

        // Structural unification
        if self.unify_structural(&t1, &t2, 0) {
            return true;
        }

        // Coercion fallback
        if coerce::try_coerce(self, &t1, &t2).is_ok() {
            return true;
        }

        // Alias expansion as last resort. Only tried when normal
        // unification + coercion both fail, to avoid regressions from
        // always expanding (config-dependent aliases become error types).
        {
            let t1_name = match t1.kind() {
                TyKind::Adt(n, _) | TyKind::App { name: n, .. } => Some(n.as_str()),
                _ => None,
            };
            let t2_name = match t2.kind() {
                TyKind::Adt(n, _) | TyKind::App { name: n, .. } => Some(n.as_str()),
                _ => None,
            };
            // Only try if at least one side is a named type that differs
            if t1_name.is_some() || t2_name.is_some() {
                // Try expanding t1 (local alias map first)
                let t1_exp = if t1_name.is_some() {
                    let exp = self.normalize_alias_ty(&t1);
                    if exp != t1 {
                        Some(exp)
                    } else {
                        None
                    }
                } else {
                    None
                };
                // Try expanding t2 (local alias map first)
                let t2_exp = if t2_name.is_some() {
                    let exp = self.normalize_alias_ty(&t2);
                    if exp != t2 {
                        Some(exp)
                    } else {
                        None
                    }
                } else {
                    None
                };
                // Cross-file alias expansion via db is deferred to
                // apply_to (safe aliases only). See workspace.rs.
                // Re-try unification with expanded forms
                if t1_exp.is_some() || t2_exp.is_some() {
                    let t1_use = t1_exp.as_ref().unwrap_or(&t1);
                    let t2_use = t2_exp.as_ref().unwrap_or(&t2);
                    if self.unify_structural(t1_use, t2_use, 0) {
                        return true;
                    }
                    if coerce::try_coerce(self, t1_use, t2_use).is_ok() {
                        return true;
                    }
                }
            }
        }

        tracing::debug!(
            t1 = %t1.display_text(),
            t2 = %t2.display_text(),
            alias_count = self.type_aliases.len(),
            "unification failed"
        );
        false
    }

    /// Structural unification without coercion fallback.
    pub fn raw_unify(&mut self, t1: &Ty, t2: &Ty) -> bool {
        let t1 = self.shallow_resolve(t1);
        let t2 = self.shallow_resolve(t2);
        let t1 = self.normalize_alias_ty(&t1);
        let t2 = self.normalize_alias_ty(&t2);

        match (t1.kind(), t2.kind()) {
            (TyKind::Infer(crate::ty::InferTy(id)), _) => return self.bind(*id, t2),
            (_, TyKind::Infer(crate::ty::InferTy(id))) => return self.bind(*id, t1),
            _ => {}
        }

        self.unify_structural(&t1, &t2, 0)
    }

    /// Save current state for speculative unification.
    pub fn snapshot(&mut self) -> InferenceTableSnapshot {
        InferenceTableSnapshot {
            ena_snapshot: self.table.snapshot(),
            obligations_len: self.pending_obligations.len(),
            var_count: self.var_count,
        }
    }

    /// Restore to a previous snapshot, undoing speculative bindings.
    /// Also restores var_count so it stays in sync with ena's table size.
    pub fn rollback_to(&mut self, snapshot: InferenceTableSnapshot) {
        self.table.rollback_to(snapshot.ena_snapshot);
        self.pending_obligations.truncate(snapshot.obligations_len);
        self.var_count = snapshot.var_count;
    }

    /// Try unification speculatively. If it fails, the table is unchanged.
    pub(super) fn try_unify(&mut self, t1: &Ty, t2: &Ty) -> bool {
        let snap = self.snapshot();
        if self.unify(t1, t2) {
            true
        } else {
            self.rollback_to(snap);
            false
        }
    }

    /// Register a freshened placeholder name → InferTy id mapping.
    pub fn register_placeholder(&mut self, placeholder: &str, infer_id: u32) {
        self.placeholder_to_infer.insert(placeholder.to_string(), infer_id);
    }

    /// Deep-resolve a type, following all InferTy bindings recursively.
    pub fn deep_resolve(&mut self, ty: &Ty) -> Ty {
        let resolved = self.resolve(ty);
        match resolved.kind() {
            TyKind::Infer(_) => resolved,
            TyKind::Error | TyKind::Scalar(_) => resolved,
            TyKind::Param(name) => {
                // Check if this Param name is a placeholder with a known InferTy.
                if let Some(&infer_id) = self.placeholder_to_infer.get(name.as_str()) {
                    let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
                    if !matches!(bound.kind(), TyKind::Infer(_)) {
                        return bound;
                    }
                }
                resolved
            }
            TyKind::Adt(_, _) => resolved,
            TyKind::App { name, args, .. } => {
                let new_args: Vec<TyArg> = args
                    .iter()
                    .map(|arg| match arg {
                        TyArg::Type(t) => TyArg::Type(self.deep_resolve(t)),
                        TyArg::Nexp(nexp) => self.resolve_nexp_arg(nexp),
                        TyArg::Value(v) => self.resolve_value_arg(v),
                    })
                    .collect();
                let text = super::numeric::app_text(name, &new_args);
                Ty::app(name, new_args, text)
            }
            TyKind::Tuple(items) => Ty::tuple(items.iter().map(|t| self.deep_resolve(t)).collect()),
            TyKind::FnPtr(crate::ty::FnSig { params, ret }) => Ty::function(
                params.iter().map(|p| self.deep_resolve(p)).collect(),
                self.deep_resolve(ret),
            ),
            TyKind::Exist { vars, constraint, inner } => {
                Ty::exist(vars.clone(), constraint.clone(), self.deep_resolve(inner))
            }
            TyKind::Bidir { lhs, rhs } => Ty::bidir(self.deep_resolve(lhs), self.deep_resolve(rhs)),
            TyKind::Abstract { .. } => resolved,
        }
    }

    /// Resolve a `TyArg::Nexp` by checking if any Var in the expression
    /// is a registered placeholder with a resolved InferTy.
    fn resolve_nexp_arg(&mut self, nexp: &NumericExpr) -> TyArg {
        match nexp {
            NumericExpr::Var(name) | NumericExpr::Symbol(name) => {
                if let Some(&infer_id) = self.placeholder_to_infer.get(name.as_str()) {
                    let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
                    if !matches!(bound.kind(), TyKind::Infer(_)) {
                        return TyArg::numeric(bound.display_text());
                    }
                }
                TyArg::Nexp(nexp.clone())
            }
            // For compound expressions, substitute recursively.
            NumericExpr::Add(a, b) => {
                let a = self.resolve_nexp_to_expr(a);
                let b = self.resolve_nexp_to_expr(b);
                TyArg::Nexp(NumericExpr::Add(Box::new(a), Box::new(b)))
            }
            NumericExpr::Sub(a, b) => {
                let a = self.resolve_nexp_to_expr(a);
                let b = self.resolve_nexp_to_expr(b);
                TyArg::Nexp(NumericExpr::Sub(Box::new(a), Box::new(b)))
            }
            NumericExpr::Mul(a, b) => {
                let a = self.resolve_nexp_to_expr(a);
                let b = self.resolve_nexp_to_expr(b);
                TyArg::Nexp(NumericExpr::Mul(Box::new(a), Box::new(b)))
            }
            _ => TyArg::Nexp(nexp.clone()),
        }
    }

    /// Resolve a NumericExpr, returning a NumericExpr with placeholders resolved.
    fn resolve_nexp_to_expr(&mut self, nexp: &NumericExpr) -> NumericExpr {
        match nexp {
            NumericExpr::Var(name) | NumericExpr::Symbol(name) => {
                if let Some(&infer_id) = self.placeholder_to_infer.get(name.as_str()) {
                    let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
                    if !matches!(bound.kind(), TyKind::Infer(_)) {
                        if let Some(parsed) = NumericExpr::parse(&bound.display_text()) {
                            return parsed;
                        }
                    }
                }
                nexp.clone()
            }
            _ => nexp.clone(),
        }
    }

    /// Resolve a `TyArg::Value` string by checking for placeholder names.
    fn resolve_value_arg(&mut self, value: &str) -> TyArg {
        if let Some(&infer_id) = self.placeholder_to_infer.get(value) {
            let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
            if !matches!(bound.kind(), TyKind::Infer(_)) {
                return TyArg::numeric(bound.display_text());
            }
        }
        // Try substring replacement for compound expressions like "2 * '_fv0".
        let mut result = value.to_string();
        for (placeholder, &infer_id) in &self.placeholder_to_infer.clone() {
            if result.contains(placeholder.as_str()) {
                let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
                if !matches!(bound.kind(), TyKind::Infer(_)) {
                    result = result.replace(placeholder.as_str(), &bound.display_text());
                }
            }
        }
        if result != value {
            TyArg::numeric(result)
        } else {
            TyArg::Value(value.to_string())
        }
    }

    /// Structural unification with InferTy binding (permissive semantics).
    pub(super) fn unify_structural(&mut self, expected: &Ty, actual: &Ty, depth: usize) -> bool {
        const DEPTH_LIMIT: usize = 96;
        if depth > DEPTH_LIMIT {
            return true; // permissive on deep recursion
        }
        if actual.is_error() {
            return true;
        }
        // Existential on actual side — unwrap with witness extraction.
        if let TyKind::Exist { vars, constraint, inner } = actual.kind() {
            let vars = vars.clone();
            let constraint = constraint.clone();
            let inner = inner.clone();
            let ok = self.unify_structural(expected, &inner, depth + 1);
            if ok && !vars.is_empty() {
                use super::existential;
                let mut table = super::InferenceTable::default();
                match existential::extract_witnesses(
                    &vars,
                    &constraint,
                    &inner,
                    expected,
                    &mut table,
                ) {
                    existential::WitnessResult::ConstraintViolation { .. } => {
                        return false;
                    }
                    _ => {}
                }
            }
            return ok;
        }
        match expected.kind() {
            TyKind::Error | TyKind::Infer(crate::ty::InferTy(_)) => true,
            TyKind::Param(name) => {
                // Param in expected position: accept (Param binding is handled
                // by the caller via freshen/apply_subst, not during structural unification).
                if matches!(actual.kind(), TyKind::Param(actual_name) if actual_name == name) {
                    return true;
                }
                // If actual also contains this param, reject (avoid occurs-check).
                if super::ty_contains_var(actual, name) {
                    return false;
                }
                true // permissive — Param can unify with anything
            }
            TyKind::Scalar(expected_scalar) => {
                if let TyKind::Scalar(actual_scalar) = actual.kind() {
                    return expected_scalar == actual_scalar;
                }
                if *expected_scalar == crate::ty::Scalar::Bit {
                    if let Some(width) = constraint::bits_width(actual) {
                        if width.trim() == "1" {
                            return true;
                        }
                    }
                }
                if let Some(actual_name) = actual.as_name() {
                    return expected_scalar.name() == actual_name;
                }
                false
            }
            TyKind::Adt(expected, _) => {
                if matches!(actual.kind(), TyKind::Tuple(_) | TyKind::FnPtr(..)) {
                    return false;
                }
                if let Some(actual_name) = actual.as_name() {
                    if expected == actual_name {
                        return true;
                    }
                }
                if let TyKind::Adt(actual_text, _) = actual.kind() {
                    if expected == actual_text {
                        return true;
                    }
                    if (expected == "bit" && actual_text == "bits(1)")
                        || (expected == "bits(1)" && actual_text == "bit")
                    {
                        return true;
                    }
                    let primitives = ["int", "nat", "bool", "string", "unit", "real", "bit", "_"];
                    let exp_prim = primitives.contains(&expected.as_str());
                    let act_prim = primitives.contains(&actual_text.as_str());
                    if exp_prim
                        && act_prim
                        && !(constraint::is_numeric_text(expected)
                            && constraint::is_numeric_text(actual_text))
                    {
                        return false;
                    }
                }
                if constraint::is_numeric_text(expected) && constraint::is_numeric_scalar_ty(actual)
                {
                    return true;
                }
                if expected == "bit" {
                    if let Some(width) = constraint::bits_width(actual) {
                        return width.trim() == "1";
                    }
                }
                if expected.starts_with("bits(") && expected.ends_with(')') {
                    let expected_width = &expected["bits(".len()..expected.len() - 1];
                    if let Some(actual_width) = constraint::bits_width(actual) {
                        let exp_num = expected_width.parse::<i64>().ok();
                        let act_num = actual_width.parse::<i64>().ok();
                        return match (exp_num, act_num) {
                            (Some(a), Some(b)) => a == b,
                            _ => true,
                        };
                    }
                }
                let primitives = ["int", "nat", "bool", "string", "unit", "real", "bit", "_"];
                let is_expected_primitive = primitives.contains(&expected.as_str());
                let is_actual_primitive = constraint::is_primitive_or_bits_ty(actual);
                if !is_expected_primitive || !is_actual_primitive {
                    return true;
                }
                false
            }
            TyKind::Tuple(expected_items) => match actual.kind() {
                TyKind::Tuple(actual_items) if expected_items.len() == actual_items.len() => {
                    let pairs: Vec<_> = expected_items
                        .iter()
                        .zip(actual_items.iter())
                        .map(|(e, a)| (e.clone(), a.clone()))
                        .collect();
                    pairs.iter().all(|(e, a)| self.unify_structural(e, a, depth + 1))
                }
                _ => false,
            },
            TyKind::FnPtr(crate::ty::FnSig { params: expected_params, ret: expected_ret }) => {
                match actual.kind() {
                    TyKind::FnPtr(crate::ty::FnSig { params: actual_params, ret: actual_ret })
                        if expected_params.len() == actual_params.len() =>
                    {
                        let expected_ret = expected_ret.clone();
                        let actual_ret = actual_ret.clone();
                        let pairs: Vec<_> = expected_params
                            .iter()
                            .zip(actual_params.iter())
                            .map(|(e, a)| (e.clone(), a.clone()))
                            .collect();
                        pairs.iter().all(|(e, a)| self.unify_structural(e, a, depth + 1))
                            && self.unify_structural(&expected_ret, &actual_ret, depth + 1)
                    }
                    _ => false,
                }
            }
            TyKind::App { name: expected_name, args: expected_args, .. } => {
                let expected_name = expected_name.clone();
                let expected_args = expected_args.clone();
                // Numeric permissive check — bind any freshened quantifier args first
                if matches!(expected_name.as_str(), "range" | "atom" | "int" | "nat")
                    && constraint::is_numeric_scalar_ty(actual)
                {
                    self.bind_numeric_args(&expected_args);
                    return true;
                }
                // bit ≡ bits(1)
                if expected_name == "bits"
                    && matches!(actual.kind(), TyKind::Scalar(crate::ty::Scalar::Bit))
                {
                    if let Some(width) = expected_args.first().and_then(|a| a.as_value_str()) {
                        if width.trim() == "1" {
                            return true;
                        }
                    }
                }
                // bits('n) ≡ bitvector('n, 'ord) — fundamental Sail type equivalence.
                // Sail prelude: `type bits('n) = bitvector('n, dec)`.
                // The second arg ('ord) is the order parameter (dec/inc);
                // only fires when names DIFFER (bits vs bitvector), not
                // for same-name width mismatches like bits(8) vs bits(16).
                if let TyKind::App { name: actual_name, .. } = actual.kind() {
                    if (expected_name == "bits" && actual_name == "bitvector")
                        || (expected_name == "bitvector" && actual_name == "bits")
                    {
                        return true;
                    }
                }
                // bits(N) ≡ vector(N, bit)
                if expected_name == "bits" {
                    if let TyKind::App { name: a_name, args: a_args, .. } = actual.kind() {
                        if a_name == "vector" {
                            let actual_n = a_args.first().and_then(|a| a.as_value_str());
                            let elem = a_args.get(1).and_then(|a| {
                                if let TyArg::Type(t) = a {
                                    Some(t.clone())
                                } else {
                                    None
                                }
                            });
                            if let (Some(actual_n), Some(elem)) = (actual_n, elem) {
                                if matches!(elem.kind(), TyKind::Scalar(crate::ty::Scalar::Bit)) {
                                    if let Some(expected_n) =
                                        expected_args.first().and_then(|a| a.as_value_str())
                                    {
                                        let en = super::normalized_value_text(&expected_n);
                                        let an = super::normalized_value_text(&actual_n);
                                        return en == an
                                            || expected_n.starts_with('\'')
                                            || actual_n.starts_with('\'');
                                    }
                                }
                            }
                        }
                    }
                }
                if expected_name == "vector" {
                    if let Some(actual_n) = constraint::bits_width(actual) {
                        if let Some(expected_n) =
                            expected_args.first().and_then(|a| a.as_value_str())
                        {
                            if let Some(TyArg::Type(elem)) = expected_args.get(1) {
                                if matches!(elem.kind(), TyKind::Scalar(crate::ty::Scalar::Bit)) {
                                    let en = super::normalized_value_text(&expected_n);
                                    let an = super::normalized_value_text(&actual_n);
                                    return en == an
                                        || expected_n.starts_with('\'')
                                        || actual_n.starts_with('\'');
                                }
                            }
                        }
                    }
                }
                match actual.kind() {
                    TyKind::App { name: actual_name, args: actual_args, .. }
                        if expected_name == *actual_name
                            && expected_args.len() == actual_args.len() =>
                    {
                        let pairs: Vec<_> = expected_args
                            .iter()
                            .zip(actual_args.iter())
                            .map(|(e, a)| (e.clone(), a.clone()))
                            .collect();
                        pairs.iter().all(|(e, a)| self.unify_ty_arg(e, a, depth))
                    }
                    _ => {
                        // On-demand alias expansion: when the same-name
                        // fast path fails, try substituting actual args
                        // into either side's alias body and retry once.
                        // Config-dependent aliases are filtered out to
                        // avoid poisoning unification with error types.
                        if let Some(sch) = self.alias_scheme(&expected_name) {
                            if let Some(expanded) = sch.substitute(&expected_args) {
                                if self.unify_structural(&expanded, actual, depth + 1) {
                                    return true;
                                }
                            }
                        }
                        if let TyKind::App { name: a_name, args: a_args, .. } = actual.kind() {
                            let a_name = a_name.clone();
                            let a_args = a_args.clone();
                            if let Some(sch) = self.alias_scheme(&a_name) {
                                if let Some(expanded) = sch.substitute(&a_args) {
                                    let expected_ty =
                                        Ty::app(expected_name.clone(), expected_args.clone(), "");
                                    if self.unify_structural(&expected_ty, &expanded, depth + 1) {
                                        return true;
                                    }
                                }
                            }
                        }
                        false
                    }
                }
            }
            // Existential in expected position
            TyKind::Exist { vars, constraint, inner } => {
                let vars = vars.clone();
                let constraint = constraint.clone();
                let inner = inner.clone();
                let ok = self.unify_structural(&inner, actual, depth + 1);
                if ok && !vars.is_empty() {
                    use super::existential;
                    let mut table = super::InferenceTable::default();
                    match existential::extract_witnesses(
                        &vars,
                        &constraint,
                        &inner,
                        actual,
                        &mut table,
                    ) {
                        existential::WitnessResult::ConstraintViolation { .. } => {
                            return false;
                        }
                        _ => {}
                    }
                }
                ok
            }
            // Bidirectional type
            TyKind::Bidir { lhs, rhs } => {
                let lhs = lhs.clone();
                let rhs = rhs.clone();
                match actual.kind() {
                    TyKind::Bidir { lhs: a_lhs, rhs: a_rhs } => {
                        let a_lhs = a_lhs.clone();
                        let a_rhs = a_rhs.clone();
                        self.unify_structural(&lhs, &a_lhs, depth + 1)
                            && self.unify_structural(&rhs, &a_rhs, depth + 1)
                    }
                    _ => false,
                }
            }
            TyKind::Abstract { name, .. } => {
                matches!(actual.kind(), TyKind::Abstract { name: n, .. } if n == name)
            }
        }
    }

    /// Unify two TyArgs structurally. Used by `unify_structural` for App args.
    fn unify_ty_arg(&mut self, a1: &TyArg, a2: &TyArg, depth: usize) -> bool {
        match (a1, a2) {
            (TyArg::Type(t1), TyArg::Type(t2)) => self.unify_structural(t1, t2, depth + 1),
            (a, b) => {
                let s1 = a.as_value_str();
                let s2 = b.as_value_str();
                match (s1, s2) {
                    (Some(v1), Some(v2)) => {
                        if super::normalized_value_text(&v1) == super::normalized_value_text(&v2) {
                            return true;
                        }
                        // If either contains a type variable (apostrophe prefix),
                        // we can't decide without a solver — be permissive.
                        if v1.contains('\'') || v2.contains('\'') {
                            return true;
                        }
                        // Try algebraic comparison by parsing both as numeric expressions.
                        // This handles cases like "16" == "(8 * 2)" where apply_subst
                        // produced a complex string that couldn't be re-parsed to Nexp.
                        use super::nexp_simplify;
                        use crate::ty::NumericExpr;
                        let parse_any = |s: &str| -> Option<NumericExpr> {
                            // Try direct parse first, then use parse_numeric_expr_text
                            NumericExpr::parse(s).or_else(|| super::parse_numeric_expr_text(s))
                        };
                        if let (Some(n1), Some(n2)) = (parse_any(&v1), parse_any(&v2)) {
                            match nexp_simplify::check_eq(&n1, &n2) {
                                Some(result) => return result,
                                None => {} // undecidable — fall through to permissive
                            }
                        }
                        // Final fallback: permissive (can't decide)
                        true
                    }
                    _ => true, // permissive
                }
            }
        }
    }

    /// Bind any InferTy vars in `args` to `int` (numeric permissive fallback).
    fn bind_numeric_args(&mut self, args: &[TyArg]) {
        for arg in args {
            if let TyArg::Type(ty) = arg {
                if let TyKind::Infer(crate::ty::InferTy(id)) = ty.kind() {
                    let _ = self.bind(*id, Ty::named("int"));
                }
            }
        }
    }
}

/// Snapshot of InferenceTable state for speculative unification.
pub struct InferenceTableSnapshot {
    ena_snapshot: ut::Snapshot<InPlace<TyVarKey>>,
    obligations_len: usize,
    var_count: u32,
}

/// Substitute type parameters with replacements.
fn substitute_params(ty: &Ty, subst: &HashMap<String, Ty>) -> Ty {
    match ty.kind() {
        TyKind::Param(name) => {
            if let Some(replacement) = subst.get(name.as_str()) {
                replacement.clone()
            } else {
                // Also try without leading apostrophe
                let bare = name.strip_prefix('\'').unwrap_or(name);
                subst.get(bare).cloned().unwrap_or_else(|| ty.clone())
            }
        }
        TyKind::App { name, args, text } => {
            let new_args: Vec<TyArg> = args
                .iter()
                .map(|a| match a {
                    TyArg::Type(t) => TyArg::Type(substitute_params(t, subst)),
                    other => other.clone(),
                })
                .collect();
            Ty::app(name, new_args, text)
        }
        TyKind::Tuple(items) => {
            Ty::tuple(items.iter().map(|t| substitute_params(t, subst)).collect())
        }
        TyKind::Exist { vars, constraint, inner } => {
            // Don't substitute into shadowed vars
            let filtered: HashMap<String, Ty> = subst
                .iter()
                .filter(|(k, _)| !vars.contains(k))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            Ty::exist(vars.clone(), constraint.clone(), substitute_params(inner, &filtered))
        }
        _ => ty.clone(),
    }
}

/// Substitute type variables in a constraint expression.
fn substitute_constraint(
    constraint: &ConstraintExpr,
    subst: &HashMap<String, Ty>,
) -> ConstraintExpr {
    match constraint {
        ConstraintExpr::Bool(_) | ConstraintExpr::Unsupported => constraint.clone(),
        ConstraintExpr::Compare { lhs, op, rhs } => ConstraintExpr::Compare {
            lhs: substitute_nexp(lhs, subst),
            op: *op,
            rhs: substitute_nexp(rhs, subst),
        },
        ConstraintExpr::InSet { value, items } => ConstraintExpr::InSet {
            value: substitute_nexp(value, subst),
            items: items.iter().map(|i| substitute_nexp(i, subst)).collect(),
        },
        ConstraintExpr::And(parts) => {
            ConstraintExpr::And(parts.iter().map(|p| substitute_constraint(p, subst)).collect())
        }
        ConstraintExpr::Or(parts) => {
            ConstraintExpr::Or(parts.iter().map(|p| substitute_constraint(p, subst)).collect())
        }
        ConstraintExpr::Not(inner) => {
            ConstraintExpr::Not(Box::new(substitute_constraint(inner, subst)))
        }
        ConstraintExpr::App { name, args } => ConstraintExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| substitute_constraint(a, subst)).collect(),
        },
        ConstraintExpr::BoolVar(v) => {
            if subst.contains_key(v.as_str()) {
                ConstraintExpr::Bool(true) // Bool var replaced — permissive.
            } else {
                constraint.clone()
            }
        }
    }
}

/// Substitute type variables in a numeric expression.
fn substitute_nexp(nexp: &NumericExpr, subst: &HashMap<String, Ty>) -> NumericExpr {
    match nexp {
        NumericExpr::Var(name) => {
            if let Some(ty) = subst.get(name.as_str()) {
                NumericExpr::Symbol(ty.display_text())
            } else {
                let bare = name.strip_prefix('\'').unwrap_or(name);
                if let Some(ty) = subst.get(bare) {
                    NumericExpr::Symbol(ty.display_text())
                } else {
                    nexp.clone()
                }
            }
        }
        NumericExpr::Add(a, b) => NumericExpr::Add(
            Box::new(substitute_nexp(a, subst)),
            Box::new(substitute_nexp(b, subst)),
        ),
        NumericExpr::Sub(a, b) => NumericExpr::Sub(
            Box::new(substitute_nexp(a, subst)),
            Box::new(substitute_nexp(b, subst)),
        ),
        NumericExpr::Mul(a, b) => NumericExpr::Mul(
            Box::new(substitute_nexp(a, subst)),
            Box::new(substitute_nexp(b, subst)),
        ),
        NumericExpr::Div(a, b) => NumericExpr::Div(
            Box::new(substitute_nexp(a, subst)),
            Box::new(substitute_nexp(b, subst)),
        ),
        NumericExpr::Mod(a, b) => NumericExpr::Mod(
            Box::new(substitute_nexp(a, subst)),
            Box::new(substitute_nexp(b, subst)),
        ),
        NumericExpr::Neg(e) => NumericExpr::Neg(Box::new(substitute_nexp(e, subst))),
        NumericExpr::Exp(e) => NumericExpr::Exp(Box::new(substitute_nexp(e, subst))),
        NumericExpr::App { name, args } => NumericExpr::App {
            name: name.clone(),
            args: args.iter().map(|a| substitute_nexp(a, subst)).collect(),
        },
        NumericExpr::If { cond, then_expr, else_expr } => NumericExpr::If {
            cond: Box::new(substitute_constraint(cond, subst)),
            then_expr: Box::new(substitute_nexp(then_expr, subst)),
            else_expr: Box::new(substitute_nexp(else_expr, subst)),
        },
        _ => nexp.clone(),
    }
}
