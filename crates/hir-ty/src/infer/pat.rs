//! Pattern type inference.
//!
//! Adds `impl InferenceContext` methods for pattern inference, called
//! from expression inference (match arms, let bindings, foreach, etc.).
//!
//! # Key methods
//!
//! - `bind_pattern_hir`: Main entry — recursively binds pattern variables
//!   with their inferred types based on the expected (scrutinee) type.
//! - `collect_vector_subrange_parts_hir`: Extracts vector subrange info.

use std::collections::{HashMap, HashSet};

use hir_def::Pat;

use super::constraint;
use super::expr::InferenceContext;
use super::is_pattern_binding;
use super::{ConstraintExpr, NumericExpr, Subst, Ty, TyArg, TyKind};
use crate::infer::{InferenceDiagnostic, LocalEnv};

impl<'db> InferenceContext<'db> {
    fn invalid_pattern(&mut self, pat: hir_def::PatId, message: impl Into<String>) {
        self.push_inference_diagnostic(InferenceDiagnostic::InvalidTypeUse {
            node: hir_def::ExprOrPatId::PatId(pat),
            message: message.into(),
        });
    }

    /// Only a resolved shape proves a mismatch. Unknown/abstract types and
    /// inference variables must remain undecided in an incomplete workspace.
    pub(super) fn known_type_shape(&self, ty: &Ty) -> bool {
        match ty.kind() {
            TyKind::Scalar(_) | TyKind::Tuple(_) => true,
            _ => ty.ctor_name().is_some_and(|n| {
                matches!(
                    n,
                    "list"
                        | "vector"
                        | "bits"
                        | "bitvector"
                        | "register"
                        | "atom"
                        | "atom_bool"
                        | "range"
                        | "implicit"
                ) || self.env.records.contains_key(n)
                    || self.env.enums.contains_key(n)
                    || self.env.unions.contains_key(n)
                    || self.env.bitfields.contains_key(n)
            }),
        }
    }

    /// Substitute actual arguments before inspecting a type-pattern alias.
    /// Keep the numeric argument and stop recursive synonym expansion.
    fn expand_type_pattern_alias(&self, ty: &Ty) -> Ty {
        let mut current = ty.clone();
        let mut seen = HashSet::new();
        for _ in 0..64 {
            let (name, args) = match current.kind() {
                TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
                _ => break,
            };
            if !seen.insert(name.clone()) {
                break;
            }
            if let Some(scheme) = self.table.alias_scheme(name) {
                match scheme.substitute(args) {
                    Some(expanded) => current = expanded,
                    None => break,
                }
            } else if args.is_empty() {
                let expanded = self.table.normalize_alias_ty(&current);
                if expanded == current {
                    break;
                }
                current = expanded;
            } else {
                break;
            }
        }
        current
    }

    /// Type patterns match expanded synonyms structurally. Ordinary numeric
    /// subtyping would incorrectly allow `dup(int('n))` against `(int(1),
    /// int(2))`, or `bytes('n)` against `bits(33)`.
    fn match_type_pattern(
        &mut self,
        pattern: &Ty,
        actual: &Ty,
        subst: &mut Subst,
        depth: usize,
    ) -> bool {
        if depth > 64 || pattern.is_unknown() || actual.is_unknown() {
            return true;
        }
        let pattern = self.expand_type_pattern_alias(pattern);
        let actual = self.expand_type_pattern_alias(actual);
        if pattern.is_named("_") {
            return true;
        }
        match (pattern.kind(), actual.kind()) {
            (
                TyKind::Param(_),
                TyKind::Scalar(
                    crate::ty::Scalar::String | crate::ty::Scalar::Unit | crate::ty::Scalar::Real,
                ),
            ) if depth == 0 => false,
            (TyKind::Param(name), _) => {
                if let Some(bound) = subst.types.get(name).cloned() {
                    self.match_type_pattern(&bound, &actual, subst, depth + 1)
                } else {
                    subst.types.insert(name.clone(), actual);
                    true
                }
            }
            (TyKind::Tuple(ps), TyKind::Tuple(ts)) => {
                ps.len() == ts.len()
                    && ps
                        .iter()
                        .zip(ts)
                        .all(|(p, t)| self.match_type_pattern(p, t, subst, depth + 1))
            }
            (
                TyKind::App { name: pn, args: ps, .. } | TyKind::Named(pn, ps),
                TyKind::App { name: tn, args: ts, .. } | TyKind::Named(tn, ts),
            ) if type_pattern_head(pn) == type_pattern_head(tn) => {
                ps.len() == ts.len()
                    && ps.iter().zip(ts).enumerate().all(|(i, (p, t))| {
                        let numeric = match pn.as_str() {
                            "bits" | "bitvector" | "int" | "atom" | "range" | "implicit" => true,
                            "vector" => i == 0,
                            _ => matches!(p, TyArg::Nexp(_)) || matches!(t, TyArg::Nexp(_)),
                        };
                        let text = |arg: &TyArg| match arg {
                            TyArg::Type(t) => t.display_text(),
                            TyArg::Nexp(n) => n.to_string_repr(),
                            TyArg::Bool(c) => c.to_text(),
                            TyArg::Order(v) | TyArg::Value(v) => v.clone(),
                        };
                        if text(p) == "_" {
                            return true;
                        }
                        if numeric {
                            let (Some(p), Some(t)) = (
                                crate::ty::NumericExpr::parse(&text(p)),
                                crate::ty::NumericExpr::parse(&text(t)),
                            ) else {
                                return false;
                            };
                            // A nested type application in an Int slot has the
                            // wrong kind (e.g. `bits(int('n))`).
                            if matches!(p, crate::ty::NumericExpr::App { ref name, .. }
                            if matches!(name.as_str(), "int" | "bits" | "vector" | "bool"))
                            {
                                return false;
                            }
                            match super::numeric::numeric_equality(&p, &t, subst, &[]) {
                                super::ConstraintStatus::Satisfied => true,
                                super::ConstraintStatus::Failed => false,
                                super::ConstraintStatus::Unknown => {
                                    let p = constraint::subst_numeric_expr(&p, subst);
                                    let t = constraint::subst_numeric_expr(&t, subst);
                                    if let Some(value) = super::nexp_simplify::try_eval_const(&t) {
                                        if let Some(Err(())) =
                                            super::nexp_simplify::solve_linear(&p, value)
                                        {
                                            return false;
                                        }
                                    }
                                    self.table.push_subtype_obligation(
                                        super::Obligation::WidthEquality(p, t),
                                    );
                                    true
                                }
                            }
                        } else {
                            match (p, t) {
                                (TyArg::Type(p), TyArg::Type(t)) => {
                                    self.match_type_pattern(p, t, subst, depth + 1)
                                }
                                _ => constraint::unify_value(&text(p), &text(t), subst),
                            }
                        }
                    })
            }
            (
                TyKind::App { name, args, .. },
                TyKind::Scalar(crate::ty::Scalar::Int | crate::ty::Scalar::Nat),
            ) if matches!(name.as_str(), "atom" | "int") && args.len() == 1 => true,
            (TyKind::App { name, .. }, TyKind::Scalar(crate::ty::Scalar::Bool))
                if matches!(name.as_str(), "bool" | "atom_bool") =>
            {
                true
            }
            _ => {
                if !constraint::collect_type_bindings(&pattern, &actual, subst) {
                    return false;
                }
                let instantiated = constraint::apply_subst(&pattern, subst);
                self.table.check_type_equality(&instantiated, &actual).is_compatible()
            }
        }
    }

