//! Simultaneous, capture-avoiding substitution for types and constraints.
//!
//! Replacements are inserted once, never visited again. Binder entry removes
//! shadowed mappings and alpha-renames only binders that would capture a free
//! variable in an active replacement. Fresh names are local and deterministic.
use super::{parse_constraint_text, Subst};
use crate::ty::{ConstraintExpr, NumericExpr, Ty, TyArg, TyKind};
use std::borrow::Cow;
use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

// Borrow the small inference maps on the common path. Only binder entry
// clones them. Alias arguments also retain their already-parsed numeric AST.
#[derive(Clone)]
struct Bindings<'a> {
    legacy: Cow<'a, Subst>,
    numbers: Vec<(String, NumericExpr)>,
}
impl<'a> Bindings<'a> {
    fn borrowed(s: &'a Subst) -> Self {
        Self { legacy: Cow::Borrowed(s), numbers: Vec::new() }
    }
}
impl Deref for Bindings<'_> {
    type Target = Subst;
    fn deref(&self) -> &Subst {
        &self.legacy
    }
}
impl DerefMut for Bindings<'_> {
    fn deref_mut(&mut self) -> &mut Subst {
        self.legacy.to_mut()
    }
}
fn number_value<'a>(s: &'a Bindings<'_>, name: &str) -> Option<&'a NumericExpr> {
    s.numbers.iter().find_map(|(k, n)| (key(k) == key(name)).then_some(n))
}

pub(super) fn apply_arguments(ty: &Ty, params: &[String], args: &[TyArg]) -> Option<Ty> {
    if params.len() != args.len() {
        return None;
    }
    let mut bindings = Bindings { legacy: Cow::Owned(Subst::default()), numbers: Vec::new() };
    for (name, arg) in params.iter().zip(args) {
        match arg {
            TyArg::Type(t) => {
                bindings.types.insert(name.clone(), t.clone());
            }
            TyArg::Nexp(n) => bindings.numbers.push((name.clone(), n.clone())),
            TyArg::Bool(c) => {
                bindings.values.insert(name.clone(), c.to_text());
            }
            TyArg::Order(v) | TyArg::Value(v) => {
                bindings.values.insert(name.clone(), v.clone());
            }
        }
    }
    Some(apply_inner(ty, &bindings, &mut Fresh::default()))
}

pub(super) fn numeric(expr: &NumericExpr, s: &Subst) -> NumericExpr {
    numeric_inner(expr, &Bindings::borrowed(s))
}
pub(super) fn constraint(expr: &ConstraintExpr, s: &Subst) -> ConstraintExpr {
    constraint_inner(expr, &Bindings::borrowed(s))
}

fn key(name: &str) -> &str {
    name.strip_prefix('\'').unwrap_or(name)
}
fn type_value<'a>(s: &'a Bindings<'_>, name: &str) -> Option<&'a Ty> {
    s.types
        .get(name)
        .or_else(|| s.types.entries.iter().find_map(|(k, v)| (key(k) == key(name)).then_some(v)))
}
fn value_text<'a>(s: &'a Bindings<'_>, name: &str) -> Option<&'a String> {
    s.values
        .get(name)
        .or_else(|| s.values.entries.iter().find_map(|(k, v)| (key(k) == key(name)).then_some(v)))
}
fn replacement_text(s: &Bindings<'_>, name: &str) -> Option<String> {
    number_value(s, name)
        .map(NumericExpr::to_string_repr)
        .or_else(|| value_text(s, name).cloned())
        .or_else(|| type_value(s, name).map(Ty::display_text))
}

/// Variables in legacy textual arguments. Whole tokens only, including fresh
/// names; inserted text is parenthesized to preserve arithmetic precedence.
pub(super) fn map_text(text: &str, mut replace: impl FnMut(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, ch)) = chars.next() {
        if ch == '"' {
            out.push(ch);
            let mut escaped = false;
            for (_, c) in chars.by_ref() {
                out.push(c);
                if c == '"' && !escaped {
                    break;
                }
                escaped = c == '\\' && !escaped;
            }
            continue;
        }
        if ch != '\'' {
            out.push(ch);
            continue;
        }
        let mut end = start + 1;
        while let Some(&(i, c)) = chars.peek() {
            if !c.is_alphanumeric() && c != '_' && c != '#' {
                break;
            }
            chars.next();
            end = i + c.len_utf8();
        }
        let name = &text[start..end];
        if let Some(value) = replace(name) {
            if start == 0 && end == text.len() {
                out.push_str(&value);
            } else {
                out.push('(');
                out.push_str(&value);
                out.push(')');
            }
        } else {
            out.push_str(name);
        }
    }
    out
}
pub(super) fn text_names(text: &str, out: &mut HashSet<String>) {
    map_text(text, |name| {
        out.insert(key(name).to_owned());
        None
    });
}

