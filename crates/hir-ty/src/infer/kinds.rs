//! Kinds shared by lowering, alias substitution and function instantiation.
use super::*;

pub(super) fn declared(node: &syntax::SyntaxNode) -> HashMap<String, Kind> {
    use parser::SyntaxKind as SK;
    let mut out = HashMap::new();
    for param in node.descendants().filter(|n| n.kind() == SK::TYPE_PARAM) {
        let tokens: Vec<_> = param
            .descendants_with_tokens()
            .filter_map(|e| e.into_token())
            .filter(|t| !t.kind().is_trivia())
            .collect();
        let Some(colon) = tokens.iter().position(|t| t.kind() == SK::COLON) else { continue };
        let Some(kind) = tokens.get(colon + 1).and_then(|t| Kind::from_name(t.text())) else {
            continue;
        };
        for token in &tokens[..colon] {
            if token.kind() == SK::TY_VAR {
                out.insert(token.text().to_owned(), kind);
            }
        }
    }
    out
}

pub(super) fn arguments(name: &str, count: usize) -> Option<&'static [Kind]> {
    hir_def::builtin_type_kinds(match (name, count) {
        ("bits", 1) => "bitvector",
        ("vector", 3) => "vector_legacy",
        ("bitvector", 2) => "bitvector_legacy",
        ("int", 1) => "int_applied",
        ("bool", 1) => "bool_applied",
        _ => name,
    })
    .filter(|ks| ks.len() == count)
}

/// Infer omitted binder kinds from constructor positions. Explicit declarations
/// remain authoritative; diagnostics report inconsistent uses separately.
pub(super) fn infer(ty: &Ty, bounds: &mut HashMap<String, Kind>) {
    match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) => {
            for (i, arg) in args.iter().enumerate() {
                let kind = arguments(name, args.len()).and_then(|ks| ks.get(i)).copied();
                let text = match arg {
                    TyArg::Type(t) => t.display_text(),
                    _ => arg.as_value_str().unwrap_or_default(),
                };
                if text.starts_with('\'')
                    && text[1..].chars().all(|c| c.is_alphanumeric() || c == '_')
                {
                    if let Some(k) = kind {
                        bounds.entry(text).or_insert(k);
                    }
                }
                if let TyArg::Type(t) = arg {
                    if !matches!(t.kind(), TyKind::Param(_)) {
                        infer(t, bounds);
                    }
                }
            }
        }
        TyKind::Param(p) => {
            bounds.entry(p.clone()).or_insert(Kind::Type);
        }
        TyKind::Tuple(ts) => ts.iter().for_each(|t| infer(t, bounds)),
        TyKind::FnPtr(sig) => {
            sig.params.iter().for_each(|t| infer(t, bounds));
            infer(&sig.ret, bounds);
        }
        TyKind::Exist { inner, .. } => infer(inner, bounds),
        TyKind::Bidir { lhs, rhs } => {
            infer(lhs, bounds);
            infer(rhs, bounds);
        }
        _ => {}
    }
}

pub(super) fn argument(a: &TyArg, want: Option<Kind>, bounds: &HashMap<String, Kind>) -> TyArg {
    let text = match a {
        TyArg::Type(t) => t.display_text(),
        _ => a.as_value_str().unwrap_or_default(),
    };
    let kind = bounds.get(&text).copied().or(want);
    match kind {
        // ItemTree aliases may spell a Type-kinded binder as a numeric
        // argument. Preserve its kind before substitution inserts a complete
        // type such as bits(8 * 'n), rather than a numeric application string.
        Some(Kind::Type) if matches!(a, TyArg::Nexp(NumericExpr::Var(_))) => {
            TyArg::Type(Ty::param(text))
        }
        Some(Kind::Int) if matches!(a,TyArg::Type(t) if matches!(t.kind(),TyKind::Param(_) | TyKind::Infer(_))) => {
            a.clone()
        }
        Some(Kind::Int) => NumericExpr::parse(&text).map(TyArg::Nexp).unwrap_or_else(|| a.clone()),
        Some(Kind::Bool) => TyArg::Bool(parse_constraint_text(&text)),
        Some(Kind::Order) => TyArg::Order(text),
        _ => match a {
            TyArg::Type(t) => TyArg::Type(normalize(t, bounds)),
            _ => a.clone(),
        },
    }
}

pub(super) fn normalize(ty: &Ty, bounds: &HashMap<String, Kind>) -> Ty {
    match ty.kind() {
        TyKind::App { name, args, .. } | TyKind::Named(name, args) if !args.is_empty() => {
            let kinds = arguments(name, args.len());
            let args: Vec<_> = args
                .iter()
                .enumerate()
                .map(|(i, a)| argument(a, kinds.and_then(|ks| ks.get(i)).copied(), bounds))
                .collect();
            Ty::app(name.clone(), args, "")
        }
        TyKind::Tuple(ts) => Ty::tuple(ts.iter().map(|t| normalize(t, bounds)).collect()),
        TyKind::FnPtr(sig) => Ty::function(
            sig.params.iter().map(|t| normalize(t, bounds)).collect(),
            normalize(&sig.ret, bounds),
        ),
        TyKind::Exist { vars, constraint, inner } => Ty::exist(
            vars.clone(),
            normalize_constraint(constraint, bounds),
            normalize(inner, bounds),
        ),
        TyKind::Bidir { lhs, rhs } => Ty::bidir(normalize(lhs, bounds), normalize(rhs, bounds)),
        _ => ty.clone(),
    }
}

pub(super) fn normalize_constraint(
    c: &ConstraintExpr,
    bounds: &HashMap<String, Kind>,
) -> ConstraintExpr {
    use ConstraintExpr as C;
    match c {
        C::App { name, args } => C::App {
            name: name.clone(),
            args: args.iter().map(|a| argument(a, None, bounds)).collect(),
        },
        C::And(xs) => C::And(xs.iter().map(|c| normalize_constraint(c, bounds)).collect()),
        C::Or(xs) => C::Or(xs.iter().map(|c| normalize_constraint(c, bounds)).collect()),
        C::Not(c) => C::Not(Box::new(normalize_constraint(c, bounds))),
        C::Compare { lhs: NumericExpr::Var(a), op, rhs: NumericExpr::Var(b) }
            if matches!(op, CompareOp::Eq | CompareOp::Neq)
                && bounds.get(a) == Some(&Kind::Bool)
                && bounds.get(b) == Some(&Kind::Bool) =>
        {
            let a = C::BoolVar(a.clone());
            let b = C::BoolVar(b.clone());
            let eq = C::And(vec![
                C::Or(vec![C::Not(Box::new(a.clone())), b.clone()]),
                C::Or(vec![C::Not(Box::new(b)), a]),
            ]);
            if *op == CompareOp::Eq {
                eq
            } else {
                C::Not(Box::new(eq))
            }
        }
        _ => c.clone(),
    }
}

/// Legacy CST indices can carry an Int-kinded variable as a type parameter.
/// Interpret it numerically only at a constructor position declared Int;
/// preserve inference variables so call instantiation can still bind them.
pub(super) fn numeric_argument(arg: &TyArg) -> std::borrow::Cow<'_, TyArg> {
    use std::borrow::Cow;
    match arg {
        TyArg::Type(ty) if matches!(ty.kind(), TyKind::Param(_)) => {
            NumericExpr::parse(&ty.display_text())
                .map(|n| Cow::Owned(TyArg::Nexp(n)))
                .unwrap_or(Cow::Borrowed(arg))
        }
        _ => Cow::Borrowed(arg),
    }
}