    /// Total width of a concatenation pattern, when every part states one.
    ///
    /// `None` as soon as a part does not: a bare binder or a wildcard has no
    /// width of its own, and the equation cannot be checked without it.
    /// The parts of an `@` chain, left to right.
    fn flatten_concat_parts(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        out: &mut Vec<hir_def::PatId>,
    ) {
        match body.pat(pat_id) {
            Some(Pat::Infix { lhs, op, rhs }) if op == "@" => {
                self.flatten_concat_parts(body, *lhs, out);
                self.flatten_concat_parts(body, *rhs, out);
            }
            _ => out.push(pat_id),
        }
    }

    fn concat_pattern_width(&self, body: &hir_def::Body, pat_id: hir_def::PatId) -> Option<usize> {
        let sum = self.concat_width_expr(body, pat_id)?;
        let simplified = super::nexp_simplify::simplify(&sum);
        super::nexp_simplify::try_eval_const(&simplified)
            .or_else(|| super::nexp_simplify::try_eval_linear_const(&simplified))
            .and_then(|v| usize::try_from(v).ok())
    }

    /// The parts' widths as one numeric expression.
    ///
    /// Added symbolically, so `_ : bits('n) @ _ : bits(64 - 'n)` totals 64 —
    /// concrete even though neither part is. `None` as soon as a part states
    /// no width at all: a bare binder has none, and the equation cannot be
    /// checked without it.
    fn concat_width_expr(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
    ) -> Option<crate::ty::NumericExpr> {
        use crate::ty::NumericExpr as N;
        use hir_def::hir::Literal;
        let konst = |n: usize| Some(N::Const(n as i64));
        match body.pat(pat_id)? {
            Pat::Infix { lhs, op, rhs } if op == "@" => Some(N::Add(
                Box::new(self.concat_width_expr(body, *lhs)?),
                Box::new(self.concat_width_expr(body, *rhs)?),
            )),
            Pat::Literal(Literal::Binary(text)) => {
                konst(text.trim_start_matches("0b").chars().filter(|c| *c != '_').count())
            }
            Pat::Literal(Literal::Hex(text)) => {
                konst(text.trim_start_matches("0x").chars().filter(|c| *c != '_').count() * 4)
            }
            Pat::Literal(Literal::BitZero | Literal::BitOne) => konst(1),
            Pat::Array(items) => konst(items.len()),
            Pat::Index { .. } => konst(1),
            // `imm[31 .. 16]` is sixteen bits. Without this the whole
            // concatenation's width came out unknown, so a mapping clause
            // whose two subranges do not add up to the declared width —
            // `Foo <-> bits(33)` against `imm[31 .. 16] @ imm[15 .. 0]` —
            // was never compared with anything.
            Pat::RangeIndex { start, end, .. } => {
                let parse = |t: &str| t.trim().parse::<i64>().ok();
                match (parse(start), parse(end)) {
                    (Some(a), Some(b)) => konst((a - b).unsigned_abs() as usize + 1),
                    _ => None,
                }
            }
            Pat::Typed { inner, ty } => {
                let annotated = super::type_from_type_text(ty);
                match bitvector_width_expr(&annotated) {
                    Some(e) => Some(e),
                    None => self.concat_width_expr(body, *inner),
                }
            }
            Pat::As { pat, .. } | Pat::AsType { pat, .. } => self.concat_width_expr(body, *pat),
            Pat::Tuple(items) if items.len() == 1 => self.concat_width_expr(body, items[0]),
            _ => None,
        }
    }

    /// Infer types for a pattern, binding variables in `locals`.
    /// Recursively walks the pattern tree, unifying each sub-pattern
    /// with the expected type and recording bindings in the local
    /// environment.
    pub(super) fn bind_pattern_hir(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        expected: &Ty,
        locals: &mut LocalEnv,
    ) {
        self.bind_pattern_hir_with_mutability(body, pat_id, expected, locals, false);
    }

    pub(super) fn bind_mutable_pattern_hir(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        expected: &Ty,
        locals: &mut LocalEnv,
    ) {
        self.bind_pattern_hir_with_mutability(body, pat_id, expected, locals, true);
    }