fn numeric_inner(expr: &NumericExpr, s: &Bindings<'_>) -> NumericExpr {
    use NumericExpr::*;
    match expr {
        Var(name) if number_value(s, name).is_some() => number_value(s, name).unwrap().clone(),
        Var(name) => replacement_text(s, name)
            .map(|v| NumericExpr::parse(&v).unwrap_or(Symbol(v)))
            .unwrap_or_else(|| expr.clone()),
        // Symbols denote global names, not quantified variables. A legacy
        // compound symbol may contain variables, so parse it before visiting.
        Symbol(text) if text.contains('\'') => match NumericExpr::parse(text) {
            Some(parsed) if parsed != *expr => numeric_inner(&parsed, s),
            _ => expr.clone(),
        },
        Const(_) | BigConst(_) | Symbol(_) => expr.clone(),
        Neg(a) => Neg(Box::new(numeric_inner(a, s))),
        Exp(a) => Exp(Box::new(numeric_inner(a, s))),
        Add(a, b) => Add(Box::new(numeric_inner(a, s)), Box::new(numeric_inner(b, s))),
        Sub(a, b) => Sub(Box::new(numeric_inner(a, s)), Box::new(numeric_inner(b, s))),
        Mul(a, b) => Mul(Box::new(numeric_inner(a, s)), Box::new(numeric_inner(b, s))),
        Div(a, b) => Div(Box::new(numeric_inner(a, s)), Box::new(numeric_inner(b, s))),
        Mod(a, b) => Mod(Box::new(numeric_inner(a, s)), Box::new(numeric_inner(b, s))),
        App { name, args } => {
            App { name: name.clone(), args: args.iter().map(|a| numeric_inner(a, s)).collect() }
        }
        If { cond, then_expr, else_expr } => If {
            cond: Box::new(constraint_inner(cond, s)),
            then_expr: Box::new(numeric_inner(then_expr, s)),
            else_expr: Box::new(numeric_inner(else_expr, s)),
        },
    }
}

fn constraint_inner(c: &ConstraintExpr, s: &Bindings<'_>) -> ConstraintExpr {
    use ConstraintExpr::*;
    match c {
        Bool(_) | Unsupported => c.clone(),
        BoolVar(v)
            if value_text(s, v).is_none()
                && number_value(s, v).is_none()
                && type_value(s, v).is_some_and(|t| {
                    matches!(t.kind(), TyKind::Param(_))
                        || matches!(t.kind(), TyKind::Named(_, args) if args.is_empty())
                }) =>
        {
            // Param may carry a predicate or literal as well as a skolem.
            // Preserve unquoted legacy skolem names, but never reinterpret an
            // entire compound formula as one unrelated Boolean atom.
            let text = type_value(s, v).unwrap().display_text();
            let parsed = parse_constraint_text(&text);
            if matches!(parsed, Unsupported)
                && text != crate::ty::UNRESOLVED_VAR
                && text.chars().all(|c| c.is_alphanumeric() || c == '_')
            {
                BoolVar(text)
            } else {
                parsed
            }
        }
        BoolVar(v) => replacement_text(s, v)
            .map(|text| {
                // An inference variable has no boolean value yet. In particular,
                // substituting it must never turn an unknown predicate into true.
                if text == crate::ty::UNRESOLVED_VAR {
                    Unsupported
                } else {
                    parse_constraint_text(&text)
                }
            })
            .unwrap_or_else(|| c.clone()),
        Compare { lhs, op, rhs } => {
            Compare { lhs: numeric_inner(lhs, s), op: *op, rhs: numeric_inner(rhs, s) }
        }
        InSet { value, items } => InSet {
            value: numeric_inner(value, s),
            items: items.iter().map(|i| numeric_inner(i, s)).collect(),
        },
        And(xs) => And(xs.iter().map(|x| constraint_inner(x, s)).collect()),
        Or(xs) => Or(xs.iter().map(|x| constraint_inner(x, s)).collect()),
        Not(x) => Not(Box::new(constraint_inner(x, s))),
        App { name, args } => App {
            name: name.clone(),
            args: args.iter().map(|a| arg(a, s, &mut Fresh::default())).collect(),
        },
    }
}

