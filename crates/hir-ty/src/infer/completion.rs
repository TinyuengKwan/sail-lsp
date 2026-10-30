//! Bounded semantic compatibility for completion ranking. Uses inference's
//! aliases, substitutions and subtype relation, without running the SMT backend.
use super::*;
use base_db::FileText;
use hir_def::source_order;
use unify::Compatibility;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TypeMatch {
    CouldUnify,
    Exact,
}

pub fn type_match_at(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    expected: &Ty,
    candidate: &Ty,
) -> Option<TypeMatch> {
    let mut names = dependencies::names_in_type(expected);
    names.extend(dependencies::names_in_type(candidate));
    let env = environment(db, file, offset, names)?;
    match_types(&env.0.env, expected, candidate, &[], &[])
}

pub fn return_type_match_at(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    expected: &Ty,
    name: &str,
) -> Option<TypeMatch> {
    let mut names = dependencies::names_in_type(expected);
    names.insert(hir_def::Name::new(name));
    let env = environment(db, file, offset, names)?;
    env.0
        .env
        .lookup_functions(name)
        .iter()
        .filter_map(|scheme| {
            db.unwind_if_revision_cancelled();
            let mut variables = scheme.quantifiers.clone();
            // Include implicit quantification, while leaving existential binders
            // to the subtype relation and keeping configuration constants rigid.
            let mut mentioned = Vec::new();
            expr::collect_type_vars(&scheme.ret, &mut mentioned);
            let mut free = HashSet::new();
            substitution::ty_names(&scheme.ret, &mut free, false);
            variables.extend(
                mentioned.into_iter().filter(|name| free.contains(name.trim_start_matches('\''))),
            );
            variables.sort();
            variables.dedup();
            let constraints: Vec<_> = scheme.constraints.iter().map(|c| c.expr.clone()).collect();
            match_types(&env.0.env, expected, &scheme.ret, &variables, &constraints)
        })
        .max()
}

/// Instantiate a record field from the inferred owner type, preserving aliases
/// and type/numeric arguments. Used for an unfinished field initializer.
pub fn field_type_at(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    owner: &Ty,
    field: &str,
) -> Option<Ty> {
    let env = environment(db, file, offset, dependencies::names_in_type(owner))?;
    let mut table = InferenceTable::with_aliases(env.0.env.type_aliases.clone());
    table.set_alias_schemes(env.0.env.alias_schemes.clone());
    let owner = table.normalize_alias_deep(owner);
    let (name, args) = match owner.kind() {
        TyKind::Named(name, args) | TyKind::App { name, args, .. } => (name, args),
        _ => return None,
    };
    let record = env.0.env.records.get(name)?;
    substitution::apply_arguments(record.fields.get(field)?, &record.params, args)
}

fn environment(
    db: &dyn salsa::Database,
    file: FileText,
    offset: usize,
    names: std::collections::BTreeSet<hir_def::Name>,
) -> Option<&crate::query::ArcTopLevelEnv> {
    Some(if source_order::program(db, file).is_some() {
        dependencies::ordered::environment(
            db,
            source_order::at_offset(db, file, offset)
                .or_else(|| source_order::at_offset(db, file, offset.saturating_sub(1)))?,
            names,
        )
    } else {
        dependencies::callable_environment(db, file, names)
    })
}

/// Inference ids are local to the producing body. Never pass them into a new
/// inference table, where the same number would designate a different variable.
/// Recovery types supply no positive evidence for ranking.
fn has_recovery(ty: &Ty) -> bool {
    match ty.kind() {
        TyKind::Error | TyKind::Infer(_) => true,
        TyKind::Named(_, args) | TyKind::App { args, .. } => args.iter().any(|a| match a {
            TyArg::Type(t) => has_recovery(t),
            TyArg::Nexp(n) => subtype::contains_erased_numeric(n),
            TyArg::Value(value) => {
                NumericExpr::parse(value).is_some_and(|n| subtype::contains_erased_numeric(&n))
            }
            TyArg::Bool(predicate) => obligation_stats::unsupported(predicate),
            TyArg::Order(_) => false,
        }),
        TyKind::Tuple(ts) => ts.iter().any(has_recovery),
        TyKind::FnPtr(sig) => sig.params.iter().any(has_recovery) || has_recovery(&sig.ret),
        TyKind::Exist { inner, .. } => has_recovery(inner),
        TyKind::Bidir { lhs, rhs } => has_recovery(lhs) || has_recovery(rhs),
        _ => false,
    }
}