    fn bind_pattern_hir_with_mutability(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        expected: &Ty,
        locals: &mut LocalEnv,
        mutable: bool,
    ) {
        // Fuel guard
        self.inference_fuel = self.inference_fuel.saturating_sub(1);
        if self.inference_fuel == 0 {
            return;
        }
        // An existential is opened before its contents are matched:
        // `let ('n, 'bytes) = split_misaligned(..)` against `{'n, 'bytes,
        // .. . (int('n), int('bytes))}` is a pair like any other. Matching
        // the *quantifier* against a tuple pattern bound nothing, so `n` and
        // `bytes` — and everything computed from them — had no type.
        // The constructor-payload arm below already does this for its own
        // argument; it belongs at the door.
        let normalized = self.table.normalize_alias_deep(expected);
        let normalized = if !mutable && matches!(body.pat(pat_id), Some(Pat::Bind(_))) {
            self.immutable_boolean_identity(normalized)
        } else {
            normalized
        };
        // Each immutable bounded number has one stable runtime identity.
        // Mutable bindings keep their declared range for subsequent writes.
        // Literal patterns instead filter the scrutinee within their own arm.
        let normalized = if (!mutable
            && matches!(body.pat(pat_id), Some(Pat::Bind(_) | Pat::TypeVar(_)))
            && !matches!(body.pat(pat_id), Some(Pat::Bind(name)) if self.enum_declaring_member(name).is_some()))
            || matches!(body.pat(pat_id), Some(Pat::Literal(parser::Literal::Number(n)))
            if !n.starts_with("0x") && !n.starts_with("0b"))
        {
            if let Some((var, predicate, inner)) = super::refinement::numeric_value(&normalized) {
                if let Some(Pat::Bind(name) | Pat::TypeVar(name)) = body.pat(pat_id) {
                    let mut used = HashSet::new();
                    super::substitution::ty_names(&normalized, &mut used, true);
                    let mut binder = format!("'{}", name.trim_start_matches('\''));
                    if matches!(normalized.kind(), TyKind::Scalar(crate::ty::Scalar::Int)) {
                        binder.push_str("__flow_int");
                    }
                    while used.contains(binder.trim_start_matches('\'')) {
                        binder.push('_');
                    }
                    let mut subst = Subst::default();
                    subst.values.insert(var, binder.clone());
                    Ty::exist(
                        vec![binder],
                        constraint::apply_subst_constraint_expr(&predicate, &subst),
                        constraint::apply_subst(&inner, &subst),
                    )
                } else {
                    Ty::exist(vec![var], predicate, inner)
                }
            } else {
                normalized
            }
        } else {
            normalized
        };
        let expected =
            if matches!(normalized.kind(), TyKind::Exist { .. }) { &normalized } else { expected };
        let opened;
        let expected = match expected.kind() {
            TyKind::Exist { vars, constraint, inner }
                if !matches!(body.pat(pat_id), Some(Pat::Literal(_)))
                    || matches!(body.pat(pat_id), Some(Pat::Literal(parser::Literal::Bool(_))))
                    || matches!(body.pat(pat_id), Some(Pat::Literal(parser::Literal::Number(n)))
                        if !n.starts_with("0x") && !n.starts_with("0b")) =>
            {
                let package = self.table.open_existential(vars, constraint, inner);
                locals.add_constraint(package.constraint);
                opened = package.inner_ty;
                &opened
            }
            _ => expected,
        };
        // Write pattern type to InferenceResult.
        self.result.write_pat_ty(pat_id, expected.clone());
        let Some(pat) = body.pat(pat_id) else { return };
        match pat {
            Pat::Wild | Pat::Missing => {}
            Pat::Literal(lit) => {
                if let parser::Literal::Bool(value) = lit {
                    if let Some(predicate) = super::subtype::bool_value(expected) {
                        // A literal pattern restricts this arm; it does not
                        // require every scrutinee value to equal the literal.
                        locals.add_constraint(if *value {
                            predicate
                        } else {
                            ConstraintExpr::Not(Box::new(predicate))
                        });
                        return;
                    }
                }
                if let parser::Literal::Number(text) = lit {
                    if !text.starts_with("0x") && !text.starts_with("0b") {
                        if let (Some(index), Some(value)) = (
                            super::refinement::singleton(&self.table.normalize_alias_ty(expected)),
                            NumericExpr::integer_literal(text),
                        ) {
                            // A pattern restricts this arm's value; it does
                            // not require the scrutinee to equal the literal
                            // on every path through the function.
                            locals.add_constraint(ConstraintExpr::Compare {
                                lhs: index,
                                op: crate::ty::CompareOp::Eq,
                                rhs: value,
                            });
                            return;
                        }
                    }
                }
                let lit_ty = match (expected.kind(), lit) {
                    (_, parser::Literal::Number(n))
                        if !n.starts_with("0x") && !n.starts_with("0b") =>
                    {
                        NumericExpr::integer_literal(n)
                            .map(|n| Ty::app("atom", vec![TyArg::Nexp(n)], ""))
                            .unwrap_or_else(|| super::infer_literal_type(lit))
                    }
                    _ => super::infer_literal_type(lit),
                };
                if !expected.is_error()
                    && !lit_ty.is_error()
                    && !self.table.unify(expected, &lit_ty)
                {
                    self.result.record_type_mismatch_at(
                        hir_def::ExprOrPatId::PatId(pat_id),
                        expected,
                        &lit_ty,
                    );
                }
            }
            Pat::Bind(name) => {
                if self.env.registers.contains_key(name)
                    || self.env.cross_file_register_names.contains(name)
                {
                    self.invalid_pattern(
                        pat_id,
                        format!("Cannot shadow register in pattern {name}"),
                    );
                }
                // An enum member in a pattern is a *use*, not a binding —
                // upstream's `bind_pat` looks the name up before it binds
                // anything. It lowers to `Pat::Bind` all the same, and this
                // defined it as a fresh local at whatever type the context
                // offered, so `Fthing(Athing, _)` written the wrong way round
                // silently rebound `Athing` as an integer.
                if let Some(enum_name) = self.enum_declaring_member(name) {
                    let member_ty = Ty::named(enum_name);
                    if !expected.is_error() && self.enum_conflict(expected, &member_ty) {
                        self.result.record_type_mismatch_at(
                            hir_def::ExprOrPatId::PatId(pat_id),
                            expected,
                            &member_ty,
                        );
                    }
                    return;
                }
                // Duplicate detection is done per-pattern-tree (not against
                // enclosing scope) in check_pattern_duplicates.
                locals.define(name, expected.clone());
            }
            Pat::TypeVar(name) => {
                locals.define_type_variable(name);
                let numeric = self.table.normalize_alias_deep(expected);
                // Preserve singleton identity directly; equations using the
                // user's spelling would change meaning when that name is shadowed.
                let singleton = match numeric.kind() {
                    TyKind::App { name: ctor, args, .. }
                        if matches!(ctor.as_str(), "atom" | "int" | "implicit")
                            && args.len() == 1 =>
                    {
                        Some(match &args[0] {
                            TyArg::Type(t) => t.display_text(),
                            arg => arg.as_value_str().unwrap_or_default(),
                        })
                    }
                    TyKind::App { name, args, .. }
                        if name == "range" && args.len() == 2 && args[0] == args[1] =>
                    {
                        Some(match &args[0] {
                            TyArg::Type(t) => t.display_text(),
                            arg => arg.as_value_str().unwrap_or_default(),
                        })
                    }
                    _ => None,
                };
                let bound_ty = if let Some(value) = singleton {
                    locals.define_type_binding(name, Ty::param(value));
                    numeric.clone()
                } else if let Some(predicate) = super::subtype::bool_value(&numeric) {
                    locals.define_type_binding(name, Ty::param(predicate.to_text()));
                    numeric.clone()
                } else if constraint::is_numeric_scalar_ty(&numeric) {
                    let variable = NumericExpr::Var(name.clone());
                    if let Some((lo, hi)) = super::expr::concrete_range_bounds(&numeric) {
                        if let (Ok(lo), Ok(hi)) = (i64::try_from(lo), i64::try_from(hi)) {
                            locals.add_constraint(ConstraintExpr::Compare {
                                lhs: NumericExpr::Const(lo),
                                op: crate::ty::CompareOp::Lte,
                                rhs: variable.clone(),
                            });
                            locals.add_constraint(ConstraintExpr::Compare {
                                lhs: variable.clone(),
                                op: crate::ty::CompareOp::Lte,
                                rhs: NumericExpr::Const(hi),
                            });
                        }
                    }
                    Ty::app("atom", vec![TyArg::Nexp(variable)], format!("atom({name})"))
                } else {
                    expected.clone()
                };
                locals.define(name, bound_ty.clone());
                let value_name = name.strip_prefix('\'').unwrap_or(name);
                if value_name != name {
                    locals.define(value_name, bound_ty);
                }
            }
            Pat::Typed { inner, ty } => {
                // The annotation is what the pattern binds at, not whatever
                // the context happened to offer. They differ wherever a
                // pattern annotates a *part*: in
                // `splitting : bits(4) @ 0xFFFFF @ vop : bits(2) @ ..` the
                // parameter is `bits(32)` and each named slice is narrower.
                let annotated = Some(constraint::apply_subst(
                    &super::type_from_type_text(ty),
                    &locals.type_substitution(),
                ))
                .filter(|t| !t.is_error());
                match annotated {
                    Some(ty) => {
                        let mut used = Vec::new();
                        let mut bound = Vec::new();
                        super::expr::collect_type_vars(&ty, &mut used);
                        super::expr::collect_existential_vars(&ty, &mut bound);
                        for name in used {
                            if !bound.contains(&name)
                                && !locals.type_variables.contains(&name)
                                && !self.table.is_rigid(&name)
                            {
                                self.invalid_pattern(
                                    pat_id,
                                    format!("Unbound type variable {name} in annotation"),
                                );
                            }
                        }
                        // Recursive pattern binding opens the quantified
                        // annotation and captures its renamed assumptions.
                        let ty = if matches!(ty.kind(), TyKind::Scalar(crate::ty::Scalar::Bool))
                            && super::subtype::bool_value(expected).is_some()
                        {
                            expected.clone()
                        } else {
                            ty
                        };
                        self.bind_pattern_hir_with_mutability(body, *inner, &ty, locals, mutable)
                    }
                    None => self
                        .bind_pattern_hir_with_mutability(body, *inner, expected, locals, mutable),
                }
            }
            Pat::AsType { pat: inner, ty } => {
                let pattern = super::type_from_type_text(ty);
                let mut bound = Vec::new();
                super::expr::collect_type_vars(&pattern, &mut bound);
                for name in bound {
                    locals.define_type_variable(&name);
                }
                let mut subst = Subst::default();
                if !is_type_pattern_syntax(&pattern, 0) {
                    self.invalid_pattern(
                        pat_id,
                        "Type patterns require binders or wildcards instead of literal arguments",
                    );
                } else if !self.match_type_pattern(&pattern, expected, &mut subst, 0) {
                    let actual = self.expand_type_pattern_alias(expected);
                    let message = if matches!(pattern.kind(), TyKind::Param(_))
                        && matches!(
                            actual.kind(),
                            TyKind::Scalar(
                                crate::ty::Scalar::String
                                    | crate::ty::Scalar::Unit
                                    | crate::ty::Scalar::Real
                            )
                        ) {
                        format!(
                            "Type {} has no numeric expression to bind {}",
                            expected.display_text(),
                            pattern.display_text()
                        )
                    } else {
                        format!(
                            "Type pattern {} does not match type {}",
                            pattern.display_text(),
                            expected.display_text()
                        )
                    };
                    self.invalid_pattern(pat_id, message);
                } else {
                    // Bind the newly introduced names to their actual rigid
                    // identities; textual aliases must not conflate shadowed witnesses.
                    for (name, ty) in &subst.types.entries {
                        locals.define_type_binding(name, ty.clone());
                    }
                    for (name, value) in &subst.values.entries {
                        locals.define_type_binding(name, Ty::param(value));
                    }
                }
                self.bind_pattern_hir_with_mutability(body, *inner, expected, locals, mutable);
            }
            Pat::As { pat, binding } => {
                self.bind_pattern_hir_with_mutability(body, *pat, expected, locals, mutable);
                locals.define(binding, expected.clone());
            }
            Pat::Tuple(items) => {
                // `(p)` is `p` — the same rule the types already follow. A
                // parenthesised parameter pattern such as `function f (0x0)`
                // arrives as a one-element tuple against a non-tuple type,
                // took the else branch below, and bound its contents at
                // `error()`: every literal parameter pattern went unchecked.
                if items.len() == 1 && !matches!(expected.kind(), TyKind::Tuple(_)) {
                    self.bind_pattern_hir_with_mutability(
                        body, items[0], expected, locals, mutable,
                    );
                    return;
                }
                if let TyKind::Tuple(item_tys) = expected.kind() {
                    if items.len() != item_tys.len() {
                        self.invalid_pattern(
                            pat_id,
                            "Tuple pattern and tuple type have different length",
                        );
                    }
                    for (pat_id, ty) in items.iter().zip(item_tys.iter()) {
                        self.bind_pattern_hir_with_mutability(body, *pat_id, ty, locals, mutable);
                    }
                } else {
                    if self.known_type_shape(expected) {
                        self.invalid_pattern(
                            pat_id,
                            "Cannot bind tuple pattern against non tuple type",
                        );
                    }
                    for &pid in items {
                        self.bind_pattern_hir_with_mutability(
                            body,
                            pid,
                            &Ty::error(),
                            locals,
                            mutable,
                        );
                    }
                }
            }
            Pat::App { ctor, args } => {
                let is_constructor = self.env.constructors.contains_key(ctor)
                    || self.env.cross_file_constructor_names.contains(ctor);
                if !is_constructor
                    && self.env.lookup_mappings(ctor).is_empty()
                    && (self.env.functions.contains_key(ctor)
                        || self.env.cross_file_function_names.contains(ctor))
                {
                    self.invalid_pattern(
                        pat_id,
                        format!("{ctor} is not a union constructor or mapping in pattern"),
                    );
                }
                let candidates = self.env.lookup_functions(ctor);

                let scheme = candidates
                    .iter()
                    .find(|s| self.constructor_types_plausible(&s.ret, expected))
                    .or_else(|| candidates.first());
                if let Some(scheme) = scheme {
                    if !self.constructor_types_plausible(&scheme.ret, expected) {
                        self.invalid_pattern(
                            pat_id,
                            format!(
                                "Constructor {ctor} produces {}, but pattern expects {}",
                                scheme.ret.display_text(),
                                expected.display_text()
                            ),
                        );
                    }
                    let mut subst = Subst::default();
                    self.extract_value_bindings(&scheme.ret, expected, &mut subst);
                    let instantiated_ret = constraint::apply_subst(&scheme.ret, &subst);
                    let _ = self.table.unify(&instantiated_ret, expected);
                    // A constructor whose payload is a tuple may be spelled
                    // either way: `Some(ip, p)` destructures the pair that
                    // `Some : 'a -> option('a)` carries as one argument.
                    let flattened;
                    let params: &[Ty] = match scheme.params.first() {
                        Some(only) if scheme.params.len() == 1 && args.len() > 1 => {
                            let only = self.table.resolve(&constraint::apply_subst(only, &subst));
                            // An existential payload is opened before its
                            // contents are matched: `Foo : {'n, 'n in {16,32}.
                            // (int, int, int('n), bits('n))}` is destructured
                            // by `Foo(a, b, _, d)` like any other tuple.
                            let only = match only.kind() {
                                TyKind::Exist { vars, constraint, inner } => {
                                    let opened =
                                        self.table.open_existential(vars, constraint, inner);
                                    locals.add_constraint(opened.constraint);
                                    opened.inner_ty
                                }
                                _ => only,
                            };
                            match only.kind() {
                                TyKind::Tuple(items) if items.len() == args.len() => {
                                    flattened = items.clone();
                                    &flattened
                                }
                                _ => &scheme.params,
                            }
                        }
                        _ => &scheme.params,
                    };
                    for (pid, param_ty) in args.iter().zip(params.iter()) {
                        let ty = constraint::apply_subst(param_ty, &subst);
                        let ty = self.immutable_boolean_identity(ty);
                        self.bind_pattern_hir_with_mutability(body, *pid, &ty, locals, mutable);
                    }
                    // Record variant/constructor resolution for this pattern.
                    // Enables goto-def for constructor patterns (e.g., `Some(x)`).
                    let ctor_name = hir_def::Name::from(ctor.as_str());
                    let per_ns = self.make_resolver().def_map().root_scope().get(&ctor_name);
                    if let Some(ctor_item) = per_ns.values.or(per_ns.types) {
                        self.result.record_variant_resolution(
                            hir_def::ExprOrPatId::PatId(pat_id),
                            ctor_item.def,
                        );
                    }
                } else {
                    let mapping_schemes = self.env.lookup_mappings(ctor);
                    if let Some(ms) = mapping_schemes.first() {
                        // Which side the pattern matches decides which side
                        // its arguments are, and upstream tries them in a
                        // fixed order: `unify typ2 typ` first, so the
                        // sub-pattern is `typ1` — the *left* side — and only
                        // if that fails does it unify `typ1` and give the
                        // sub-pattern `typ2` (`type_check.ml:4390,4406`).
                        //
                        // Both the order and the fallback were the other way
                        // round here, and the fallback is what reported:
                        // with no expectation to decide by, `zero_int_bits(2)`
                        // in `{ v <-> zero_int_bits(2) @ v }` bound the
                        // literal `2` against `bits('m)` — the side the
                        // pattern *matches* — and `e_pair_bits(e1, EB)`
                        // bound `EB : E` against `bits(2)`.
                        // (`test/c/poly_mapping2.sail`,
                        // `test/c/issue243_fixed.sail`, both accepted
                        // upstream.)
                        //
                        // `try_unify` alone cannot order them: it lets a
                        // non-primitive `Named` accept anything, so
                        // `m1 : bits(1) <-> E1` matched against a `bits(1)`
                        // took the `E1` side and gave the sub-pattern
                        // `bits(1)` — the expression path works around the
                        // same permissiveness by comparing head names
                        // (`test/c/nested_mapping.sail`).
                        let expected_head = self.table.normalize_alias_ty(expected);
                        let rhs_matches = mapping_side_matches(
                            &self.table.normalize_alias_ty(&ms.rhs),
                            &expected_head,
                        );
                        let lhs_matches = mapping_side_matches(
                            &self.table.normalize_alias_ty(&ms.lhs),
                            &expected_head,
                        );
                        let payload_ty = if !expected.is_error() {
                            let mut rhs_subst = Subst::default();
                            self.extract_value_bindings(&ms.rhs, expected, &mut rhs_subst);
                            let mut lhs_subst = Subst::default();
                            self.extract_value_bindings(&ms.lhs, expected, &mut lhs_subst);
                            if rhs_matches
                                && self.table.try_unify(
                                    &constraint::apply_subst(&ms.rhs, &rhs_subst),
                                    expected,
                                )
                            {
                                constraint::apply_subst(&ms.lhs, &rhs_subst)
                            } else if lhs_matches
                                && self.table.try_unify(
                                    &constraint::apply_subst(&ms.lhs, &lhs_subst),
                                    expected,
                                )
                            {
                                constraint::apply_subst(&ms.rhs, &lhs_subst)
                            } else {
                                // Neither direction accepts what the pattern
                                // is matching, and upstream stops there:
                                // "Unification error when pattern matching
                                // against mapping constructor"
                                // (`type_check.ml:4417`), naming the last
                                // side it tried — `typ1`, the left one.
                                //
                                // Guarded exactly as the expression path
                                // guards its `NoOverloading`: a side or an
                                // expectation with an inference variable or
                                // a quantifier in it cannot disagree with
                                // anything yet, and reporting on one is how
                                // this check would cost more than it buys.
                                if expected.is_fully_known()
                                    && ms.lhs.is_fully_known()
                                    && ms.rhs.is_fully_known()
                                {
                                    self.result.record_type_mismatch_at(
                                        hir_def::ExprOrPatId::PatId(pat_id),
                                        expected,
                                        &ms.lhs,
                                    );
                                }
                                ms.lhs.clone()
                            }
                        } else {
                            // A concat operand may have no width yet. The
                            // other side's binding still identifies the
                            // mapping's payload direction (e.g. an integer
                            // under a bits <-> integer mapping).
                            let argument = args.first().and_then(|id| match body.pat(*id) {
                                Some(Pat::Bind(name)) => locals.lookup(name).cloned(),
                                _ => None,
                            });
                            if argument.is_some_and(|ty| {
                                let ty = self.table.normalize_alias_deep(&ty);
                                let lhs = self.table.normalize_alias_deep(&ms.lhs);
                                let rhs = self.table.normalize_alias_deep(&ms.rhs);
                                !mapping_side_matches(&lhs, &ty) && mapping_side_matches(&rhs, &ty)
                            }) {
                                ms.rhs.clone()
                            } else {
                                ms.lhs.clone()
                            }
                        };
                        // A tuple payload is destructured across the
                        // pattern's arguments, the same way a constructor's
                        // is: `hex_bits(2, s)` against
                        // `bits('n) <-> (int('n), string)` binds `s` to the
                        // string, not to the whole pair.
                        let resolved = self.table.resolve(&payload_ty);
                        match resolved.kind() {
                            TyKind::Tuple(items) if items.len() == args.len() => {
                                let items = items.clone();
                                for (&pid, ty) in args.iter().zip(items.iter()) {
                                    self.bind_pattern_hir_with_mutability(
                                        body, pid, ty, locals, mutable,
                                    );
                                }
                            }
                            _ => {
                                for &pid in args {
                                    self.bind_pattern_hir_with_mutability(
                                        body,
                                        pid,
                                        &payload_ty,
                                        locals,
                                        mutable,
                                    );
                                }
                            }
                        }
                    } else {
                        for &pid in args {
                            self.bind_pattern_hir_with_mutability(
                                body,
                                pid,
                                &Ty::error(),
                                locals,
                                mutable,
                            );
                        }
                    }
                }
            }
            Pat::List(items) | Pat::Array(items) => {
                let shape = self.table.normalize_alias_deep_for_shape(expected);
                let list_element = match shape.kind() {
                    TyKind::App { name, args, .. } | TyKind::Named(name, args)
                        if name == "list" =>
                    {
                        args.first().and_then(|a| {
                            if let TyArg::Type(t) = a {
                                Some(t.clone())
                            } else {
                                None
                            }
                        })
                    }
                    _ => None,
                };
                if matches!(pat, Pat::List(_))
                    && shape.ctor_name() != Some("list")
                    && self.known_type_shape(&shape)
                {
                    self.invalid_pattern(pat_id, "Cannot match list pattern against non-list type");
                }
                // A vector-literal pattern states its own length, and that
                // has to agree with the width being matched: `[_,_,_,bitzero]`
                // against a `bits(5)` cannot match, and upstream says so
                // ("Failed to prove constraint: 5 == 4"). Nothing compared
                // the two, so the pattern's length was decorative.
                if matches!(pat, Pat::Array(_)) {
                    if let Some(width) = concrete_bitvector_width(expected) {
                        let found = items.len();
                        if width != found {
                            self.result.record_type_mismatch_at(
                                hir_def::ExprOrPatId::PatId(pat_id),
                                expected,
                                &Ty::app(
                                    "bits",
                                    vec![TyArg::numeric(found.to_string())],
                                    format!("bits({found})"),
                                ),
                            );
                        }
                    }
                }
                for &pid in items {
                    self.bind_pattern_hir_with_mutability(
                        body,
                        pid,
                        &list_element.clone().unwrap_or_else(Ty::error),
                        locals,
                        mutable,
                    );
                }
            }
            Pat::Struct { name, fields } => {
                let record_name = name.as_ref().or_else(|| match expected.kind() {
                    TyKind::Named(n, _) => Some(n),
                    TyKind::App { name: n, .. } => Some(n),
                    _ => None,
                });
                let record_info = record_name
                    .and_then(|rn| self.env.records.get(rn.as_str()).cloned())
                    .map(|r| (record_name.unwrap().clone(), r));
                if let Some((rname, record)) = &record_info {
                    let subst = self
                        .record_info_for_type(expected)
                        .filter(|(name, _, _)| name == rname)
                        .map(|(_, _, subst)| subst)
                        .unwrap_or_default();
                    for (fname, pid) in fields {
                        let field_ty = record
                            .fields
                            .get(fname.as_str())
                            .map(|ty| constraint::apply_subst(ty, &subst))
                            .unwrap_or_else(|| {
                                // A field the record does not declare. The
                                // missing-fields check below already reads
                                // this list; the other direction — a name in
                                // the pattern that is not in the record —
                                // typed as `error()` and said nothing, so
                                // renaming a field left every pattern that
                                // matched on it silently unchecked.
                                //
                                // `_` is the wildcard that means "and the
                                // rest", not a field. An empty field list
                                // means the declaration was not understood.
                                if fname != "_" && !record.fields.is_empty() {
                                    self.push_inference_diagnostic(
                                        InferenceDiagnostic::UnknownPatternField {
                                            pat: *pid,
                                            record_name: rname.clone(),
                                            name: fname.clone(),
                                        },
                                    );
                                }
                                Ty::error()
                            });
                        self.bind_pattern_hir_with_mutability(
                            body, *pid, &field_ty, locals, mutable,
                        );
                    }
                    let provided: HashSet<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
                    // A `_` field name is a wildcard meaning "ignore remaining
                    // fields", so skip the missing-fields diagnostic.
                    let has_wildcard = provided.contains("_");
                    let mut missing: Vec<&str> = record
                        .fields
                        .keys()
                        .filter(|k| !provided.contains(k.as_str()))
                        .map(|s| s.as_str())
                        .collect();
                    if !missing.is_empty() && !has_wildcard {
                        missing.sort();
                        let rname = record_name.cloned().unwrap_or_default();
                        self.push_inference_diagnostic(InferenceDiagnostic::MissingPatternFields {
                            pat: pat_id,
                            record_name: rname,
                            missing: missing.iter().map(|s| s.to_string()).collect(),
                            field_types: missing
                                .iter()
                                .map(|name| {
                                    constraint::apply_subst(&record.fields[*name], &subst)
                                        .display_text()
                                })
                                .collect(),
                        });
                    }
                } else {
                    if self.known_type_shape(expected) {
                        self.invalid_pattern(
                            pat_id,
                            "Cannot match struct pattern against non-record type",
                        );
                    }
                    for (_, pid) in fields {
                        self.bind_pattern_hir_with_mutability(
                            body,
                            *pid,
                            &Ty::error(),
                            locals,
                            mutable,
                        );
                    }
                }
            }
            Pat::Infix { lhs, op, rhs } => {
                if op == "@" {
                    // The parts of a concatenation pattern have to add up to
                    // the width being matched. `0x00 @ 0b000 @ _ : bits(2) @
                    // 0x0` is 17 bits and can only match a `bits(17)`;
                    // upstream proves the equation and rejects the rest.
                    // Nothing here had ever added them up.
                    if let (Some(total), Some(expected_width)) = (
                        self.concat_pattern_width(body, pat_id),
                        concrete_bitvector_width(expected),
                    ) {
                        if total != expected_width {
                            self.result.record_type_mismatch_at(
                                hir_def::ExprOrPatId::PatId(pat_id),
                                expected,
                                &Ty::app(
                                    "bits",
                                    vec![TyArg::numeric(total.to_string())],
                                    format!("bits({total})"),
                                ),
                            );
                        }
                    }
                    let mut order_parts = Vec::new();
                    self.flatten_concat_parts(body, pat_id, &mut order_parts);
                    for part in order_parts {
                        self.check_subrange_order(body, part);
                    }
                    let mut subranges = Vec::new();
                    self.collect_vector_subrange_parts_hir(body, pat_id, &mut subranges);
                    // Upstream groups a concatenation's subranges *by the
                    // identifier they bind* and checks each group on its own
                    // (`each_pattern_vector_subranges`,
                    // `type_check.ml:1786-1806`). This merged every name's
                    // pieces into one coverage map and bound only the first
                    // name it saw, at the total width of all of them — so
                    // `imm[15 .. 5] @ rest[4 .. 0]` looked like full coverage
                    // of `bits(16)`, `imm` was bound at `bits(16)` instead of
                    // `bits(11)`, and `rest` was not bound at all.
                    if !subranges.is_empty() {
                        let mut names: Vec<String> = Vec::new();
                        let mut widths: HashMap<String, usize> = HashMap::new();
                        let mut ranges: HashMap<String, Vec<(usize, usize)>> = HashMap::new();
                        let mut unresolved: HashSet<String> = HashSet::new();
                        for (name, width, hi, lo) in &subranges {
                            if !names.iter().any(|n| n == name) {
                                names.push(name.clone());
                            }
                            *widths.entry(name.clone()).or_insert(0) += width;
                            match (hi, lo) {
                                (Some(h), Some(l)) => ranges
                                    .entry(name.clone())
                                    .or_default()
                                    .push((*h.max(l), *h.min(l))),
                                // A piece whose bounds did not resolve says
                                // nothing about the rest, and reporting a gap it
                                // might have filled is how this check first went
                                // wrong.
                                _ => {
                                    unresolved.insert(name.clone());
                                }
                            }
                        }
                        for name in &names {
                            let width = widths.get(name).copied().unwrap_or(0);
                            let bind_ty = if width > 0 {
                                Ty::app(
                                    "bits",
                                    vec![TyArg::numeric(width.to_string())],
                                    format!("bits({width})"),
                                )
                            } else {
                                expected.clone()
                            };
                            locals.define(name, bind_ty);
                            if unresolved.contains(name) {
                                continue;
                            }
                            let Some(mut parts) = ranges.remove(name) else { continue };
                            // Highest first, then merge what touches: upstream
                            // reports on what is *left*, so `v[1 .. 0] @ v[4 .. 3]
                            // @ v[2]` is one range and says nothing.
                            parts.sort_by_key(|p| std::cmp::Reverse(p.0));
                            let mut merged: Vec<(usize, usize)> = Vec::new();
                            for (h, l) in parts {
                                match merged.last_mut() {
                                    Some((_, low)) if *low == h + 1 => *low = l,
                                    _ => merged.push((h, l)),
                                }
                            }
                            let problem = match merged.as_slice() {
                                [(_, 0)] | [] => None,
                                [(_, lowest)] => {
                                    Some(super::SubrangeProblem::NotFromZero { lowest: *lowest })
                                }
                                [_, (second_hi, _), ..] => {
                                    Some(super::SubrangeProblem::Gap { missing: second_hi + 1 })
                                }
                            };
                            if let Some(problem) = problem {
                                self.push_inference_diagnostic(
                                    InferenceDiagnostic::NonContiguousSubrange {
                                        pat: pat_id,
                                        name: name.clone(),
                                        problem,
                                    },
                                );
                            }
                        }
                    } else {
                        // `JAL(imm @ 0b0, rd)` against a `bits(21)` payload:
                        // `0b0` is one bit, so `imm` is the other twenty.
                        // Binding both sides at the error type instead threw
                        // that away — and with it every use of `imm`, which
                        // in a mapping clause is the whole other side.
                        let mut parts = Vec::new();
                        self.flatten_concat_parts(body, pat_id, &mut parts);
                        let widths: Vec<Option<usize>> =
                            parts.iter().map(|&p| self.concat_pattern_width(body, p)).collect();
                        let unknown: Vec<usize> = widths
                            .iter()
                            .enumerate()
                            .filter_map(|(i, w)| w.is_none().then_some(i))
                            .collect();
                        let total = concrete_bitvector_width(expected);
                        let known: usize = widths.iter().flatten().sum();
                        for (i, &p) in parts.iter().enumerate() {
                            let ty = match (widths[i], total) {
                                (Some(w), _) => Some(w),
                                // The one part nobody stated is what is left
                                // over, and only when it is the only one.
                                (None, Some(t)) if unknown.len() == 1 && t >= known => {
                                    Some(t - known)
                                }
                                _ => None,
                            };
                            let bind_ty = match ty {
                                Some(w) => Ty::app(
                                    "bits",
                                    vec![TyArg::numeric(w.to_string())],
                                    format!("bits({w})"),
                                ),
                                None => Ty::error(),
                            };
                            self.bind_pattern_hir_with_mutability(
                                body, p, &bind_ty, locals, mutable,
                            );
                        }
                    }
                } else if op == "::" {
                    let shape = self.table.normalize_alias_deep_for_shape(expected);
                    if shape.ctor_name() != Some("list") && self.known_type_shape(&shape) {
                        self.invalid_pattern(
                            pat_id,
                            "Cannot match cons pattern against non-list type",
                        );
                    }
                    // `h :: t` against `list(T)` binds the head at `T` and
                    // the tail at `list(T)`. Without this both sides were
                    // bound at the error type, and a single-parameter clause
                    // ended up giving the head the whole list type.
                    let elem = match expected.kind() {
                        TyKind::App { name, args, .. } | TyKind::Named(name, args)
                            if name == "list" =>
                        {
                            match args.first() {
                                Some(TyArg::Type(t)) => t.clone(),
                                _ => Ty::error(),
                            }
                        }
                        _ => Ty::error(),
                    };
                    self.bind_pattern_hir_with_mutability(body, *lhs, &elem, locals, mutable);
                    self.bind_pattern_hir_with_mutability(body, *rhs, expected, locals, mutable);
                } else if op == "^^" || op == "^" {
                    // String-append pattern: both sides are strings. `^` is
                    // the spelling sail-riscv's `assembly` clauses use, and
                    // without it a mapping pattern inside one — such as
                    // `hex_bits_signed_6(imm)` — had nothing to resolve
                    // against and bound `imm` to the wrong side.
                    let string_ty = Ty::named("string".to_string());
                    if self.known_type_shape(expected)
                        && !self.table.try_unify(expected, &string_ty)
                    {
                        self.invalid_pattern(
                            pat_id,
                            "Cannot match string-append pattern against non-string type",
                        );
                    }
                    self.bind_pattern_hir_with_mutability(body, *lhs, &string_ty, locals, mutable);
                    self.bind_pattern_hir_with_mutability(body, *rhs, &string_ty, locals, mutable);
                } else {
                    self.bind_pattern_hir_with_mutability(
                        body,
                        *lhs,
                        &Ty::error(),
                        locals,
                        mutable,
                    );
                    self.bind_pattern_hir_with_mutability(
                        body,
                        *rhs,
                        &Ty::error(),
                        locals,
                        mutable,
                    );
                }
            }
            Pat::Index { name, .. } => {
                if self.env.registers.contains_key(name)
                    || self.env.cross_file_register_names.contains(name)
                {
                    self.invalid_pattern(pat_id, "Invalid identifier in vector subrange pattern");
                }
                if is_pattern_binding(name, &self.pattern_constants, self.env.has_workspace_context)
                {
                    locals.define(name, Ty::named("bit".to_string()));
                }
            }
            Pat::RangeIndex { name, start, end } => {
                if self.env.registers.contains_key(name)
                    || self.env.cross_file_register_names.contains(name)
                {
                    self.invalid_pattern(pat_id, "Invalid identifier in vector subrange pattern");
                }
                self.check_subrange_order(body, pat_id);
                if is_pattern_binding(name, &self.pattern_constants, self.env.has_workspace_context)
                {
                    let (start_text, end_text) = (start.as_str(), end.as_str());
                    let width = start_text.parse::<usize>().ok().and_then(|s| {
                        end_text.parse::<usize>().ok().map(|e| {
                            if s >= e {
                                s - e + 1
                            } else {
                                e - s + 1
                            }
                        })
                    });
                    if let Some(w) = width {
                        locals.define(
                            name,
                            Ty::app(
                                "bits",
                                vec![TyArg::numeric(w.to_string())],
                                format!("bits({w})"),
                            ),
                        );
                    } else {
                        locals.define(name, expected.clone());
                    }
                }
            }
        }
    }

