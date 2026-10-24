//! Type unification and inference variable management.
//!
//! Uses ena union-find with snapshot/rollback for speculative unification.

use std::collections::HashMap;

use ena::unify::{self as ut, InPlace, NoError, UnifyKey, UnifyValue};

use super::coerce;
use super::constraint;
use super::{Obligation, ScopedObligation, Subst, Ty, TyArg, TyKind};
use crate::ty::{ConstraintExpr, NumericExpr};

/// Directional compatibility separates proof from pending goals and recovery.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compatibility {
    Proven,
    Deferred,
    Recovery,
    Mismatch,
}
impl Compatibility {
    pub fn is_compatible(self) -> bool {
        self != Self::Mismatch
    }
}

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

/// Whether a vector element type is `bit`, or not yet known.
fn is_bit_or_unresolved(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::Scalar(crate::ty::Scalar::Bit) | TyKind::Error | TyKind::Infer(_))
}

/// Manages inference variables during type inference.
#[derive(Debug)]
pub struct InferenceTable {
    pub(super) table: ut::UnificationTable<InPlace<TyVarKey>>,
    pub(super) var_count: u32,
    universe: u32,
    next_universe: u32,
    variable_universes: Vec<u32>,
    universe_undo: Vec<(u32, u32)>,
    rigid_variables: HashMap<String, u32>,
    rigid_order: Vec<String>,
    reserved_names: std::collections::HashSet<String>,
    rigid_predicates: HashMap<String, std::sync::Arc<ConstraintExpr>>,
    pending_obligations: Vec<ScopedObligation>,
    type_aliases: HashMap<String, Ty>,
    alias_schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    /// Freshened placeholder names → InferTy ids for deep_resolve.
    placeholder_to_infer: HashMap<String, u32>,
    // Undo only names changed during speculation, rather than cloning the map.
    placeholder_undo: Vec<(String, Option<u32>)>,
    snapshot_depth: usize,
    /// Type names declared by a `struct`/`union`/`enum`/`bitfield`.
    ///
    /// A `Named` is also how a type alias arrives, and an alias may stand
    /// for anything — which is why the tail of the `Named` arm below is
    /// permissive. A nominal type is not an alias: two different ones never
    /// unify, and none of them is a primitive.
    nominal_types: std::collections::HashSet<String>,
}

impl Default for InferenceTable {
    fn default() -> Self {
        Self {
            table: ut::UnificationTable::new(),
            var_count: 0,
            universe: 0,
            next_universe: 0,
            variable_universes: Vec::new(),
            universe_undo: Vec::new(),
            rigid_variables: HashMap::new(),
            rigid_order: Vec::new(),
            reserved_names: Default::default(),
            rigid_predicates: HashMap::new(),
            pending_obligations: Vec::new(),
            type_aliases: HashMap::new(),
            alias_schemes: HashMap::new(),
            placeholder_to_infer: HashMap::new(),
            placeholder_undo: Vec::new(),
            snapshot_depth: 0,
            nominal_types: std::collections::HashSet::new(),
        }
    }
}

impl InferenceTable {
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a table with type alias context for normalization.
    pub(super) fn with_aliases(type_aliases: HashMap<String, Ty>) -> Self {
        Self { type_aliases, ..Self::default() }
    }

    pub(super) fn set_nominal_types(&mut self, names: std::collections::HashSet<String>) {
        self.nominal_types = names;
    }

    pub(super) fn set_alias_schemes(
        &mut self,
        schemes: HashMap<String, std::sync::Arc<super::env::AliasScheme>>,
    ) {
        self.alias_schemes = schemes;
    }

    /// Look up an alias scheme; configuration values remain symbolic.
    /// Expand a bare `Named` alias once.
    ///
    /// `type bits_H = bits(16)` reaches a candidate's argument list as
    /// `Named("bits_H")`, and the substitution the constraint check reads is
    /// built by a *different* unifier — the one in `constraint.rs`, which
    /// has no aliases to consult. Expanding here feeds both.
    pub(super) fn expand_named_alias(&self, ty: &Ty) -> Ty {
        let TyKind::Named(name, args) = ty.kind() else { return ty.clone() };
        let Some(sch) = self.alias_scheme(name) else { return ty.clone() };
        sch.substitute(args).unwrap_or_else(|| ty.clone())
    }