fn match_types(
    env: &TopLevelEnv,
    expected: &Ty,
    candidate: &Ty,
    quantified: &[String],
    constraints: &[ConstraintExpr],
) -> Option<TypeMatch> {
    if has_recovery(expected) || has_recovery(candidate) {
        return None;
    }
    let mut table = InferenceTable::with_aliases(env.type_aliases.clone());
    table.set_alias_schemes(env.alias_schemes.clone());
    table.set_nominal_types(
        env.records.keys().filter(|n| !env.type_aliases.contains_key(*n)).cloned().collect(),
    );
    let expected = table.normalize_alias_deep(expected);
    let candidate = table.normalize_alias_deep(candidate);

    if has_recovery(&expected) || has_recovery(&candidate) {
        return None;
    }
    table.reserve_source_names(&expected.display_text());
    table.reserve_source_names(&candidate.display_text());
    let subst = table.existential_substitution(
        quantified,
        &ConstraintExpr::And(constraints.to_vec()),
        &candidate,
        &expected,
    );
    let candidate = constraint::apply_subst(&candidate, &subst);
    let exact = if quantified.is_empty() {
        let snapshot = table.snapshot();
        let equal = table.check_type_equality(&expected, &candidate) == Compatibility::Proven;
        table.rollback_to(snapshot);
        equal
    } else {
        false
    };
    let compatibility = table.check_compatibility(&expected, &candidate);
    if matches!(compatibility, Compatibility::Mismatch | Compatibility::Recovery) {
        return None;
    }
    let mut deferred = false;
    let goals = table
        .drain_obligations()
        .into_iter()
        .map(|ob| ob.goal.constraint())
        .chain(constraints.iter().map(|c| substitution::constraint(c, &subst)));
    for goal in goals {
        let goal = table.resolve_constraint(&goal);
        match solver::solve_without_smt(&goal, &Subst::default(), &[]) {
            ConstraintStatus::Failed => return None,
            ConstraintStatus::Unknown => deferred = true,
            ConstraintStatus::Satisfied => {}
        }
    }
    Some(if exact && !deferred { TypeMatch::Exact } else { TypeMatch::CouldUnify })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn check(expected: &str, candidate: &str) -> Option<TypeMatch> {
        match_types(
            &TopLevelEnv::default(),
            &type_from_type_text(expected),
            &type_from_type_text(candidate),
            &[],
            &[],
        )
    }
    #[test]
    fn compares_nested_types_and_numeric_arguments() {
        assert_eq!(check("bits(32)", "bits(64)"), None);
        assert_eq!(check("bits(32)", "bits(32)"), Some(TypeMatch::Exact));
        assert_eq!(check("(int, bool)", "(bool, int)"), None);
        assert_eq!(check("int -> bool", "bool -> int"), None);
        assert_eq!(check("int", "range(0, 10)"), Some(TypeMatch::CouldUnify));
    }
    #[test]
    fn aliases_are_compared_semantically() {
        let mut env = TopLevelEnv::default();
        env.type_aliases.insert("word".into(), type_from_type_text("bits(32)"));
        assert_eq!(
            match_types(&env, &Ty::named("word"), &type_from_type_text("bits(32)"), &[], &[]),
            Some(TypeMatch::Exact)
        );
        assert_eq!(
            match_types(&env, &Ty::named("word"), &type_from_type_text("bits(64)"), &[], &[]),
            None
        );
    }
    #[test]
    fn generic_identity_and_constraints_survive_instantiation() {
        let env = TopLevelEnv::default();
        let candidate = type_from_type_text("('a, 'a)");
        let vars = vec!["'a".into()];
        assert_eq!(
            match_types(&env, &type_from_type_text("(int, bool)"), &candidate, &vars, &[]),
            None
        );
        assert_eq!(
            match_types(&env, &type_from_type_text("(int, int)"), &candidate, &vars, &[]),
            Some(TypeMatch::CouldUnify)
        );
        let candidate = type_from_type_text("bits('n)");
        let vars = vec!["'n".into()];
        let constraints = vec![parse_constraint_text("'n == 64")];
        assert_eq!(
            match_types(&env, &type_from_type_text("bits(32)"), &candidate, &vars, &constraints),
            None
        );
        assert_eq!(
            match_types(&env, &type_from_type_text("bits(64)"), &candidate, &vars, &constraints),
            Some(TypeMatch::CouldUnify)
        );
    }
    #[test]
    fn unknowns_and_foreign_inference_ids_are_not_evidence() {
        let env = TopLevelEnv::default();
        for ty in [
            Ty::error(),
            Ty::new(TyKind::Infer(crate::ty::InferTy(900))),
            Ty::tuple(vec![Ty::error()]),
            Ty::app("bits", vec![TyArg::Value("int".into())], ""),
        ] {
            assert_eq!(match_types(&env, &ty, &ty, &[], &[]), None);
        }
    }
}