    /// `imm[0 .. 2]` under `default Order dec` names no slice.
    ///
    /// `bitvector_typ_from_vector_subrange` raises `Err_vector_subrange`
    /// before it computes a width (`type_check.ml:1763-1777`), and `bind_pat`
    /// reaches it for every `P_vector_subrange` (`:3095`). The width this
    /// checker computes takes the indices in either order, which is why
    /// nothing noticed: a reversed subrange whose width still adds up —
    /// `rest[12 .. 0] @ imm[0 .. 2]` against a `bits(16)` — was accepted in
    /// silence.
    ///
    /// Called from both places a subrange pattern is read: on its own, and as
    /// one part of an `@` chain. The chain does not recurse through
    /// `bind_pattern_hir`, so putting the check only in that arm left the
    /// form the corpus actually writes unchecked.
    fn check_subrange_order(&mut self, body: &hir_def::Body, pat_id: hir_def::PatId) {
        use hir_def::type_error::VectorOrder;
        let Some(Pat::RangeIndex { start, end, .. }) = body.pat(pat_id) else { return };
        let (Ok(s), Ok(e)) = (start.parse::<i64>(), end.parse::<i64>()) else { return };
        let violated = match self.env.vector_order {
            VectorOrder::Dec => s < e,
            VectorOrder::Inc => s > e,
        };
        if violated {
            self.push_inference_diagnostic(InferenceDiagnostic::VectorSubrangeOrder {
                node: hir_def::ExprOrPatId::PatId(pat_id),
                first: s.to_string(),
                second: e.to_string(),
                order: self.env.vector_order,
            });
        }
    }