pub(super) fn apply(ty: &Ty, s: &Subst) -> Ty {
    if s.types.entries.is_empty() && s.values.entries.is_empty() {
        return ty.clone();
    }
    apply_inner(ty, &Bindings::borrowed(s), &mut Fresh::default())
}
#[derive(Default)]
struct Fresh {
    used: HashSet<String>,
    next: usize,
}
impl Fresh {
    fn name(&mut self) -> String {
        loop {
            let name = format!("'_subst{}", self.next);
            self.next += 1;
            if self.used.insert(key(&name).to_owned()) {
                return name;
            }
        }
    }
}
fn arg(a: &TyArg, s: &Bindings<'_>, fresh: &mut Fresh) -> TyArg {
    match a {
        TyArg::Type(t) if matches!(t.kind(), TyKind::Param(name) if number_value(s,name).is_some()) =>
        {
            let TyKind::Param(name) = t.kind() else { unreachable!() };
            TyArg::Nexp(number_value(s, name).unwrap().clone())
        }
        TyArg::Type(t) => TyArg::Type(apply_inner(t, s, fresh)),
        TyArg::Nexp(NumericExpr::Var(v))
            if value_text(s, v).is_none()
                && number_value(s, v).is_none()
                && type_value(s, v).is_some() =>
        {
            let t = type_value(s, v).unwrap();
            if matches!(t.kind(), TyKind::Infer(_)) {
                TyArg::Type(t.clone())
            } else {
                TyArg::Nexp(numeric_inner(&NumericExpr::Var(v.clone()), s))
            }
        }
        TyArg::Nexp(n) => TyArg::Nexp(numeric_inner(n, s)),
        TyArg::Bool(c) => TyArg::Bool(constraint_inner(c, s)),
        TyArg::Order(v) => TyArg::Order(replacement_text(s, v).unwrap_or_else(|| v.clone())),
        TyArg::Value(v) => {
            if let Some(n) = NumericExpr::parse(v) {
                TyArg::Nexp(numeric_inner(&n, s))
            } else {
                TyArg::Value(map_text(v, |name| replacement_text(s, name)))
            }
        }
    }
}
fn apply_inner(ty: &Ty, s: &Bindings<'_>, fresh: &mut Fresh) -> Ty {
    match ty.kind() {
        TyKind::Param(name) => {
            if let Some(t) = type_value(s, name) {
                return t.clone();
            }
            if let Some(n) = number_value(s, name) {
                return Ty::param(n.to_string_repr());
            }
            if let Some(v) = value_text(s, name) {
                return Ty::param(v.clone());
            }
            if let Some(n) = NumericExpr::parse(name) {
                let replaced = numeric_inner(&n, s);
                if replaced != n {
                    return Ty::param(replaced.to_string_repr());
                }
            }
            ty.clone()
        }
        TyKind::Named(name, args) => {
            Ty::adt_with_args(name, args.iter().map(|a| arg(a, s, fresh)).collect())
        }
        TyKind::App { name, args, .. } => {
            let new_args: Vec<_> = args.iter().map(|a| arg(a, s, fresh)).collect();
            if new_args == *args {
                return ty.clone();
            }
            let text = super::numeric::app_text(name, &new_args);
            Ty::app(name, new_args, text)
        }
        TyKind::Tuple(xs) => Ty::tuple(xs.iter().map(|t| apply_inner(t, s, fresh)).collect()),
        TyKind::FnPtr(sig) => Ty::function(
            sig.params.iter().map(|t| apply_inner(t, s, fresh)).collect(),
            apply_inner(&sig.ret, s, fresh),
        ),
        TyKind::Bidir { lhs, rhs } => {
            Ty::bidir(apply_inner(lhs, s, fresh), apply_inner(rhs, s, fresh))
        }
        TyKind::Exist { vars, constraint: predicate, inner } => {
            let bound: HashSet<_> = vars.iter().map(|v| key(v)).collect();
            let mut scoped = s.clone();
            scoped.types.entries.retain(|(name, _)| !bound.contains(key(name)));
            scoped.values.entries.retain(|(name, _)| !bound.contains(key(name)));
            scoped.numbers.retain(|(name, _)| !bound.contains(key(name)));
            let mut used = HashSet::new();
            ty_names(inner, &mut used, false);
            constraint_names(predicate, &mut used);
            let mut captures = HashSet::new();
            for (name, t) in &scoped.types.entries {
                if used.contains(key(name)) {
                    ty_names(t, &mut captures, false);
                }
            }
            for (name, value) in &scoped.values.entries {
                if used.contains(key(name)) {
                    text_names(value, &mut captures);
                }
            }
            for (name, value) in &scoped.numbers {
                if used.contains(key(name)) {
                    numeric_names(value, &mut captures);
                }
            }
            let mut new_vars = vars.clone();
            if vars.iter().any(|v| captures.contains(key(v))) {
                // Reserve both free and bound names, including names in
                // replacements: renaming must not create a new capture.
                ty_names(ty, &mut fresh.used, true);
                for (k, t) in &s.types.entries {
                    fresh.used.insert(key(k).to_owned());
                    ty_names(t, &mut fresh.used, true);
                }
                for (k, v) in &s.values.entries {
                    fresh.used.insert(key(k).to_owned());
                    text_names(v, &mut fresh.used);
                }
                for (k, n) in &s.numbers {
                    fresh.used.insert(key(k).to_owned());
                    numeric_names(n, &mut fresh.used);
                }
                for v in &mut new_vars {
                    if captures.contains(key(v)) {
                        let new = fresh.name();
                        scoped.types.insert(v.clone(), Ty::param(&new));
                        scoped.values.insert(v.clone(), new.clone());
                        *v = new;
                    }
                }
            }
            Ty::exist(
                new_vars,
                constraint_inner(predicate, &scoped),
                apply_inner(inner, &scoped, fresh),
            )
        }
        TyKind::Error | TyKind::Infer(_) | TyKind::Scalar(_) | TyKind::Abstract { .. } => {
            ty.clone()
        }
    }
}