    pub(super) fn alias_scheme(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<super::env::AliasScheme>> {
        let sch = self.alias_schemes.get(name)?.clone();
        Some(sch)
    }

    /// Shape consumers share the same symbolic expansion as subtype checking.
    /// An unknown configuration width remains a symbol, never a guessed value.
    pub(super) fn alias_scheme_for_shape(
        &self,
        name: &str,
    ) -> Option<std::sync::Arc<super::env::AliasScheme>> {
        self.alias_schemes.get(name).cloned()
    }

    /// [`Self::normalize_alias_deep`], including aliases whose width is
    /// configured. For deciding what a type *is*, never for proving.
    pub(super) fn normalize_alias_deep_for_shape(&self, ty: &Ty) -> Ty {
        let mut current = ty.clone();
        let mut seen = std::collections::HashSet::new();
        loop {
            let name = match current.kind() {
                TyKind::Named(n, _) => n.clone(),
                TyKind::App { name: n, .. } => n.clone(),
                _ => return current,
            };
            if !seen.insert(name.clone()) {
                return current;
            }
            let Some(sch) = self.alias_scheme_for_shape(&name) else { return current };
            let args = match current.kind() {
                TyKind::App { args, .. } | TyKind::Named(_, args) => args.clone(),
                _ => Vec::new(),
            };
            let Some(expanded) = sch.substitute(&args) else { return current };
            current = expanded;
        }
    }

    /// One step of alias expansion, config-dependent aliases included.
    ///
    /// [`Self::normalize_alias_deep_for_shape`] runs the same step to a fixed
    /// point, which answers "is this a bitvector" and nothing finer. A caller
    /// that wants the *width* has to stop at the level where the width is
    /// still written down — `flenbits` is `bits(flen)` after one step — so it
    /// takes the steps itself.
    pub(super) fn expand_alias_once_for_shape(&self, ty: &Ty) -> Option<Ty> {
        let name = match ty.kind() {
            TyKind::Named(n, _) | TyKind::App { name: n, .. } => n.clone(),
            _ => return None,
        };
        let sch = self.alias_scheme_for_shape(&name)?;
        let args = match ty.kind() {
            TyKind::App { args, .. } | TyKind::Named(_, args) => args.clone(),
            _ => Vec::new(),
        };
        let expanded = sch.substitute(&args)?;
        (expanded != *ty).then_some(expanded)
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
        let mut current = applied_form(ty);
        let mut seen = std::collections::HashSet::new();
        loop {
            let name = match current.kind() {
                TyKind::Named(n, _) => n.clone(),
                TyKind::App { name: n, .. } => n.clone(),
                _ => return current,
            };
            if !seen.insert(name.clone()) {
                return current; // cycle detected
            }
            if !self.type_aliases.contains_key(&name) {
                return current;
            }
            // Never expand a parameterized alias using its uninstantiated
            // body from type_aliases: doing so discards the actual arguments.
            let expanded = if let Some(scheme) = self.alias_schemes.get(&name) {
                let args = match current.kind() {
                    TyKind::Named(_, args) | TyKind::App { args, .. } => args,
                    _ => unreachable!(),
                };
                scheme.substitute(args)
            } else {
                self.type_aliases.get(&name).cloned()
            };
            match expanded {
                Some(expanded) => current = applied_form(&expanded),
                None => return current,
            }
        }
    }

    /// Expand aliases, following parameterised schemes as well.
    ///
    /// Separate from `normalize_alias_ty` on purpose. Schemes cover every
    /// alias whose declaration mentions a type variable — including
    /// `type xlenbits = bits(xlen)`, because extracting formal parameters
    /// routes all of them through the scheme path — and expanding them
    /// inside unification changed enough widths to turn eleven sail-riscv
    /// files red. Callers that only want to *classify* a type can use this.
    pub(super) fn normalize_alias_deep(&self, ty: &Ty) -> Ty {
        let mut current = self.normalize_alias_ty(ty);
        let mut seen = std::collections::HashSet::new();
        loop {
            let name = match current.kind() {
                TyKind::Named(n, _) => n.clone(),
                TyKind::App { name: n, .. } => n.clone(),
                _ => return current,
            };
            if !seen.insert(name.clone()) {
                return current;
            }
            let Some(sch) = self.alias_scheme(&name) else { return current };
            let args = match current.kind() {
                TyKind::App { args, .. } | TyKind::Named(_, args) => args.clone(),
                _ => Vec::new(),
            };
            let Some(expanded) = sch.substitute(&args) else { return current };
            current = self.normalize_alias_ty(&expanded);
        }
    }

    /// Queue a constraint obligation for later batch solving.
    pub fn push_obligation(&mut self, ob: Obligation) {
        self.push_obligation_with_cause(ob, super::ObligationCause::Explicit);
    }
    pub(super) fn push_subtype_obligation(&mut self, ob: Obligation) {
        self.push_obligation_with_cause(ob, super::ObligationCause::Compatibility);
    }
    fn push_obligation_with_cause(&mut self, ob: Obligation, cause: super::ObligationCause) {
        match ob {
            Obligation::All(goals) => {
                for goal in goals {
                    self.push_obligation_with_cause(goal, cause);
                }
                return;
            }
            Obligation::Assuming { assumptions, goal } => {
                let start = self.pending_obligations.len();
                self.push_obligation_with_cause(*goal, cause);
                for ob in &mut self.pending_obligations[start..] {
                    let mut combined = ob.assumptions.to_vec();
                    combined.extend(assumptions.clone());
                    ob.assumptions = combined.into();
                }
                return;
            }
            _ => {}
        }
        let mut substitution = Subst::default();
        if !self.placeholder_to_infer.is_empty() {
            let mut names = std::collections::HashSet::new();
            super::substitution::constraint_names(&ob.constraint(), &mut names);
            for (name, id) in &self.placeholder_to_infer {
                if names.contains(name.strip_prefix('\'').unwrap_or(name)) {
                    substitution
                        .types
                        .insert(name.clone(), Ty::new(TyKind::Infer(crate::ty::InferTy(*id))));
                }
            }
        }
        let mut names = std::collections::HashSet::new();
        super::substitution::constraint_names(&ob.constraint(), &mut names);
        let mut rigid_variables: Vec<_> = self
            .rigid_variables
            .keys()
            .filter(|name| names.contains(name.trim_start_matches('\'')))
            .cloned()
            .collect();
        rigid_variables.sort();
        let universe = rigid_variables
            .iter()
            .filter_map(|n| self.rigid_variables.get(n))
            .copied()
            .max()
            .unwrap_or(self.universe);
        self.pending_obligations.push(ScopedObligation {
            goal: ob,
            universe,
            rigid_variables,
            cause,
            origin: None,
            assumptions: Default::default(),
            substitution,
            unknown_reason: None,
            recovery_types: None,
            compatibility_types: None,
        });
    }

    /// Take all pending obligations for batch solving.
    pub(super) fn drain_obligations(&mut self) -> Vec<ScopedObligation> {
        std::mem::take(&mut self.pending_obligations)
    }

    pub(super) fn obligation_count(&self) -> usize {
        self.pending_obligations.len()
    }
    pub(super) fn has_unscoped_obligations_since(&self, start: usize) -> bool {
        self.pending_obligations.iter().skip(start).any(|o| o.origin.is_none())
    }
    pub(super) fn scope_obligations_since(
        &mut self,
        start: usize,
        origin: hir_def::ExprId,
        assumptions: Vec<ConstraintExpr>,
    ) {
        let assumptions: std::sync::Arc<[ConstraintExpr]> = assumptions.into();
        for goal in self.pending_obligations.iter_mut().skip(start).filter(|o| o.origin.is_none()) {
            goal.origin = Some(origin);
            if goal.assumptions.is_empty() {
                goal.assumptions = assumptions.clone();
            } else {
                let mut combined = goal.assumptions.to_vec();
                combined.extend(assumptions.iter().cloned());
                goal.assumptions = combined.into();
            }
            let mut names = std::collections::HashSet::new();
            super::substitution::constraint_names(&goal.goal.constraint(), &mut names);
            for assumption in goal.assumptions.iter() {
                super::substitution::constraint_names(assumption, &mut names);
            }
            goal.rigid_variables = self
                .rigid_variables
                .keys()
                .filter(|n| names.contains(n.trim_start_matches('\'')))
                .cloned()
                .collect();
            goal.rigid_variables.sort();
            goal.universe = goal
                .rigid_variables
                .iter()
                .filter_map(|n| self.rigid_variables.get(n))
                .copied()
                .max()
                .unwrap_or(goal.universe);
        }
    }

    /// Create a fresh inference variable.
    pub fn new_type_var(&mut self) -> Ty {
        let key = self.table.new_key(TyVarValue::Unknown);
        self.var_count += 1;
        self.variable_universes.push(self.universe);
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
            // atom(N), range(lo,hi): the bounds may mention the callee's
            // quantifiers, which mean nothing here, so those become a fresh
            // var and the caller's context decides.
            //
            // Concrete bounds are not that. `range(0, 1)` says the same
            // thing in every scope, and erasing it threw away the only
            // thing that could disagree with the caller's annotation —
            // which is why no numeric range mismatch was ever reported.
            TyKind::App { name, args, text } if name == "atom" || name == "range" => {
                if ty.is_fully_known() {
                    ty.clone()
                } else {
                    // Freshen the *bounds*, not the type. Replacing the whole
                    // `range` with a variable erased the one thing that is
                    // true in every scope — that this is a number — so a
                    // cross-file `forall 'n. bits('n) -> range(0, 'n)` came
                    // back as an unbound variable and its call site unified
                    // with `bool`, with `string`, with anything at all. The
                    // `int`/`nat` arm below already did it this way.
                    let new_args: Vec<crate::ty::TyArg> = args
                        .iter()
                        .map(|a| match a {
                            crate::ty::TyArg::Type(t) => {
                                crate::ty::TyArg::Type(self.insert_type_vars(t))
                            }
                            crate::ty::TyArg::Value(v) if v.contains('\'') => {
                                crate::ty::TyArg::Type(self.new_type_var())
                            }
                            crate::ty::TyArg::Nexp(n) if n.to_string_repr().contains('\'') => {
                                crate::ty::TyArg::Type(self.new_type_var())
                            }
                            other => other.clone(),
                        })
                        .collect();
                    Ty::app(name.clone(), new_args, text.clone())
                }
            }
            // int(constraint), nat(constraint): the constraint names the
            // callee's quantifier, so freshen it rather than dropping it.
            // Dropped, `sail_zeros : forall 'n. int('n) -> bits('n)` arrived
            // as `int -> bits(?)` with nothing connecting the argument to
            // the width, and every call returned a bitvector of unknown
            // width that then agreed with any declared width at all.
            TyKind::App { name, args, text } if name == "int" || name == "nat" => {
                if args.is_empty() {
                    Ty::named(name.clone())
                } else {
                    let new_args: Vec<crate::ty::TyArg> = args
                        .iter()
                        .map(|a| match a {
                            crate::ty::TyArg::Type(t) => {
                                crate::ty::TyArg::Type(self.insert_type_vars(t))
                            }
                            crate::ty::TyArg::Value(v) if v.contains('\'') => {
                                crate::ty::TyArg::Type(self.new_type_var())
                            }
                            crate::ty::TyArg::Nexp(n) if n.to_string_repr().contains('\'') => {
                                crate::ty::TyArg::Type(self.new_type_var())
                            }
                            other => other.clone(),
                        })
                        .collect();
                    Ty::app(name.clone(), new_args, text.clone())
                }
            }
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
            TyKind::Exist { .. } => ty.clone(),
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
            TyKind::Named(name, args) => Ty::adt_with_args(
                name,
                args.iter()
                    .map(|a| match a {
                        TyArg::Type(t) => TyArg::Type(self.resolve(t)),
                        other => other.clone(),
                    })
                    .collect(),
            ),
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
    #[cfg(test)]
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
            TyKind::Named(name, args) => Ty::adt_with_args(
                name,
                args.iter()
                    .map(|a| match a {
                        TyArg::Type(t) => TyArg::Type(self.resolve_or_unknown(t)),
                        other => other.clone(),
                    })
                    .collect(),
            ),
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

    pub(super) fn universe(&self) -> u32 {
        self.universe
    }
    pub(super) fn restore_universe(&mut self, universe: u32) {
        self.universe = universe;
    }
    pub(super) fn is_rigid(&self, name: &str) -> bool {
        self.rigid_variables.contains_key(name)
    }

    /// Render source binder names without changing solver identities. Repeated
    /// binders retain a suffix so independent witnesses remain distinguishable.
    pub(super) fn display_constraint(&self, goal: &ConstraintExpr) -> String {
        let mut names = std::collections::HashSet::new();
        super::substitution::constraint_names(goal, &mut names);
        let mut subst = Subst::default();
        for name in names {
            let name = format!("'{name}");
            if !self.is_rigid(&name) {
                continue;
            }
            let Some((universe, original)) =
                name.strip_prefix("'_sk").and_then(|n| n.split_once('_'))
            else {
                continue;
            };
            let suffix = format!("_{original}");
            let repeated =
                self.rigid_order.iter().filter(|n| n.ends_with(&suffix)).take(2).count() > 1;
            let display =
                if repeated { format!("'{original}#{universe}") } else { format!("'{original}") };
            subst.values.insert(name, display);
        }
        constraint::apply_subst_constraint_expr(goal, &subst).to_text()
    }

    /// Open an actual existential with fresh constants. Only the caller's
    /// lexical scope may assume its predicate; these names are never unifiers.
    pub(super) fn open_existential(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
    ) -> super::existential::Skolemized {
        self.open_existential_avoiding(vars, predicate, inner, &Ty::error())
    }

    pub(super) fn reserve_source_names(&mut self, text: &str) {
        if text.contains('\'') {
            super::substitution::text_names(text, &mut self.reserved_names);
        }
    }

    fn witness_names(
        &self,
        predicate: &ConstraintExpr,
        inner: &Ty,
        avoid: &Ty,
    ) -> std::collections::HashSet<String> {
        let mut names = self.reserved_names.clone();
        super::substitution::ty_names(inner, &mut names, true);
        super::substitution::ty_names(avoid, &mut names, true);
        super::substitution::constraint_names(predicate, &mut names);
        names.extend(
            self.placeholder_to_infer
                .keys()
                .chain(self.rigid_variables.keys())
                .map(|n| n.trim_start_matches('\'').to_owned()),
        );
        names
    }

    pub(super) fn open_existential_avoiding(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
        avoid: &Ty,
    ) -> super::existential::Skolemized {
        let names = self.witness_names(predicate, inner, avoid);
        let prefix = loop {
            self.next_universe += 1;
            let prefix = format!("'_sk{}_", self.next_universe);
            if vars.iter().all(|v| {
                !names.contains(&format!(
                    "{}{}",
                    prefix.trim_start_matches('\''),
                    v.trim_start_matches('\'')
                ))
            }) {
                break prefix;
            }
        };
        self.universe = self.next_universe;
        let opened = super::existential::skolemize(vars, predicate, inner, &prefix);
        let mut names: Vec<_> = opened.skolem_map.values().cloned().collect();
        names.sort();
        let predicate = std::sync::Arc::new(opened.constraint.clone());
        for name in names {
            self.rigid_predicates.insert(name.clone(), predicate.clone());
            self.rigid_variables.insert(name.clone(), self.universe);
            self.rigid_order.push(name);
        }
        opened
    }

    /// A value leaving an elimination scope is repackaged, never exported
    /// with a free local skolem. Include witnesses referenced by its predicate.
    pub(super) fn close_universe(&mut self, ty: &Ty, outer: u32) -> Ty {
        if self.rigid_variables.is_empty() {
            return ty.clone();
        }
        let ty = self.deep_resolve(ty);
        let mut names = std::collections::HashSet::new();
        super::substitution::ty_names(&ty, &mut names, false);
        let mut vars = Vec::new();
        let mut predicates = Vec::new();
        loop {
            let mut added = false;
            let mut batch: Vec<_> = names
                .iter()
                .map(|n| format!("'{n}"))
                .filter(|n| {
                    self.rigid_variables.get(n).is_some_and(|u| *u > outer) && !vars.contains(n)
                })
                .collect();
            batch.sort();
            for name in batch {
                vars.push(name.clone());
                added = true;
                if let Some(predicate) = self.rigid_predicates.get(&name) {
                    super::substitution::constraint_names(predicate, &mut names);
                    if !predicates.contains(predicate.as_ref()) {
                        predicates.push(predicate.as_ref().clone());
                    }
                }
            }
            if !added {
                break;
            }
        }
        if vars.is_empty() {
            ty
        } else {
            Ty::exist(vars, ConstraintExpr::And(predicates), ty)
        }
    }

    /// Instantiate an expected existential with flexible witnesses. Numeric
    /// occurrences keep registered placeholders rather than rendering to `_`.
    #[cfg(test)]
    pub(super) fn instantiate_existential(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
    ) -> (Ty, ConstraintExpr) {
        self.instantiate_existential_avoiding(vars, predicate, inner, &Ty::error())
    }

    pub(super) fn instantiate_existential_avoiding(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
        avoid: &Ty,
    ) -> (Ty, ConstraintExpr) {
        let subst = self.existential_substitution(vars, predicate, inner, avoid);
        (
            constraint::apply_subst(inner, &subst),
            constraint::apply_subst_constraint_expr(predicate, &subst),
        )
    }

    pub(super) fn existential_substitution(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
        avoid: &Ty,
    ) -> Subst {
        let mut names = self.witness_names(predicate, inner, avoid);
        let mut subst = Subst::default();
        for var in vars {
            let fresh = self.new_type_var();
            let TyKind::Infer(crate::ty::InferTy(id)) = fresh.kind() else { unreachable!() };
            let mut placeholder = format!("'_witness{id}");
            while names.contains(placeholder.trim_start_matches('\'')) {
                placeholder.push('_');
            }
            names.insert(placeholder.trim_start_matches('\'').to_owned());
            self.register_placeholder(&placeholder, *id);
            subst.types.insert(var.clone(), fresh);
            subst.values.insert(var.clone(), placeholder);
        }
        subst
    }

    #[cfg(test)]
    fn freshen_existential(
        &mut self,
        vars: &[String],
        predicate: &ConstraintExpr,
        inner: &Ty,
    ) -> Ty {
        let (inner, predicate) = self.instantiate_existential(vars, predicate, inner);
        self.push_obligation(Obligation::NumericConstraint(predicate));
        inner
    }

    /// Bind only a flexible Boolean index, never a rigid caller value or a
    /// compound predicate. The ordinary binding machinery enforces universes.
    pub(super) fn bind_boolean_index(
        &mut self,
        expected: &ConstraintExpr,
        actual: &ConstraintExpr,
    ) {
        let ConstraintExpr::BoolVar(name) = expected else { return };
        let Some(id) = self.placeholder_infer_id(name) else { return };
        let variable = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(id))));
        let TyKind::Infer(variable) = variable.kind() else { return };
        let id = variable.0;
        let mut names = std::collections::HashSet::new();
        super::substitution::constraint_names(actual, &mut names);
        let mentions_variable = names.iter().any(|name| {
            self.placeholder_infer_id(&format!("'{name}")).is_some_and(|other| {
                self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(other))))
                    == Ty::new(TyKind::Infer(crate::ty::InferTy(id)))
            })
        });
        if !mentions_variable && !matches!(actual, ConstraintExpr::Unsupported) {
            self.bind(id, Ty::param(actual.to_text()));
        }
    }

    pub(super) fn resolve_constraint(&mut self, goal: &ConstraintExpr) -> ConstraintExpr {
        let mut names = std::collections::HashSet::new();
        super::substitution::constraint_names(goal, &mut names);
        let mut subst = Subst::default();
        for name in names {
            let name = format!("'{name}");
            if let Some(value) = self.placeholder_value(&name) {
                subst.values.insert(name, value);
            }
        }
        constraint::apply_subst_constraint_expr(goal, &subst)
    }

    /// Resolve the transitive placeholder dependencies of a deferred goal or
    /// its captured assumptions. A remaining inference placeholder is flexible,
    /// even when it occurs inside an otherwise resolved numeric expression.
    pub(super) fn resolve_deferred_constraint(
        &mut self,
        goal: &ConstraintExpr,
    ) -> (ConstraintExpr, bool) {
        let mut resolved = goal.clone();
        for _ in 0..=self.placeholder_to_infer.len() {
            let next = self.resolve_constraint(&resolved);
            if next == resolved {
                break;
            }
            resolved = next;
        }
        let mut names = std::collections::HashSet::new();
        super::substitution::constraint_names(&resolved, &mut names);
        let flexible =
            names.iter().any(|n| self.placeholder_to_infer.contains_key(&format!("'{n}")));
        (resolved, flexible)
    }

    /// Enforce universe levels transitively, including variables inside type
    /// arguments and numeric placeholders. Lower younger flexible variables
    /// when they become reachable from an older one.
    fn restrict_universe(&mut self, ty: &Ty, level: u32, depth: usize) -> bool {
        if depth > 96 {
            return false;
        }
        let mut names = std::collections::HashSet::new();
        super::substitution::ty_names(ty, &mut names, false);
        for name in names {
            let name = format!("'{name}");
            if self.rigid_variables.get(&name).is_some_and(|u| *u > level) {
                return false;
            }
            if let Some(id) = self.placeholder_infer_id(&name) {
                if !self.restrict_variable(id, level, depth + 1) {
                    return false;
                }
            }
        }
        self.restrict_nested_variables(ty, level, depth)
    }
    fn restrict_nested_variables(&mut self, ty: &Ty, level: u32, depth: usize) -> bool {
        if depth > 96 {
            return false;
        }
        match ty.kind() {
            TyKind::Infer(crate::ty::InferTy(id)) => self.restrict_variable(*id, level, depth + 1),
            TyKind::Tuple(xs) => {
                xs.iter().all(|x| self.restrict_nested_variables(x, level, depth + 1))
            }
            TyKind::FnPtr(sig) => sig
                .params
                .iter()
                .chain(std::iter::once(&sig.ret))
                .all(|x| self.restrict_nested_variables(x, level, depth + 1)),
            TyKind::App { args, .. } | TyKind::Named(_, args) => args.iter().all(|a| match a {
                TyArg::Type(t) => self.restrict_nested_variables(t, level, depth + 1),
                _ => true,
            }),
            TyKind::Exist { inner, .. } => self.restrict_nested_variables(inner, level, depth + 1),
            TyKind::Bidir { lhs, rhs } => {
                self.restrict_nested_variables(lhs, level, depth + 1)
                    && self.restrict_nested_variables(rhs, level, depth + 1)
            }
            _ => true,
        }
    }
    fn restrict_variable(&mut self, id: u32, level: u32, depth: usize) -> bool {
        if id >= self.var_count {
            return false;
        }
        if let TyVarValue::Known(ty) = self.safe_probe_by_id(id) {
            return self.restrict_universe(&ty, level, depth + 1);
        }
        let old = self.variable_universes[id as usize];
        if old > level {
            if self.snapshot_depth > 0 {
                self.universe_undo.push((id, old));
            }
            self.variable_universes[id as usize] = level;
        }
        true
    }

    /// Bind an inference variable to a type (with occurs check).
    pub(super) fn bind(&mut self, var_id: u32, ty: Ty) -> bool {
        if self.rigid_variables.is_empty() {
            return self.bind_inner(var_id, ty);
        }
        let snapshot = self.snapshot();
        let ok = self.bind_inner(var_id, ty);
        if ok {
            self.commit(snapshot);
        } else {
            self.rollback_to(snapshot);
        }
        ok
    }
    fn bind_inner(&mut self, var_id: u32, ty: Ty) -> bool {
        // Range guard: var from a different InferenceTable
        if var_id >= self.var_count {
            return false;
        }
        let level = self.variable_universes[var_id as usize];
        if !self.rigid_variables.is_empty() && !self.restrict_universe(&ty, level, 0) {
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
            TyKind::Named(_, args) | TyKind::App { args, .. } => {
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

    /// Compatibility adapter for inference callers. Arguments are expected, actual.
    pub fn unify(&mut self, expected: &Ty, actual: &Ty) -> bool {
        self.check_compatibility(expected, actual).is_compatible()
    }

    /// Check actual <: expected. Structural recovery may supply inference
    /// bindings, but cannot override a known nested mismatch. Erased value
    /// refinements use explicit Recovery rather than claiming a proof.
    pub fn check_compatibility(&mut self, expected: &Ty, actual: &Ty) -> Compatibility {
        use super::subtype::{is_subtype, SubtypeResult};
        let snapshot = self.snapshot();
        let expected = self.shallow_resolve(expected);
        let actual = self.shallow_resolve(actual);
        let result = match is_subtype(self, &actual, &expected) {
            SubtypeResult::Ok => Compatibility::Proven,
            SubtypeResult::Fail if self.needs_shape_recovery(&expected, &actual) => {
                self.push_subtype_obligation(Obligation::NumericConstraint(
                    ConstraintExpr::Unsupported,
                ));
                Compatibility::Recovery
            }
            SubtypeResult::Fail => Compatibility::Mismatch,
            SubtypeResult::NeedsConstraint(goal)
                if matches!(goal, Obligation::Assuming { .. } | Obligation::All(_))
                    || matches!(
                        self.normalize_alias_deep(&expected).kind(),
                        TyKind::Exist { .. }
                    )
                    || matches!(
                        self.normalize_alias_deep(&actual).kind(),
                        TyKind::Exist { .. }
                    ) =>
            {
                self.push_subtype_obligation(goal);
                Compatibility::Deferred
            }
            SubtypeResult::NeedsConstraint(_) => {
                let bindings = self.snapshot();
                let recovered = self.infer_bindings(&expected, &actual);
                if recovered {
                    self.commit(bindings);
                } else {
                    self.rollback_to(bindings);
                }
                if !recovered {
                    if self.needs_shape_recovery(&expected, &actual) {
                        self.push_subtype_obligation(Obligation::NumericConstraint(
                            ConstraintExpr::Unsupported,
                        ));
                        Compatibility::Recovery
                    } else {
                        Compatibility::Mismatch
                    }
                } else {
                    let expected = self.resolve(&expected);
                    let actual = self.resolve(&actual);
                    match is_subtype(self, &actual, &expected) {
                        SubtypeResult::Ok => Compatibility::Proven,
                        SubtypeResult::Fail if self.needs_shape_recovery(&expected, &actual) => {
                            self.push_subtype_obligation(Obligation::NumericConstraint(
                                ConstraintExpr::Unsupported,
                            ));
                            Compatibility::Recovery
                        }
                        SubtypeResult::Fail => Compatibility::Mismatch,
                        SubtypeResult::NeedsConstraint(goal) => {
                            let recovery = matches!(
                                goal,
                                Obligation::NumericConstraint(ConstraintExpr::Unsupported)
                            );
                            self.push_subtype_obligation(goal);
                            if recovery {
                                Compatibility::Recovery
                            } else {
                                Compatibility::Deferred
                            }
                        }
                    }
                }
            }
        };
        if result == Compatibility::Deferred {
            let unsupported: Vec<_> = self.pending_obligations[snapshot.obligations_len..]
                .iter()
                .enumerate()
                .filter_map(|(i, ob)| {
                    (ob.compatibility_types.is_none()
                        && super::obligation_stats::unsupported(&ob.goal.constraint()))
                    .then_some(snapshot.obligations_len + i)
                })
                .collect();
            if !unsupported.is_empty() {
                let actual = self.resolve(&actual);
                let expected = self.resolve(&expected);
                let types =
                    (self.normalize_alias_deep(&actual), self.normalize_alias_deep(&expected));
                for i in unsupported {
                    self.pending_obligations[i].compatibility_types = Some(types.clone());
                }
            }
        }
        if result == Compatibility::Recovery {
            for ob in &mut self.pending_obligations[snapshot.obligations_len..] {
                ob.cause = super::ObligationCause::Recovery;
                ob.recovery_types = Some((actual.clone(), expected.clone()));
            }
        }
        if result == Compatibility::Mismatch {
            self.rollback_to(snapshot);
        } else {
            self.commit(snapshot);
        }
        result
    }

    fn needs_shape_recovery(&self, expected: &Ty, actual: &Ty) -> bool {
        if erased_refinement(
            &self.normalize_alias_deep(expected),
            &self.normalize_alias_deep(actual),
        ) {
            return true;
        }
        // Config-dependent aliases and struct literals with erased generic
        // arguments do not supply enough evidence to prove the relation.
        for ty in [expected, actual] {
            if let Some(name) = ty.as_name() {
                if self.alias_schemes.get(name).is_some_and(|s| s.config_dependent) {
                    return true;
                }
            }
        }
        matches!((expected.kind(),actual.kind()),
            (TyKind::App {name,..},TyKind::Named(other,args)) if name==other && args.is_empty())
    }

    /// Legacy binding/recovery machinery; its boolean success is not a proof.
    pub(super) fn infer_bindings(&mut self, t1: &Ty, t2: &Ty) -> bool {
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
                return true;
            }
            _ => {}
        }

        // Infer on either side → bind
        match (t1.kind(), t2.kind()) {
            (TyKind::Infer(crate::ty::InferTy(id)), _) => return self.bind(*id, t2),
            (_, TyKind::Infer(crate::ty::InferTy(id))) => return self.bind(*id, t1),
            _ => {}
        }

        // Quantified relations are handled by the directional checker. Do
        // not unwrap either side here or invent flexible elimination witnesses.
        if matches!(t1.kind(), TyKind::Exist { .. }) || matches!(t2.kind(), TyKind::Exist { .. }) {
            return true;
        }

        // `implicit('n)` is `atom('n)` with the argument supplied by the
        // checker rather than written at the call site (type_env.ml
        // `builtin_typs`, both `[K_int] -> K_type`). Unwrap it so an implicit
        // parameter unifies with the integer it stands for.
        let strip_implicit = |t: &Ty| -> Option<Ty> {
            match t.kind() {
                TyKind::App { name, args, text } if name == "implicit" => {
                    Some(Ty::app("atom".to_string(), args.clone(), text.clone()))
                }
                _ => None,
            }
        };
        // The value an implicit parameter stands for may be written at the
        // call site — `canonical_NaN('m)`. That meets an unbound inference
        // variable against a numeric expression, a pair nothing below
        // matches, so `'n` stayed unbound and the constraint check threw the
        // candidate away. Binding it is safe *here* and nowhere else: an
        // implicit parameter has no other way to be determined, which is
        // what makes it implicit.
        for (imp, other) in [(&t1, &t2), (&t2, &t1)] {
            let TyKind::App { name, args, .. } = imp.kind() else { continue };
            if name != "implicit" || args.len() != 1 {
                continue;
            }
            // Freshening leaves the quantifier either as an inference
            // variable or as a placeholder *name* registered against one.
            let iv = match &args[0] {
                TyArg::Type(inner) => match self.resolve(inner).kind() {
                    TyKind::Infer(iv) => iv.0,
                    _ => continue,
                },
                TyArg::Nexp(n) => match self.placeholder_infer_id(&n.to_string_repr()) {
                    Some(id)
                        if matches!(
                            self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(id)))).kind(),
                            TyKind::Infer(_)
                        ) =>
                    {
                        id
                    }
                    _ => continue,
                },
                _ => continue,
            };
            if let TyKind::App { args: other_args, .. } = other.kind() {
                if let [TyArg::Nexp(n)] = other_args.as_slice() {
                    return self.bind(iv, Ty::param(n.to_string_repr()));
                }
            }
        }
        if let Some(inner) = strip_implicit(&t1) {
            return self.unify(&inner, &t2);
        }
        if let Some(inner) = strip_implicit(&t2) {
            return self.unify(&t1, &inner);
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

        // Two numeric ranges with concrete bounds either fit or they do not,
        // and the answer will not change later. Decide it here, before the
        // permissive numeric path in `unify_structural` and before the
        // coercion fallback — both of them accepted `range(0, 1)` where
        // `range(0, 0)` was asked for, which is why no numeric range
        // mismatch was ever reported. `is_subtype` already knew.
        //
        // By convention `t1` is the expected type and `t2` the actual, the
        // same order `infer_expr_hir_with` uses.
        if is_concrete_numeric_range(&t1) && is_concrete_numeric_range(&t2) {
            let (sub, sup) = (t2.clone(), t1.clone());
            if matches!(
                super::subtype::is_subtype(self, &sub, &sup),
                super::subtype::SubtypeResult::Fail
            ) {
                return false;
            }
        }

        // Structural unification
        if self.unify_structural(&t1, &t2, 0) {
            return true;
        }

        // Coercion fallback
        if coerce::try_coerce(self, &t2, &t1).is_ok() {
            return true;
        }

        // The expression inferencer still erases some singleton refinements
        // to int/bool. Keep editor recovery explicit: this is an unsupported
        // obligation, not a successful subtype proof. Value-aware callers
        // (literal/range/enum checks) supply their own evidence or diagnostic.
        let erased_refinement = match (t2.kind(), t1.kind()) {
            (TyKind::Scalar(crate::ty::Scalar::Int), TyKind::Scalar(crate::ty::Scalar::Nat)) => {
                true
            }
            (TyKind::Scalar(crate::ty::Scalar::Bool), TyKind::App { name, .. }) => {
                matches!(name.as_str(), "bool" | "atom_bool")
            }
            _ => false,
        };
        if erased_refinement {
            self.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Unsupported));
            return true;
        }

        // Alias expansion as last resort. Only tried when normal
        // unification + coercion both fail, to avoid regressions from
        // always expanding (config-dependent aliases become error types).
        {
            let t1_name = match t1.kind() {
                TyKind::Named(n, _) | TyKind::App { name: n, .. } => Some(n.as_str()),
                _ => None,
            };
            let t2_name = match t2.kind() {
                TyKind::Named(n, _) | TyKind::App { name: n, .. } => Some(n.as_str()),
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
                    if coerce::try_coerce(self, t2_use, t1_use).is_ok() {
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

    /// Semantic equality is subtyping in both directions, with one transaction.
    /// Recovery is explicit and never promotes a failed relation to equality.
    pub fn check_type_equality(&mut self, lhs: &Ty, rhs: &Ty) -> Compatibility {
        let snapshot = self.snapshot();
        let mut result = Compatibility::Proven;
        for (actual, expected) in [(lhs, rhs), (rhs, lhs)] {
            match super::subtype::is_subtype(self, actual, expected) {
                super::subtype::SubtypeResult::Ok => {}
                super::subtype::SubtypeResult::Fail => {
                    self.rollback_to(snapshot);
                    return Compatibility::Mismatch;
                }
                super::subtype::SubtypeResult::NeedsConstraint(goal) => {
                    result = Compatibility::Deferred;
                    self.push_subtype_obligation(goal);
                }
            }
        }
        self.commit(snapshot);
        result
    }

    /// Internal shape recovery only. The caller must return Recovery/Deferred
    /// and retain an obligation; this operation is never an equality proof.
    pub(super) fn recover_shape_bindings(&mut self, t1: &Ty, t2: &Ty) -> bool {
        let t1 = self.shallow_resolve(t1);
        let t2 = self.shallow_resolve(t2);
        let t1 = self.normalize_alias_ty(&t1);
        let t2 = self.normalize_alias_ty(&t2);

        if t1 == t2 || t1.is_error() || t2.is_error() {
            return true;
        }
        match (t1.kind(), t2.kind()) {
            (TyKind::Infer(crate::ty::InferTy(id)), _) => return self.bind(*id, t2),
            (_, TyKind::Infer(crate::ty::InferTy(id))) => return self.bind(*id, t1),
            _ => {}
        }

        self.unify_structural(&t1, &t2, 0)
    }

    /// Save current state for speculative unification.
    pub fn snapshot(&mut self) -> InferenceTableSnapshot {
        self.snapshot_depth += 1;
        InferenceTableSnapshot {
            depth: self.snapshot_depth,
            placeholder_undo_len: self.placeholder_undo.len(),
            ena_snapshot: self.table.snapshot(),
            obligations_len: self.pending_obligations.len(),
            var_count: self.var_count,
            universe: self.universe,
            next_universe: self.next_universe,
            universe_undo_len: self.universe_undo.len(),
            rigid_len: self.rigid_order.len(),
        }
    }

    /// Restore to a previous snapshot, undoing speculative bindings.
    /// Also restores var_count so it stays in sync with ena's table size.
    pub fn rollback_to(&mut self, snapshot: InferenceTableSnapshot) {
        assert_eq!(
            self.snapshot_depth, snapshot.depth,
            "inference snapshots must close in LIFO order"
        );
        self.table.rollback_to(snapshot.ena_snapshot);
        for (name, old) in self.placeholder_undo.drain(snapshot.placeholder_undo_len..).rev() {
            if let Some(id) = old {
                self.placeholder_to_infer.insert(name, id);
            } else {
                self.placeholder_to_infer.remove(&name);
            }
        }
        self.pending_obligations.truncate(snapshot.obligations_len);
        for (id, level) in self.universe_undo.drain(snapshot.universe_undo_len..).rev() {
            self.variable_universes[id as usize] = level;
        }
        self.variable_universes.truncate(snapshot.var_count as usize);
        for name in self.rigid_order.drain(snapshot.rigid_len..) {
            self.rigid_variables.remove(&name);
            self.rigid_predicates.remove(&name);
        }
        self.universe = snapshot.universe;
        self.next_universe = snapshot.next_universe;
        self.var_count = snapshot.var_count;
        self.snapshot_depth -= 1;
    }

    /// Keep this attempt's state, while allowing an outer snapshot to undo it.
    pub fn commit(&mut self, snapshot: InferenceTableSnapshot) {
        assert_eq!(
            self.snapshot_depth, snapshot.depth,
            "inference snapshots must close in LIFO order"
        );
        self.table.commit(snapshot.ena_snapshot);
        self.snapshot_depth -= 1;
        if self.snapshot_depth == 0 {
            self.placeholder_undo.clear();
            self.universe_undo.clear();
        }
    }

    /// Try unification speculatively. If it fails, the table is unchanged.
    pub(super) fn try_unify(&mut self, t1: &Ty, t2: &Ty) -> bool {
        let snap = self.snapshot();
        if self.unify(t1, t2) {
            self.commit(snap);
            true
        } else {
            self.rollback_to(snap);
            false
        }
    }

    /// Register a freshened placeholder name → InferTy id mapping.
    pub fn register_placeholder(&mut self, placeholder: &str, infer_id: u32) {
        let old = self.placeholder_to_infer.insert(placeholder.to_string(), infer_id);
        if self.snapshot_depth > 0 {
            self.placeholder_undo.push((placeholder.to_string(), old));
        }
    }

    /// The inference variable a freshening placeholder stands for.
    /// The value bound to one placeholder, as text.
    pub(super) fn placeholder_value(&mut self, name: &str) -> Option<String> {
        let id = *self.placeholder_to_infer.get(name)?;
        let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(id))));
        (!matches!(bound.kind(), TyKind::Infer(_))).then(|| bound.display_text())
    }

    fn placeholder_infer_id(&self, name: &str) -> Option<u32> {
        self.placeholder_to_infer.get(name).copied()
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
            TyKind::Named(name, args) => Ty::adt_with_args(
                name,
                args.iter()
                    .map(|arg| match arg {
                        TyArg::Type(t) => TyArg::Type(self.deep_resolve(t)),
                        TyArg::Nexp(n) => self.resolve_nexp_arg(n),
                        TyArg::Bool(c) => TyArg::Bool(self.resolve_constraint(c)),
                        TyArg::Order(v) => TyArg::Order(v.clone()),
                        TyArg::Value(v) => self.resolve_value_arg(v),
                    })
                    .collect(),
            ),
            TyKind::App { name, args, .. } => {
                let new_args: Vec<TyArg> = args
                    .iter()
                    .map(|arg| match arg {
                        TyArg::Type(t) => TyArg::Type(self.deep_resolve(t)),
                        TyArg::Nexp(nexp) => self.resolve_nexp_arg(nexp),
                        TyArg::Bool(c) => TyArg::Bool(self.resolve_constraint(c)),
                        TyArg::Order(v) => TyArg::Order(v.clone()),
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
                // A call candidate's bindings are rolled back after its
                // result is captured. Resolve free placeholders in the
                // predicate too, so e.g. min(a, b)'s bounds still refer to
                // a and b when the returned existential is opened later.
                let constraint = self.resolve_constraint(constraint);
                Ty::exist(vars.clone(), constraint, self.deep_resolve(inner))
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
            // A compound expression: substitute throughout, then fold.
            // `unsigned : forall 'n. bits('n) -> range(0, 2 ^ 'n - 1)`
            // applied to a `bits(4)` gives `range(0, 2 ^ 4 - 1)`, and
            // leaving it in that shape means the type is not *fully known*
            // — so every check that waits for a decided type waited
            // forever, and no range a call produced was ever compared with
            // the one a caller declared.
            _ => {
                let substituted = self.resolve_nexp_to_expr(nexp);
                let simplified = super::nexp_simplify::simplify(&substituted);
                match super::nexp_simplify::try_eval_const(&simplified) {
                    Some(v) => TyArg::numeric(v.to_string()),
                    None => TyArg::Nexp(simplified),
                }
            }
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
            NumericExpr::Add(a, b) => NumericExpr::Add(
                Box::new(self.resolve_nexp_to_expr(a)),
                Box::new(self.resolve_nexp_to_expr(b)),
            ),
            NumericExpr::Sub(a, b) => NumericExpr::Sub(
                Box::new(self.resolve_nexp_to_expr(a)),
                Box::new(self.resolve_nexp_to_expr(b)),
            ),
            NumericExpr::Mul(a, b) => NumericExpr::Mul(
                Box::new(self.resolve_nexp_to_expr(a)),
                Box::new(self.resolve_nexp_to_expr(b)),
            ),
            NumericExpr::Div(a, b) => NumericExpr::Div(
                Box::new(self.resolve_nexp_to_expr(a)),
                Box::new(self.resolve_nexp_to_expr(b)),
            ),
            NumericExpr::Mod(a, b) => NumericExpr::Mod(
                Box::new(self.resolve_nexp_to_expr(a)),
                Box::new(self.resolve_nexp_to_expr(b)),
            ),
            NumericExpr::App { name, args } => NumericExpr::App {
                name: name.clone(),
                args: args.iter().map(|a| self.resolve_nexp_to_expr(a)).collect(),
            },
            NumericExpr::If { cond, then_expr, else_expr } => NumericExpr::If {
                cond: Box::new(self.resolve_constraint(cond)),
                then_expr: Box::new(self.resolve_nexp_to_expr(then_expr)),
                else_expr: Box::new(self.resolve_nexp_to_expr(else_expr)),
            },
            NumericExpr::Exp(a) => NumericExpr::Exp(Box::new(self.resolve_nexp_to_expr(a))),
            NumericExpr::Neg(a) => NumericExpr::Neg(Box::new(self.resolve_nexp_to_expr(a))),
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
        // Substitute original tokens once. Prefixes such as '_fv1 / '_fv10
        // must stay distinct, and a replacement may itself mention a placeholder.
        let result = super::substitution::map_text(value, |name| {
            let infer_id = *self.placeholder_to_infer.get(name)?;
            let bound = self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(infer_id))));
            (!matches!(bound.kind(), TyKind::Infer(_))).then(|| bound.display_text())
        });
        if result != value {
            // Fold it: the substitution turns `2 ^ 'n - 1` into
            // `2 ^ 4 - 1`, and a bound in that shape compares with nothing.
            // Every range a call produced stayed a string until it did.
            if let Some(v) = NumericExpr::parse(&result)
                .map(|e| super::nexp_simplify::simplify(&e))
                .and_then(|e| super::nexp_simplify::try_eval_const(&e))
            {
                return TyArg::numeric(v.to_string());
            }
            TyArg::numeric(result)
        } else {
            TyArg::Value(value.to_string())
        }
    }

    /// Structural unification with InferTy binding (permissive semantics).
    fn unify_structural(&mut self, expected: &Ty, actual: &Ty, depth: usize) -> bool {
        const DEPTH_LIMIT: usize = 96;
        if depth > DEPTH_LIMIT {
            return true; // permissive on deep recursion
        }
        if actual.is_error() {
            return true;
        }
        // Named and App represent the same application. Never discard the
        // arguments of a Named while comparing constructors or their children.
        if matches!(expected.kind(), TyKind::Named(_, args) if !args.is_empty())
            || matches!(actual.kind(), TyKind::Named(_, args) if !args.is_empty())
        {
            return self.unify_structural(
                &applied_form(expected),
                &applied_form(actual),
                depth + 1,
            );
        }
        if matches!(actual.kind(), TyKind::Exist { .. }) {
            return self.check_compatibility(expected, actual).is_compatible();
        }
        match expected.kind() {
            // Binding only happened at the top of `unify`, so an inference
            // variable nested under a type constructor was accepted and
            // discarded: `register('a) -> 'a` freshens to `register(?0) ->
            // ?0`, met `register(bits(32))`, and still returned `?0`.
            TyKind::Infer(crate::ty::InferTy(id)) => {
                let id = *id;
                let actual = self.shallow_resolve(actual);
                if matches!(actual.kind(), TyKind::Infer(_) | TyKind::Error) {
                    return true;
                }
                self.bind(id, actual)
            }
            TyKind::Error => true,
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
            TyKind::Named(expected, _) => {
                if matches!(actual.kind(), TyKind::Tuple(_) | TyKind::FnPtr(..)) {
                    return false;
                }
                if let Some(actual_name) = actual.as_name() {
                    if expected == actual_name {
                        return true;
                    }
                }
                if let TyKind::Named(actual_text, _) = actual.kind() {
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
                    // A bitvector is not an integer. Falling through to the
                    // permissive tail below let `bits(5)` accept `{1, 2, 4,
                    // 8}`, which chose the forwards direction for
                    // `encdec_nreg : bits(5) <-> {1, 2, 4, 8}` and handed an
                    // `int` to a concatenation.
                    if constraint::is_numeric_scalar_ty(actual) {
                        return false;
                    }
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
                // A `struct`/`union`/`enum`/`bitfield` name is not an
                // alias, so nothing here is unknown: it accepts itself and
                // nothing else. Without this, `let z : int = x` with `x` a
                // struct went unreported, and so did every other use of a
                // record-typed value at a type it is not.
                if self.nominal_types.contains(expected) {
                    return match actual.as_name() {
                        Some(n) => n == expected.as_str(),
                        None => !is_actual_primitive,
                    };
                }
                if let Some(actual_name) = actual.as_name() {
                    let nominal: &str = actual_name;
                    if is_expected_primitive && self.nominal_types.contains(nominal) {
                        return false;
                    }
                }
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
                    // A concrete numeric actual pins the expected type's
                    // width variable: `int('n)` meeting an `atom(8)` is how
                    // `'n` learns it is 8. `bind_numeric_args` below binds it
                    // to `int`, which loses the number and with it the
                    // callee's return width.
                    if let Some(value) = single_numeric_value(actual) {
                        if expected_args.len() == 1
                            && matches!(expected_args[0], TyArg::Nexp(_) | TyArg::Value(_))
                        {
                            return self.unify_ty_arg(
                                &expected_args[0],
                                &TyArg::numeric(value.to_string()),
                                depth + 1,
                            );
                        }
                        for arg in &expected_args {
                            if let TyArg::Type(t) = arg {
                                let resolved = self.resolve(t);
                                if let TyKind::Infer(crate::ty::InferTy(id)) = resolved.kind() {
                                    let _ = self.bind(*id, Ty::param(value.to_string()));
                                }
                            }
                        }
                    }
                    // A *symbolic* numeric actual pins it just as well:
                    // `int(?0)` meeting `int('n)` is how `?0` learns it is
                    // the caller's own `'n`, and only then does a constraint
                    // like `0 < 'n <= max_mem_access` name something the
                    // caller's quantifiers can discharge. Without this,
                    // `bind_numeric_args` below bound it to `int` — losing
                    // the name, so the constraint spoke about a variable no
                    // assumption mentions and the candidate was dropped.
                    else if let Some(sym) = single_symbolic_value(actual) {
                        if expected_args.len() == 1
                            && matches!(expected_args[0], TyArg::Nexp(_) | TyArg::Value(_))
                        {
                            return self.unify_ty_arg(
                                &expected_args[0],
                                &TyArg::numeric(sym),
                                depth + 1,
                            );
                        }
                        for arg in &expected_args {
                            if let TyArg::Type(t) = arg {
                                let resolved = self.resolve(t);
                                if let TyKind::Infer(crate::ty::InferTy(id)) = resolved.kind() {
                                    let _ = self.bind(*id, Ty::param(sym.clone()));
                                }
                            }
                        }
                    }
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
                if let TyKind::App { name: actual_name, args: actual_args, .. } = actual.kind() {
                    if (expected_name == "bits" && actual_name == "bitvector")
                        || (expected_name == "bitvector" && actual_name == "bits")
                    {
                        // The widths still have to meet: accepting the pair
                        // outright left `'n` unbound for every call to
                        // `val unsigned : forall 'n. bitvector('n, inc) ->
                        // range(0, 2 ^ 'n - 1)` with a `bits(4)` literal.
                        let (Some(e), Some(a)) = (expected_args.first(), actual_args.first())
                        else {
                            return true;
                        };
                        let a = a.clone();
                        return self.unify_ty_arg(e, &a, depth + 1);
                    }
                }
                // bits(N) ≡ vector(N, bit), for either spelling of `bits`
                if expected_name == "bits" || expected_name == "bitvector" {
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
                                // An empty vector literal has no element type
                                // yet — `[]` is `vector(0, ?)` and must still
                                // check against `bits(0)`.
                                if is_bit_or_unresolved(&elem) {
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
                                if is_bit_or_unresolved(elem) {
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
                    // `bool('p)` carries a Bool-kinded constraint, not a
                    // type. Deciding `'p & 'q` against `'b & false` needs the
                    // constraint solver upstream runs; without it, comparing
                    // the arguments structurally rejects equal types. Same
                    // policy as the numeric arms below: same constructor,
                    // undecidable argument, accept.
                    TyKind::App { name: actual_name, .. }
                        if expected_name == *actual_name
                            && (expected_name == "bool" || expected_name == "atom_bool") =>
                    {
                        true
                    }
                    TyKind::App { name: actual_name, args: actual_args, .. }
                        if expected_name == *actual_name
                            && expected_args.len() == actual_args.len() =>
                    {
                        let pairs: Vec<_> = expected_args
                            .iter()
                            .zip(actual_args.iter())
                            .enumerate()
                            .map(|(i, (e, a))| {
                                if super::kinds::arguments(&expected_name, expected_args.len())
                                    .and_then(|ks| ks.get(i))
                                    == Some(&crate::ty::Kind::Int)
                                {
                                    (
                                        super::kinds::numeric_argument(e).into_owned(),
                                        super::kinds::numeric_argument(a).into_owned(),
                                    )
                                } else {
                                    (e.clone(), a.clone())
                                }
                            })
                            .collect();
                        // A *numeric* argument may reach here as an unbound
                        // inference variable on one side and a numeric
                        // expression on the other — freshening produces the
                        // first, a caller's own quantifier the second. That
                        // pair matched nothing, so `int(?0)` against
                        // `int('n)` left `?0` to be bound elsewhere, to
                        // `int` itself, and the constraint `0 < 'n <= 8`
                        // became a statement about a variable no assumption
                        // mentions.
                        //
                        // Only for the type constructors whose arguments are
                        // numbers. Doing it for every `TyArg` binds type
                        // arguments that have to stay open, which turned 92
                        // sail-riscv files red.
                        for (i, (e, a)) in pairs.iter().enumerate() {
                            // Vector length is numeric, its element is Type,
                            // and legacy vectors also carry an Order slot.
                            if super::kinds::arguments(&expected_name, pairs.len())
                                .and_then(|ks| ks.get(i))
                                != Some(&crate::ty::Kind::Int)
                            {
                                continue;
                            }
                            let ((TyArg::Type(t), TyArg::Nexp(n))
                            | (TyArg::Nexp(n), TyArg::Type(t))) = (e, a)
                            else {
                                continue;
                            };
                            let resolved = self.resolve(t);
                            if let TyKind::Infer(iv) = resolved.kind() {
                                let iv = *iv;
                                self.bind(iv.0, Ty::param(n.to_string_repr()));
                            }
                        }
                        // A *numeric* argument may be an unbound inference
                        // variable on one side and a number on the other:
                        // freshening produces the first, an expanded alias
                        // (`bits_H` -> `bits(16)`) or a caller's own
                        // quantifier the second. `unify_ty_arg` has no arm
                        // for that pair, so the quantifier stayed unbound
                        // and the candidate was dropped as unresolved.
                        //
                        // Only these constructors, whose arguments are
                        // numbers, and only a literal or a lone variable.
                        // Doing it for every `TyArg` binds type arguments
                        // that must stay open — 92 sail-riscv files red.
                        if matches!(
                            expected_name.as_str(),
                            "int" | "atom" | "nat" | "range" | "bits" | "bitvector" | "implicit"
                        ) {
                            for (e, a) in &pairs {
                                let ((TyArg::Type(t), TyArg::Nexp(n))
                                | (TyArg::Nexp(n), TyArg::Type(t))) = (e, a)
                                else {
                                    continue;
                                };
                                let text = n.to_string_repr();
                                let simple = text.parse::<i64>().is_ok()
                                    || (text.starts_with('\'')
                                        && text[1..]
                                            .chars()
                                            .all(|c| c.is_alphanumeric() || c == '_'));
                                if !simple {
                                    continue;
                                }
                                let resolved = self.resolve(t);
                                if let TyKind::Infer(iv) = resolved.kind() {
                                    let iv = *iv;
                                    self.bind(iv.0, Ty::param(text));
                                }
                            }
                        }
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
                        // An alias with no arguments arrives as a `Named`,
                        // not an `App` — `type bits_H = bits(16)` meeting a
                        // parameter `bits('m)`. Only the `App` side was
                        // expanded.
                        if let TyKind::Named(a_name, a_args) = actual.kind() {
                            let a_name = a_name.clone();
                            let a_args = a_args.clone();
                            if let Some(sch) = self.alias_scheme(&a_name) {
                                if let Some(expanded) = sch.substitute(&a_args) {
                                    let expected_ty =
                                        Ty::app(expected_name.clone(), expected_args.clone(), "");
                                    if self.unify(&expected_ty, &expanded) {
                                        return true;
                                    }
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
            TyKind::Exist { .. } => self.check_compatibility(expected, actual).is_compatible(),
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
    fn unify_ty_arg(&mut self, a1: &TyArg, a2: &TyArg, _depth: usize) -> bool {
        match (a1, a2) {
            (TyArg::Type(t1), TyArg::Type(t2)) => {
                // The full `unify`, not `unify_structural`. Alias expansion
                // and inference-variable binding both happen in `unify`
                // before it delegates, and a type *argument* reached the
                // delegate directly — so `result(xlenbits, unit)` and
                // `result(bits(64), ?1)` were two different types on both
                // counts.
                self.unify(t1, t2)
            }
            (a, b) => {
                let s1 = a.as_value_str();
                let s2 = b.as_value_str();
                match (s1, s2) {
                    (Some(v1), Some(v2)) => {
                        if super::normalized_value_text(&v1) == super::normalized_value_text(&v2) {
                            return true;
                        }
                        // Freshening rewrites a scheme's numeric arguments to
                        // placeholder names registered against an inference
                        // variable. Matching such a placeholder against a real
                        // width has to *bind* it: accepting the pair and moving
                        // on is what leaves a resolved signature reading
                        // `bits('_fv0)`, so the caller never learns the width it
                        // asked for.
                        for (ph, other) in [(&v1, &v2), (&v2, &v1)] {
                            let Some(id) = self.placeholder_infer_id(ph) else { continue };
                            let bound =
                                self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(id))));
                            if matches!(bound.kind(), TyKind::Infer(_)) {
                                return self.bind(id, Ty::param(other.clone()));
                            }
                        }
                        // One side a literal width, the other linear in one
                        // variable: an equation, and it either has an integer
                        // solution or the program is wrong. `bits(8 * 'n)`
                        // against `bits(64)` says `'n` is 8 — and nothing
                        // else can say it, for a call with no arguments —
                        // while against `bits(65)` nothing can make it true.
                        // Being permissive about every apostrophe left the
                        // quantifier unbound, so the constraint check called
                        // it unresolved and threw the candidate away.
                        for (expr_text, lit_text) in [(&v1, &v2), (&v2, &v1)] {
                            if !expr_text.contains('\'') || lit_text.contains('\'') {
                                continue;
                            }
                            let Ok(value) = lit_text.trim().parse::<i64>() else { continue };
                            let Some(expr) = NumericExpr::parse(expr_text)
                                .or_else(|| super::parse_numeric_expr_text(expr_text))
                            else {
                                continue;
                            };
                            match super::nexp_simplify::solve_linear(&expr, value) {
                                Some(Err(())) => return false,
                                Some(Ok((var, solved))) => {
                                    if let Some(&id) = self.placeholder_to_infer.get(&var) {
                                        let bound = self.resolve(&Ty::new(TyKind::Infer(
                                            crate::ty::InferTy(id),
                                        )));
                                        if matches!(bound.kind(), TyKind::Infer(_)) {
                                            return self.bind(id, Ty::param(solved.to_string()));
                                        }
                                    }
                                    return true;
                                }
                                None => {}
                            }
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
                            // None: undecidable — fall through to permissive
                            if let Some(result) = nexp_simplify::check_eq(&n1, &n2) {
                                return result;
                            }
                        }
                        // Final fallback: permissive (can't decide)
                        true
                    }
                    // One side is a type-shaped argument, the other a numeric
                    // one. That happens on every call to a `forall 'n`
                    // function: freshening turns `bits('n)` into
                    // `bits(Infer)` while the argument carries `bits(4)`.
                    // Accepting the pair without binding leaves the
                    // quantifier unresolved, which is what made
                    // `range_subset(0x0, 0x0)` report "could not resolve type
                    // variables". Only a literal width is safe to bind: an
                    // expression like `8 * 'n` names another variable, and
                    // equating the two is a job for the constraint solver.
                    _ => {
                        // A type-kinded quantifier still reaches `TyArg` in
                        // value position — `register('a)` lowers `'a` the same
                        // way `bits('n)` does — so bind the placeholder to the
                        // type it met. Without this `read_register(ref A)` for
                        // `forall ('a : Type). register('a) -> 'a` left `'a`
                        // free and the result came back as `unit`.
                        for (arg, other) in [(a, b), (b, a)] {
                            let TyArg::Type(t) = arg else { continue };
                            let Some(name) = other.as_value_str() else { continue };
                            if let Some(&id) = self.placeholder_to_infer.get(&*name) {
                                let bound =
                                    self.resolve(&Ty::new(TyKind::Infer(crate::ty::InferTy(id))));
                                if matches!(bound.kind(), TyKind::Infer(_))
                                    && !matches!(t.kind(), TyKind::Infer(_))
                                {
                                    return self.bind(id, t.clone());
                                }
                            }
                        }
                        for (arg, other) in [(a, b), (b, a)] {
                            let TyArg::Type(t) = arg else { continue };
                            let Some(width) = other.as_value_str() else { continue };
                            if width.parse::<u64>().is_err() {
                                continue;
                            }
                            let resolved = self.resolve(t);
                            if let TyKind::Infer(crate::ty::InferTy(id)) = resolved.kind() {
                                return self.bind(*id, Ty::param(width));
                            }
                            // Already bound by an earlier occurrence of the
                            // same quantifier. That is the whole point of
                            // `forall 'n. (bits('n), bits('n)) -> unit` —
                            // the second occurrence has to *agree*, and
                            // falling through to the permissive answer below
                            // meant a width variable was never shared with
                            // anything. Only literal widths are compared; an
                            // expression naming another variable is still the
                            // solver's job.
                            if let TyKind::Param(bound) = resolved.kind() {
                                if let (Ok(x), Ok(y)) = (bound.parse::<u64>(), width.parse::<u64>())
                                {
                                    return x == y;
                                }
                            }
                        }
                        true // permissive
                    }
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
    depth: usize,
    placeholder_undo_len: usize,
    universe: u32,
    next_universe: u32,
    universe_undo_len: usize,
    rigid_len: usize,
}

/// Canonical comparison form for the two representations of an applied type.
fn applied_form(ty: &Ty) -> Ty {
    match ty.kind() {
        TyKind::Named(name, args) if !args.is_empty() => {
            Ty::app(name, args.clone(), super::numeric::app_text(name, args))
        }
        _ => ty.clone(),
    }
}

/// Whether this is a `range`/`atom` whose bounds are all concrete numbers.
///
/// Anything mentioning a quantifier is excluded: `range(0, 'n)` cannot
/// disagree with another range until `'n` is known.
fn is_concrete_numeric_range(ty: &Ty) -> bool {
    matches!(ty.kind(), TyKind::App { name, .. } if name == "range" || name == "atom")
        && ty.is_fully_known()
}

/// The single number a concrete `atom(k)` or `range(k, k)` stands for.
/// The single *symbolic* numeric argument of `int('n)` / `atom('n)`, if the
/// type has exactly one and it is a variable rather than a literal.
fn single_symbolic_value(ty: &Ty) -> Option<String> {
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    if !matches!(name.as_str(), "int" | "atom" | "nat") || args.len() != 1 {
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
    // An exact singleton may contain arithmetic or a conditional. Keeping
    // the whole expression binds the value, without solving an equation or
    // choosing one of its branches. Erased widths are never evidence.
    let value = NumericExpr::parse(&text)?;
    (!super::subtype::contains_erased_numeric(&value)).then_some(text)
}

fn single_numeric_value(ty: &Ty) -> Option<i128> {
    let TyKind::App { name, args, .. } = ty.kind() else { return None };
    let num = |a: &TyArg| -> Option<i128> {
        match a {
            TyArg::Nexp(n) => n.to_string_repr().parse().ok(),
            TyArg::Value(v) => v.trim().parse().ok(),
            TyArg::Type(_) | TyArg::Bool(_) | TyArg::Order(_) => None,
        }
    };
    match name.as_str() {
        "atom" => num(args.first()?),
        "range" => {
            let lo = num(args.first()?)?;
            (lo == num(args.get(1)?)?).then_some(lo)
        }
        _ => None,
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    fn applied(name: &str, ty: Ty) -> Ty {
        Ty::adt_with_args(name, vec![TyArg::Type(ty)])
    }

    #[test]
    fn committed_inner_state_is_still_undone_by_the_outer_snapshot() {
        let mut table = InferenceTable::new();
        let original = table.new_type_var();
        table.register_placeholder("keep", 0);
        let outer = table.snapshot();
        let _ = table.new_type_var();
        table.register_placeholder("keep", 1);
        let inner = table.snapshot();
        let _ = table.new_type_var();
        table.register_placeholder("keep", 2);
        table.register_placeholder("discard", 2);
        table.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Bool(false)));
        table.commit(inner);
        assert_eq!(table.placeholder_infer_id("keep"), Some(2));
        table.rollback_to(outer);
        assert_eq!(table.var_count(), 1);
        assert_eq!(table.placeholder_infer_id("keep"), Some(0));
        assert_eq!(table.placeholder_infer_id("discard"), None);
        assert!(table.drain_obligations().is_empty());
        assert_eq!(table.deep_resolve(&original), original);
        assert_eq!(table.snapshot_depth, 0);
        assert!(table.placeholder_undo.is_empty());
    }

    #[test]
    fn successful_probes_close_snapshots_and_failed_candidates_restore_bindings() {
        let mut table = InferenceTable::new();
        let a = table.new_type_var();
        let b = table.new_type_var();
        assert!(table.try_unify(&a, &Ty::named("bool")));
        assert_eq!(table.snapshot_depth, 0);
        {
            let mut context =
                crate::method_resolution::MethodResolutionContext { table: &mut table };
            assert!(!context.try_candidate(
                &[b.clone(), Ty::named("int")],
                &[Ty::named("string"), Ty::named("bool")]
            ));
        }
        assert_eq!(table.deep_resolve(&b), b);
        let outer = table.snapshot();
        {
            let mut context =
                crate::method_resolution::MethodResolutionContext { table: &mut table };
            assert!(context.try_candidate(&[b.clone()], &[Ty::named("string")]));
        }
        assert_eq!(table.deep_resolve(&b), Ty::named("string"));
        table.rollback_to(outer);
        assert_eq!(table.deep_resolve(&b), b);
        assert_eq!(table.snapshot_depth, 0);
    }

    #[test]
    fn recovery_does_not_bind_an_unknown_to_error() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();
        assert!(table.check_type_equality(&var, &Ty::error()).is_compatible());
        assert!(table.check_type_equality(&Ty::error(), &var).is_compatible());
        assert_eq!(table.deep_resolve(&var), var);
        assert!(table.check_type_equality(&var, &Ty::named("bool")).is_compatible());
        assert_eq!(table.deep_resolve(&var), Ty::named("bool"));
    }

    #[test]
    fn raw_unification_is_reflexive_for_an_unknown() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();
        assert!(table.check_type_equality(&var, &var).is_compatible());
        assert_eq!(table.deep_resolve(&var), var);
    }

    #[test]
    fn a_named_type_cannot_contain_its_own_inference_variable() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();
        assert!(!table.unify(&var, &applied("Box", var.clone())));
    }

    #[test]
    fn named_arguments_are_resolved_and_unresolved_arguments_are_finalized() {
        let mut table = InferenceTable::new();
        let var = table.new_type_var();
        let named = applied("Box", var.clone());
        assert_eq!(table.resolve_or_unknown(&named), applied("Box", Ty::error()));
        assert!(table.unify(&var, &Ty::named("bool")));
        assert_eq!(table.deep_resolve(&named), applied("Box", Ty::named("bool")));
    }

    #[test]
    fn named_applications_check_arguments_in_both_representations() {
        let mut table = InferenceTable::new();
        let int_box = applied("Box", Ty::named("int"));
        let bool_box = applied("Box", Ty::named("bool"));
        let app = Ty::app("Box", vec![TyArg::Type(Ty::named("int"))], "Box(int)");
        assert!(table.unify(&int_box, &app));
        assert!(table.unify(&app, &int_box));
        assert!(!table.unify(&int_box, &bool_box));
        assert!(!table.unify(&app, &bool_box));
        let wrap = |ty| Ty::tuple(vec![ty, Ty::named("unit")]);
        assert!(!table.unify(&wrap(int_box), &wrap(bool_box)));
        // Type argument count and concrete numeric arguments are semantic too.
        let bits = |n| Ty::adt_with_args("bits", vec![TyArg::numeric(n)]);
        assert!(!table.unify(&bits("8"), &bits("16")));
        assert!(!table.unify(
            &app,
            &Ty::adt_with_args(
                "Box",
                vec![TyArg::Type(Ty::named("int")), TyArg::Type(Ty::named("bool"))]
            )
        ));
    }

    #[test]
    fn rollback_restores_placeholder_names_before_variable_ids_are_reused() {
        let mut table = InferenceTable::new();
        let original = table.new_type_var();
        table.register_placeholder("keep", 0);
        let snapshot = table.snapshot();
        let speculative = table.new_type_var();
        table.register_placeholder("keep", 1);
        table.register_placeholder("discard", 1);
        assert!(table.unify(&speculative, &Ty::named("bool")));
        table.rollback_to(snapshot);
        let reused = table.new_type_var();
        assert!(table.unify(&reused, &Ty::named("int")));
        assert!(table.unify(&original, &Ty::named("string")));
        assert_eq!(table.deep_resolve(&Ty::param("keep")), Ty::named("string"));
        assert_eq!(table.deep_resolve(&Ty::param("discard")), Ty::param("discard"));
    }

    #[test]
    fn nested_variables_remain_connected_after_structural_unification() {
        let mut table = InferenceTable::new();
        let a = table.new_type_var();
        let b = table.new_type_var();
        let list = |v| Ty::app("list", vec![TyArg::Type(v)], "list(_)");
        assert!(table.unify(&list(a.clone()), &list(b.clone())));
        assert!(table.unify(&b, &Ty::named("bool")));
        assert_eq!(table.deep_resolve(&a), Ty::named("bool"));
    }
    #[test]
    fn legacy_placeholder_resolution_is_simultaneous_and_matches_whole_names() {
        let mut table = InferenceTable::new();
        let first = table.new_type_var();
        let second = table.new_type_var();
        let TyKind::Infer(first) = first.kind() else { panic!() };
        let TyKind::Infer(second) = second.kind() else { panic!() };
        table.register_placeholder("'_fv1", first.0);
        table.register_placeholder("'_fv10", second.0);
        assert!(table.bind(first.0, Ty::param("'_fv10 + 1")));
        assert!(table.bind(second.0, Ty::param("8")));
        assert_eq!(table.resolve_value_arg("'_fv1 * '_fv10"), TyArg::numeric("('_fv10 + 1) * 8"));
    }
    #[test]
    fn existential_freshening_shares_witnesses_and_respects_nested_binders() {
        let mut table = InferenceTable::new();
        let nested = Ty::exist(vec!["'n".into()], ConstraintExpr::Bool(true), Ty::param("'n"));
        let inner = Ty::tuple(vec![
            Ty::param("'n"),
            Ty::app("bits", vec![TyArg::numeric("'n")], ""),
            nested.clone(),
        ]);
        let result = table.freshen_existential(&["'n".into()], &ConstraintExpr::Bool(true), &inner);
        let TyKind::Tuple(parts) = result.kind() else { panic!() };
        assert!(matches!(parts[0].kind(), TyKind::Infer(_)));
        let TyKind::App { args, .. } = parts[1].kind() else { panic!() };
        let TyKind::Infer(crate::ty::InferTy(id)) = parts[0].kind() else { panic!() };
        let placeholder = format!("'_witness{id}");
        assert_eq!(args, &[TyArg::numeric(&placeholder)]);
        assert_eq!(table.placeholder_infer_id(&placeholder), Some(*id));
        assert_eq!(parts[2], nested);
    }
}

// Inference still erases some literal/primitive refinements. Keep that
// compatibility explicitly unproved until value-aware inference supplies them.
fn erased_refinement(expected: &Ty, actual: &Ty) -> bool {
    match (actual.kind(), expected.kind()) {
        // A coarse expression type contains no runtime value evidence.
        // Keep its refinement obligation unknown, as for range/atom; the
        // existential wrapper does not turn missing evidence into a mismatch.
        (_, TyKind::Exist { inner, .. }) => erased_refinement(inner, actual),
        (TyKind::Scalar(crate::ty::Scalar::Int), TyKind::Scalar(crate::ty::Scalar::Nat)) => true,
        (TyKind::Scalar(crate::ty::Scalar::Int), TyKind::App { name, .. }) => {
            matches!(name.as_str(), "range" | "int" | "atom" | "nat" | "implicit")
        }
        (TyKind::Scalar(crate::ty::Scalar::Bool), TyKind::App { name, .. }) => {
            matches!(name.as_str(), "bool" | "atom_bool")
        }
        _ => false,
    }
}