    /// Collect vector subrange pattern parts from a `@` infix pattern tree.
    /// Returns (name, width, high_index, low_index) for each part.
    pub(super) fn collect_vector_subrange_parts_hir(
        &self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        out: &mut Vec<(String, usize, Option<usize>, Option<usize>)>,
    ) {
        let Some(pat) = body.pat(pat_id) else { return };
        match pat {
            Pat::Infix { lhs, op, rhs } if op == "@" => {
                self.collect_vector_subrange_parts_hir(body, *lhs, out);
                self.collect_vector_subrange_parts_hir(body, *rhs, out);
            }
            Pat::RangeIndex { name, start, end } => {
                let (start_text, end_text) = (start.as_str(), end.as_str());
                let hi = start_text.parse::<usize>().ok();
                let lo = end_text.parse::<usize>().ok();
                let width = hi
                    .and_then(|h| lo.map(|l| if h >= l { h - l + 1 } else { l - h + 1 }))
                    .unwrap_or(0);
                out.push((name.clone(), width, hi, lo));
            }
            // `v[2]` is `P_vector_subrange (v, 2, 2)` upstream
            // (`parser.mly:547`), not a piece with no bounds. Recording it
            // without them made every concatenation containing one
            // unresolvable, which is why the contiguity check had to be
            // skipped whenever one appeared.
            Pat::Index { name, index } => {
                let at = index.trim().parse::<usize>().ok();
                out.push((name.clone(), 1, at, at));
            }
            _ => {}
        }
    }