pub(super) fn numeric_names(n: &NumericExpr, out: &mut HashSet<String>) {
    use NumericExpr::*;
    match n {
        Var(v) => {
            out.insert(key(v).to_owned());
        }
        Symbol(s) => text_names(s, out),
        Const(_) | BigConst(_) => {}
        Neg(a) | Exp(a) => numeric_names(a, out),
        Add(a, b) | Sub(a, b) | Mul(a, b) | Div(a, b) | Mod(a, b) => {
            numeric_names(a, out);
            numeric_names(b, out);
        }
        App { args, .. } => args.iter().for_each(|a| numeric_names(a, out)),
        If { cond, then_expr, else_expr } => {
            constraint_names(cond, out);
            numeric_names(then_expr, out);
            numeric_names(else_expr, out);
        }
    }
}
pub(super) fn constraint_names(c: &ConstraintExpr, out: &mut HashSet<String>) {
    use ConstraintExpr::*;
    match c {
        BoolVar(v) => {
            out.insert(key(v).to_owned());
        }
        Compare { lhs, rhs, .. } => {
            numeric_names(lhs, out);
            numeric_names(rhs, out);
        }
        InSet { value, items } => {
            numeric_names(value, out);
            items.iter().for_each(|i| numeric_names(i, out));
        }
        And(xs) | Or(xs) => xs.iter().for_each(|x| constraint_names(x, out)),
        App { args, .. } => args.iter().for_each(|a| argument_names(a, out)),
        Not(x) => constraint_names(x, out),
        Bool(_) | Unsupported => {}
    }
}
pub(super) fn argument_names(arg: &TyArg, out: &mut HashSet<String>) {
    match arg {
        TyArg::Type(t) => ty_names(t, out, false),
        TyArg::Nexp(n) => numeric_names(n, out),
        TyArg::Bool(c) => constraint_names(c, out),
        TyArg::Order(s) | TyArg::Value(s) => text_names(s, out),
    }
}
pub(super) fn ty_names(ty: &Ty, out: &mut HashSet<String>, include_bound: bool) {
    match ty.kind() {
        TyKind::Param(v) => {
            if !key(v).is_empty()
                && key(v).chars().all(|c| c.is_alphanumeric() || c == '_' || c == '#')
            {
                out.insert(key(v).to_owned());
            } else {
                text_names(v, out);
            }
        }
        TyKind::Named(_, args) | TyKind::App { args, .. } => {
            for a in args {
                match a {
                    TyArg::Type(t) => ty_names(t, out, include_bound),
                    TyArg::Nexp(n) => numeric_names(n, out),
                    TyArg::Bool(c) => constraint_names(c, out),
                    TyArg::Order(v) | TyArg::Value(v) => text_names(v, out),
                }
            }
        }
        TyKind::Tuple(xs) => xs.iter().for_each(|t| ty_names(t, out, include_bound)),
        TyKind::FnPtr(sig) => {
            sig.params.iter().for_each(|t| ty_names(t, out, include_bound));
            ty_names(&sig.ret, out, include_bound);
        }
        TyKind::Bidir { lhs, rhs } => {
            ty_names(lhs, out, include_bound);
            ty_names(rhs, out, include_bound);
        }
        TyKind::Exist { vars, constraint, inner } => {
            let mut local = HashSet::new();
            ty_names(inner, &mut local, include_bound);
            constraint_names(constraint, &mut local);
            for v in vars {
                if include_bound {
                    local.insert(key(v).to_owned());
                } else {
                    local.remove(key(v));
                }
            }
            out.extend(local);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crate::infer::{constraint::*, env::AliasScheme, *};
    fn num(s: &str) -> NumericExpr {
        NumericExpr::parse(s).unwrap()
    }
    fn bits(s: &str) -> Ty {
        Ty::app("bits", vec![TyArg::numeric(s)], format!("bits({s})"))
    }
    fn values(entries: &[(&str, &str)]) -> Subst {
        let mut s = Subst::default();
        for (k, v) in entries {
            s.values.insert((*k).into(), (*v).into());
        }
        s
    }
    #[test]
    fn whole_names_and_simultaneous_swaps() {
        assert_eq!(
            apply_subst(&bits("'nn + 'n"), &values(&[("'n", "8")])).display_text(),
            "bits('nn + 8)"
        );
        assert_eq!(
            apply_subst(&bits("'n - 'm"), &values(&[("'n", "'m"), ("'m", "'n")])).display_text(),
            "bits('m - 'n)"
        );
    }
    #[test]
    fn numeric_replacements_are_trees_not_variable_names() {
        assert_eq!(apply_subst_numeric(&num("'n"), &values(&[("'n", "2 + 'm")])), num("2 + 'm"));
        assert_eq!(apply_subst_numeric(&num("'n"), &values(&[("'n", "8")])), NumericExpr::Const(8));
    }
    #[test]
    fn bound_variables_shadow_outer_substitution() {
        let original = Ty::exist(vec!["'n".into()], parse_constraint_text("'n > 0"), bits("'n"));
        assert_eq!(apply_subst(&original, &values(&[("'n", "8")])), original);
    }
    #[test]
    fn free_variables_in_existential_predicates_are_substituted() {
        let original = Ty::exist(vec!["'n".into()], parse_constraint_text("'n < 'm"), bits("'n"));
        let result = apply_subst(&original, &values(&[("'m", "8")]));
        let TyKind::Exist { constraint, .. } = result.kind() else { panic!() };
        assert_eq!(*constraint, parse_constraint_text("'n < 8"));
    }
    #[test]
    fn alpha_renaming_prevents_numeric_capture() {
        let original = Ty::exist(
            vec!["'n".into()],
            parse_constraint_text("'n > 'm"),
            Ty::tuple(vec![bits("'n"), bits("'m")]),
        );
        let result = apply_subst(&original, &values(&[("'m", "'n")]));
        let TyKind::Exist { vars, constraint, inner } = result.kind() else { panic!() };
        assert_ne!(vars[0], "'n");
        assert_eq!(*constraint, parse_constraint_text(&format!("{} > 'n", vars[0])));
        assert_eq!(inner.display_text(), format!("(bits({}), bits('n))", vars[0]));
    }
    #[test]
    fn alpha_renaming_prevents_type_capture() {
        let original = Ty::exist(
            vec!["'a".into()],
            ConstraintExpr::Bool(true),
            Ty::tuple(vec![Ty::param("'a"), Ty::param("'b")]),
        );
        let mut s = Subst::default();
        s.types.insert("'b".into(), Ty::param("'a"));
        let result = apply_subst(&original, &s);
        let TyKind::Exist { vars, inner, .. } = result.kind() else { panic!() };
        assert_ne!(vars[0], "'a");
        assert_eq!(*inner, Ty::tuple(vec![Ty::param(&vars[0]), Ty::param("'a")]));
    }
    #[test]
    fn boolean_substitution_preserves_negation_and_unknown() {
        let c = ConstraintExpr::Not(Box::new(ConstraintExpr::BoolVar("'p".into())));
        assert_eq!(
            apply_subst_constraint_expr(&c, &values(&[("'p", "false")])),
            ConstraintExpr::Not(Box::new(ConstraintExpr::Bool(false)))
        );
        assert_eq!(
            apply_subst_constraint_expr(&c, &values(&[("'p", "'q")])),
            ConstraintExpr::Not(Box::new(ConstraintExpr::BoolVar("'q".into())))
        );
    }
    #[test]
    fn named_numeric_arguments_and_legacy_type_arguments_are_rewritten() {
        let s = values(&[("'n", "8")]);
        assert_eq!(
            apply_subst(&Ty::adt_with_args("box", vec![TyArg::numeric("'n")]), &s),
            Ty::adt_with_args("box", vec![TyArg::numeric("8")])
        );
        assert_eq!(
            apply_subst(&Ty::app("bits", vec![TyArg::Type(Ty::param("'n"))], ""), &s)
                .display_text(),
            "bits(8)"
        );
    }
    #[test]
    fn alias_expansion_substitutes_predicates_without_capture() {
        let body = Ty::exist(vec!["'n".into()], parse_constraint_text("'n < 'm"), bits("'n + 'm"));
        let scheme = AliasScheme {
            params: vec!["'m".into()],
            body,
            kind_bounds: Default::default(),
            config_dependent: false,
        };
        let result = scheme.substitute(&[TyArg::numeric("'n")]).unwrap();
        let TyKind::Exist { vars, constraint, inner } = result.kind() else { panic!() };
        assert_ne!(vars[0], "'n");
        assert_eq!(*constraint, parse_constraint_text(&format!("{} < 'n", vars[0])));
        assert_eq!(inner.display_text(), format!("bits({} + 'n)", vars[0]));
    }
    #[test]
    fn nested_binders_mask_renaming_and_avoid_fresh_name_collisions() {
        let nested = Ty::exist(vec!["'n".into()], parse_constraint_text("'n > 0"), bits("'n"));
        let original = Ty::exist(
            vec!["'n".into()],
            parse_constraint_text("'n > 'm"),
            Ty::tuple(vec![nested.clone(), bits("'m + '_subst0")]),
        );
        let s = values(&[("'m", "'n")]);
        let result = apply_subst(&original, &s);
        assert_eq!(result, apply_subst(&original, &s));
        let TyKind::Exist { vars, inner, .. } = result.kind() else { panic!() };
        assert_eq!(vars, &["'_subst1"]);
        let TyKind::Tuple(parts) = inner.kind() else { panic!() };
        assert_eq!(parts[0], nested);
        assert_eq!(parts[1].display_text(), "bits('n + '_subst0)");
    }
    #[test]
    fn both_nested_scopes_avoid_capture_of_the_inserted_free_variable() {
        let nested = Ty::exist(vec!["'n".into()], parse_constraint_text("'n > 'm"), bits("'m"));
        let original = Ty::exist(vec!["'n".into()], ConstraintExpr::Bool(true), nested);
        let result = apply_subst(&original, &values(&[("'m", "'n")]));
        let TyKind::Exist { vars: outer, inner, .. } = result.kind() else { panic!() };
        let TyKind::Exist { vars, inner, constraint } = inner.kind() else { panic!() };
        assert_ne!(outer, vars);
        assert_ne!(vars[0], "'n");
        assert_eq!(inner.display_text(), "bits('n)");
        assert_eq!(*constraint, parse_constraint_text(&format!("{} > 'n", vars[0])));
    }
    #[test]
    fn unused_and_locally_bound_replacements_do_not_rename_binders() {
        let original = Ty::exist(vec!["'a".into()], ConstraintExpr::Bool(true), Ty::param("'a"));
        let mut s = Subst::default();
        s.types.insert("'b".into(), Ty::param("'a"));
        assert_eq!(apply_subst(&original, &s), original);
        s.types.insert("'b".into(), original.clone());
        let containing = Ty::exist(vec!["'a".into()], ConstraintExpr::Bool(true), Ty::param("'b"));
        assert_eq!(
            apply_subst(&containing, &s),
            Ty::exist(vec!["'a".into()], ConstraintExpr::Bool(true), original)
        );
    }
    #[test]
    fn boolean_capture_is_avoided_in_predicate_and_nested_numeric_condition() {
        let conditional = NumericExpr::If {
            cond: Box::new(ConstraintExpr::BoolVar("'p".into())),
            then_expr: Box::new(num("8")),
            else_expr: Box::new(num("16")),
        };
        let body = Ty::exist(
            vec!["'p".into()],
            ConstraintExpr::BoolVar("'q".into()),
            Ty::app("bits", vec![TyArg::Nexp(conditional)], "bits(conditional)"),
        );
        let result = apply_subst(&body, &values(&[("'q", "'p")]));
        let TyKind::Exist { vars, inner, constraint } = result.kind() else { panic!() };
        assert_ne!(vars[0], "'p");
        assert_eq!(*constraint, ConstraintExpr::BoolVar("'p".into()));
        let TyKind::App { args, .. } = inner.kind() else { panic!() };
        let TyArg::Nexp(NumericExpr::If { cond, .. }) = &args[0] else { panic!() };
        assert_eq!(**cond, ConstraintExpr::BoolVar(vars[0].clone()));
    }
    #[test]
    fn aliases_retain_conditional_numeric_argument_ast_without_reparsing() {
        let number = NumericExpr::If {
            cond: Box::new(ConstraintExpr::BoolVar("'p".into())),
            then_expr: Box::new(num("'n + 1")),
            else_expr: Box::new(num("16")),
        };
        let scheme = AliasScheme {
            params: vec!["'m".into()],
            kind_bounds: Default::default(),
            config_dependent: false,
            body: Ty::exist(
                vec!["'n".into(), "'p".into()],
                parse_constraint_text("'n < 'm"),
                bits("'m"),
            ),
        };
        let result = scheme.substitute(&[TyArg::Nexp(number.clone())]).unwrap();
        let TyKind::Exist { vars, inner, constraint } = result.kind() else { panic!() };
        assert_ne!(vars[0], "'n");
        assert_ne!(vars[1], "'p");
        let TyKind::App { args, .. } = inner.kind() else { panic!() };
        assert_eq!(args, &[TyArg::Nexp(number.clone())]);
        let ConstraintExpr::Compare { rhs, .. } = constraint else { panic!() };
        assert_eq!(*rhs, number);
        assert!(scheme.substitute(&[]).is_none());
    }
    #[test]
    fn unsupported_boolean_operands_are_not_dropped_during_substitution() {
        let predicate = ConstraintExpr::BoolVar("'p".into());
        for text in ["unparsed | false", "unparsed & true", "~(unparsed | false)"] {
            let result = apply_subst_constraint_expr(&predicate, &values(&[("'p", text)]));
            assert_eq!(
                solver::solve(&result, &Subst::default(), &[]),
                ConstraintStatus::Unknown,
                "{text}: {result:?}"
            );
        }
        assert_eq!(
            apply_subst_constraint_expr(&predicate, &values(&[("'p", "~('q#2)")])),
            ConstraintExpr::Not(Box::new(ConstraintExpr::BoolVar("'q#2".into())))
        );
        let mut s = Subst::default();
        s.types.insert("'p".into(), InferenceTable::new().new_type_var());
        assert_eq!(apply_subst_constraint_expr(&predicate, &s), ConstraintExpr::Unsupported);
    }
    #[test]
    fn cyclic_maps_are_simultaneous_and_keep_numeric_precedence() {
        let s = values(&[("'n", "'m + 1"), ("'m", "'n")]);
        assert_eq!(apply_subst_numeric(&num("'n * 'm"), &s), num("('m + 1) * 'n"));
        assert_eq!(apply_subst_numeric(&num("'n"), &values(&[("'n", "'n + 1")])), num("'n + 1"));
    }
    #[test]
    fn legacy_bare_parameter_keys_obey_shadowing() {
        let original = Ty::exist(vec!["'n".into()], parse_constraint_text("'n < 'm"), bits("'n"));
        let result = apply_subst(&original, &values(&[("n", "8"), ("m", "16")]));
        assert_eq!(
            result,
            Ty::exist(vec!["'n".into()], parse_constraint_text("'n < 16"), bits("'n"))
        );
        assert_eq!(apply_subst(&Ty::param("n"), &values(&[("'n", "8")])), Ty::param("8"));
    }
    #[test]
    fn textual_fallback_preserves_strings_and_whole_variable_tokens() {
        let original =
            Ty::adt_with_args("legacy", vec![TyArg::Value(r#"unknown['nn, 'n, "'n"]"#.into())]);
        let expected = Ty::adt_with_args(
            "legacy",
            vec![TyArg::Value(r#"unknown['nn, ('m + 1), "'n"]"#.into())],
        );
        assert_eq!(apply_subst(&original, &values(&[("'n", "'m + 1"), ("'m", "8")])), expected);
    }
    #[test]
    fn function_and_bidirectional_types_share_structural_substitution() {
        let original = Ty::function(vec![bits("'n")], Ty::bidir(Ty::param("'a"), bits("'nn")));
        let mut s = values(&[("'n", "8"), ("'nn", "16")]);
        s.types.insert("'a".into(), Ty::named("bool"));
        assert_eq!(
            apply_subst(&original, &s),
            Ty::function(vec![bits("8")], Ty::bidir(Ty::named("bool"), bits("16")))
        );
    }
    #[test]
    fn skolems_are_consistent_across_predicates_bodies_and_nested_shadowing() {
        let nested = Ty::exist(vec!["'n".into()], parse_constraint_text("'n > 0"), bits("'n"));
        let original = Ty::tuple(vec![bits("'n"), nested.clone()]);
        let result = existential::skolemize(
            &["'n".into(), "'p".into()],
            &ConstraintExpr::And(vec![
                parse_constraint_text("'n > 0"),
                ConstraintExpr::BoolVar("'p".into()),
            ]),
            &original,
            "sk_0_",
        );
        let TyKind::Tuple(parts) = result.inner_ty.kind() else { panic!() };
        assert_eq!(parts[1], nested);
        let TyKind::App { args, .. } = parts[0].kind() else { panic!() };
        assert_eq!(args, &[TyArg::Nexp(NumericExpr::Symbol("sk_0_n".into()))]);
        let ConstraintExpr::And(parts) = result.constraint else { panic!() };
        assert_eq!(parts[1], ConstraintExpr::BoolVar("sk_0_p".into()));
        let ConstraintExpr::Compare { lhs, .. } = &parts[0] else { panic!() };
        assert_eq!(lhs, &NumericExpr::Symbol("sk_0_n".into()));
    }
    #[test]
    fn source_alias_parameters_expand_through_the_shared_engine() {
        let (root, errors) =
            syntax::parse_text("type combined('n : Int, 'nn : Int) = bits('nn + 'n)\ntype bounded('m : Int) = {'n, 'n < 'm. bits('n)}");
        assert!(errors.is_empty(), "{errors:?}");
        let env = TopLevelEnv::from_cst(&root).0;
        let alias = &env.alias_schemes["combined"];
        assert_eq!(alias.params, ["'n", "'nn"]);
        let expanded = alias.substitute(&[TyArg::numeric("4"), TyArg::numeric("8")]).unwrap();
        assert_eq!(expanded.display_text(), "bits(8 + 4)");
        let bounded = env.alias_schemes["bounded"].substitute(&[TyArg::numeric("'n")]).unwrap();
        let TyKind::Exist { vars, constraint, inner } = bounded.kind() else {
            panic!("{bounded:?}")
        };
        assert_ne!(vars[0], "'n");
        assert_eq!(*constraint, parse_constraint_text(&format!("{} < 'n", vars[0])));
        assert_eq!(inner.display_text(), format!("bits({})", vars[0]));
    }
}