    /// Check for duplicate bindings within a single pattern tree.
    ///
    /// Collects all `Pat::Bind` names, then reports duplicates.
    /// Excludes enum members/constructors.
    pub(super) fn check_pattern_duplicates(
        &mut self,
        body: &hir_def::Body,
        root_pat: hir_def::PatId,
    ) {
        let mut seen: HashMap<String, hir_def::PatId> = HashMap::new();
        self.collect_pattern_names_for_dup_check(body, root_pat, &mut seen);
    }

    /// The enum that declares `name` as a member, when exactly one does.
    ///
    /// A *plain* enum: `env.unions` is what says so, the same rule
    /// `enum_conflict` follows — a `scattered union` lands in `env.enums`
    /// too and carries a payload.
    fn enum_declaring_member(&self, name: &str) -> Option<String> {
        let mut found = None;
        for (enum_name, members) in &self.env.enums {
            if self.env.unions.contains_key(enum_name) {
                continue;
            }
            if members.iter().any(|m| m == name) {
                if found.is_some() {
                    return None;
                }
                found = Some(enum_name.clone());
            }
        }
        found
    }

    fn collect_pattern_names_for_dup_check(
        &mut self,
        body: &hir_def::Body,
        pat_id: hir_def::PatId,
        seen: &mut HashMap<String, hir_def::PatId>,
    ) {
        let Some(pat) = body.pat(pat_id) else { return };
        match pat.clone() {
            Pat::Bind(name) => {
                // Exclude enum members / constructors
                if !is_pattern_binding(
                    &name,
                    &self.pattern_constants,
                    self.env.has_workspace_context,
                ) {
                    return;
                }
                match seen.entry(name) {
                    std::collections::hash_map::Entry::Occupied(e) => {
                        self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                            pat: pat_id,
                            name: e.key().clone(),
                            in_struct_pattern: false,
                        });
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(pat_id);
                    }
                }
            }
            Pat::Typed { inner, .. } | Pat::AsType { pat: inner, .. } => {
                self.collect_pattern_names_for_dup_check(body, inner, seen);
            }
            Pat::As { pat, binding } => {
                self.collect_pattern_names_for_dup_check(body, pat, seen);
                if is_pattern_binding(
                    &binding,
                    &self.pattern_constants,
                    self.env.has_workspace_context,
                ) {
                    match seen.entry(binding) {
                        std::collections::hash_map::Entry::Occupied(e) => {
                            self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                                pat: pat_id,
                                name: e.key().clone(),
                                in_struct_pattern: false,
                            });
                        }
                        std::collections::hash_map::Entry::Vacant(e) => {
                            e.insert(pat_id);
                        }
                    }
                }
            }
            Pat::Tuple(items) | Pat::List(items) | Pat::Array(items) => {
                for pid in items {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::App { args, .. } => {
                for pid in args {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::Struct { fields, .. } => {
                // Check for duplicate field names
                let mut seen_fields: HashMap<String, hir_def::PatId> = HashMap::new();
                for (fname, pid) in &fields {
                    if let Some(_prev) = seen_fields.get(fname) {
                        self.push_inference_diagnostic(InferenceDiagnostic::DuplicateBinding {
                            pat: *pid,
                            name: fname.clone(),
                            in_struct_pattern: true,
                        });
                    } else {
                        seen_fields.insert(fname.clone(), *pid);
                    }
                }
                for (_, pid) in fields {
                    self.collect_pattern_names_for_dup_check(body, pid, seen);
                }
            }
            Pat::Infix { lhs, rhs, .. } => {
                self.collect_pattern_names_for_dup_check(body, lhs, seen);
                self.collect_pattern_names_for_dup_check(body, rhs, seen);
            }
            _ => {} // Wild, Missing, Literal, Index — no bindings
        }
    }
}

/// The width of a `bits(N)`/`bitvector(N, _)` whose size is a literal number.
///
/// `None` for anything whose width is a quantifier or still unresolved —
/// those cannot disagree with a pattern's length yet.
fn concrete_bitvector_width(ty: &Ty) -> Option<usize> {
    let (name, args) = match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
        _ => return None,
    };
    if name != "bits" && name != "bitvector" {
        return None;
    }
    match args.first()? {
        crate::ty::TyArg::Nexp(n) => n.to_string_repr().parse().ok(),
        crate::ty::TyArg::Value(v) => v.trim().parse().ok(),
        crate::ty::TyArg::Type(_) | crate::ty::TyArg::Bool(_) | crate::ty::TyArg::Order(_) => None,
    }
}

/// The width of a `bits(..)`/`bitvector(..)` as a numeric expression.
fn bitvector_width_expr(ty: &Ty) -> Option<crate::ty::NumericExpr> {
    let (name, args) = match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) => (name, args),
        _ => return None,
    };
    if name != "bits" && name != "bitvector" {
        return None;
    }
    match args.first()? {
        TyArg::Bool(_) | TyArg::Order(_) => None,
        TyArg::Nexp(n) => Some(n.clone()),
        TyArg::Value(v) => {
            crate::ty::NumericExpr::parse(v).or_else(|| super::parse_numeric_expr_text(v))
        }
        // `bits('n)` lowers its width to a *type* parameter, not a numeric
        // one — the two spellings of the same thing, and reading only the
        // numeric one made every quantified width unmeasurable.
        TyArg::Type(t) => match t.kind() {
            TyKind::Param(p) => Some(crate::ty::NumericExpr::Var(p.clone())),
            _ => None,
        },
    }
}

/// Whether a mapping's side could be the one a pattern is matching.
///
/// Only a veto, and only on the shapes that cannot be an alias for one
/// another: a tuple is not a named type, and two different named heads are
/// two different types. Anything with no head — an inference variable, a
/// quantified parameter — says nothing and does not veto, because at that
/// point `try_unify` is the better judge.
fn mapping_side_matches(side: &Ty, expected: &Ty) -> bool {
    if let TyKind::Exist { inner, .. } = side.kind() {
        return mapping_side_matches(inner, expected);
    }
    if let TyKind::Exist { inner, .. } = expected.kind() {
        return mapping_side_matches(side, inner);
    }
    let head = |t: &Ty| match t.kind() {
        TyKind::Named(n, _) | TyKind::App { name: n, .. } => Some(n.clone()),
        TyKind::Scalar(s) => Some(s.name().to_owned()),
        _ => None,
    };
    match (side.kind(), expected.kind()) {
        (TyKind::Tuple(a), TyKind::Tuple(b)) => a.len() == b.len(),
        (TyKind::Tuple(_), _) | (_, TyKind::Tuple(_)) => false,
        _ => match (head(side), head(expected)) {
            (Some(a), Some(b)) => {
                type_pattern_head(&a) == type_pattern_head(&b)
                    || (matches!(a.as_str(), "int" | "atom" | "nat" | "range" | "implicit")
                        && matches!(b.as_str(), "int" | "atom" | "nat" | "range" | "implicit"))
            }
            _ => true,
        },
    }
}

fn type_pattern_head(name: &str) -> &str {
    match name {
        "bits" => "bitvector",
        "atom" => "int",
        "atom_bool" => "bool",
        other => other,
    }
}

pub(super) fn contains_type_pattern(
    body: &hir_def::Body,
    id: hir_def::PatId,
    depth: usize,
) -> bool {
    if depth > 64 {
        return false;
    }
    match body.pat(id) {
        Some(Pat::AsType { .. }) => true,
        Some(Pat::Typed { inner, .. } | Pat::As { pat: inner, .. }) => {
            contains_type_pattern(body, *inner, depth + 1)
        }
        Some(
            Pat::Tuple(items) | Pat::List(items) | Pat::Array(items) | Pat::App { args: items, .. },
        ) => items.iter().any(|&p| contains_type_pattern(body, p, depth + 1)),
        Some(Pat::Struct { fields, .. }) => {
            fields.iter().any(|(_, p)| contains_type_pattern(body, *p, depth + 1))
        }
        Some(Pat::Infix { lhs, rhs, .. }) => {
            contains_type_pattern(body, *lhs, depth + 1)
                || contains_type_pattern(body, *rhs, depth + 1)
        }
        _ => false,
    }
}

/// Source type patterns contain binders, wildcards and applications, never
/// literal numeric arguments. Constants introduced by alias expansion are
/// allowed, so this check runs before expansion.
fn is_type_pattern_syntax(ty: &Ty, depth: usize) -> bool {
    if depth > 64 {
        return true;
    }
    match ty.kind() {
        TyKind::App { args, .. } | TyKind::Named(_, args) => args.iter().all(|arg| match arg {
            TyArg::Type(t) => is_type_pattern_syntax(t, depth + 1),
            TyArg::Nexp(crate::ty::NumericExpr::Var(_)) => true,
            TyArg::Nexp(crate::ty::NumericExpr::Symbol(s)) => s == "_",
            TyArg::Value(v) => v == "_" || v.starts_with('\''),
            _ => false,
        }),
        TyKind::Tuple(items) => items.iter().all(|t| is_type_pattern_syntax(t, depth + 1)),
        _ => true,
    }
}
