use super::*;
fn n(s: &str) -> NumericExpr {
    NumericExpr::parse(s).unwrap()
}
fn c(s: &str) -> ConstraintExpr {
    parse_constraint_text(s)
}
#[test]
fn unbounded_literals_are_constants_and_round_trip() {
    for text in [
        "18446744073709551616",
        "-340282366920938463463374607431768211457",
        "2 ^ 200 + 1",
        "if true then 2 ^ 200 else 0",
    ] {
        let expr = n(text);
        assert_eq!(NumericExpr::parse(&expr.to_string_repr()), Some(expr));
    }
    assert!(matches!(n("18446744073709551616"), NumericExpr::BigConst(_)));
}
#[test]
fn arbitrary_integer_arithmetic_works_without_smt() {
    for goal in [
        "2 ^ 200 > 2 ^ 199",
        "(2 ^ 200 + 3) % 7 == 0",
        "-18446744073709551616 / 3 == -6148914691236517206",
        "-18446744073709551616 % 3 == 2",
        "2 ^ 200 / 2 ^ 199 == 2",
    ] {
        assert_eq!(
            solver::solve_without_smt(&c(goal), &Subst::default(), &[]),
            ConstraintStatus::Satisfied,
            "{goal}"
        );
    }
}
#[test]
fn arbitrary_coefficients_cancel_and_solve_integer_witnesses() {
    let a = n("18446744073709551616 * 'n + 1");
    let b = n("1 + 'n * 18446744073709551616");
    assert_eq!(
        numeric::numeric_equality_without_binding(&a, &b, &Subst::default(), &[]),
        ConstraintStatus::Satisfied
    );
    let mut subst = Subst::default();
    assert!(numeric::unify_numeric_expr(
        &n("18446744073709551616 * 'n"),
        &n("36893488147419103232"),
        &mut subst
    ));
    assert_eq!(subst.values.get("'n").map(String::as_str), Some("2"));
}
#[test]
fn division_by_zero_and_resource_exhaustion_remain_unknown() {
    for text in ["1 / 0", "1 % 0", "2 ^ 99999999999999999999999"] {
        assert!(integer::eval(&n(text), &Subst::default(), &[]).is_none());
    }
}
#[test]
fn contradictory_assumptions_make_unreachable_goals_vacuous() {
    for facts in
        [vec![c("'n > 4"), c("'n < 3")], vec![c("'a < 'b"), c("'b <= 'a")], vec![c("false")]]
    {
        assert_eq!(
            solver::solve_without_smt(&c("false"), &Subst::default(), &facts),
            ConstraintStatus::Satisfied
        );
    }
    assert_eq!(
        solver::solve_without_smt(&c("false"), &Subst::default(), &[c("'n > 4")]),
        ConstraintStatus::Failed
    );
}
#[test]
#[cfg(feature = "z3-solver")]
fn exhausted_budget_is_unknown_and_not_cached_as_proof() {
    use crate::solver_budget::{SolverBudget, SolverLimits};
    let goal = c("'budget_audit_n * 'budget_audit_n >= 0");
    let limited = SolverBudget::enter(
        CancellationToken::never(),
        SolverLimits { checks: 0, ..Default::default() },
    );
    assert_eq!(z3_solver::try_solve(&goal, &Subst::default(), &[]), ConstraintStatus::Unknown);
    drop(limited);
    let _retry = SolverBudget::enter(
        CancellationToken::never(),
        SolverLimits {
            total: std::time::Duration::from_secs(5),
            per_check: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    );
    assert_eq!(z3_solver::try_solve(&goal, &Subst::default(), &[]), ConstraintStatus::Satisfied);
}

fn ty(s: &str) -> Ty {
    let (root, _) = syntax::parse_text(&format!("val audit : unit -> {s}"));
    TopLevelEnv::from_cst(&root).0.functions["audit"][0].ret.clone()
}
#[test]
fn predicate_applications_keep_nested_numeric_and_boolean_arguments() {
    let expr = c("pred(2 ^ 100 + 1, ('b & true), 'n > 0)");
    let ConstraintExpr::App { args, .. } = &expr else { panic!("{expr:?}") };
    assert!(matches!(args.as_slice(), [TyArg::Nexp(_), TyArg::Bool(_), TyArg::Bool(_)]));
    assert_eq!(c(&expr.to_text()), expr);
    let mut subst = Subst::default();
    subst.values.insert("'b".into(), "false".into());
    let replaced = substitution::constraint(&expr, &subst);
    assert!(!replaced.to_text().contains("'b"));
}
#[test]
fn boolean_synonyms_expand_arguments_without_synthetic_equalities() {
    let synonyms = HashMap::from([("both".into(), (vec!["'p".into(), "'q".into()], c("'p & 'q")))]);
    for (text, status) in [
        ("both(true, 2 ^ 100 > 0)", ConstraintStatus::Satisfied),
        ("both(true, false)", ConstraintStatus::Failed),
    ] {
        let expanded = constraint::expand_constraint_synonyms(&c(text), &synonyms, 0);
        assert_eq!(
            solver::solve_without_smt(&expanded, &Subst::default(), &[]),
            status,
            "{expanded:?}"
        );
    }
}
#[test]
fn kinds_survive_scheme_lowering_and_alias_substitution() {
    let (root,_) = syntax::parse_text("type choice('p : Bool, 'n : Int) = (bool('p), bits('n))\nval f : forall ('p : Bool) ('n : Int) ('a : Type) ('o : Order). (bool('p), bits('n), 'a) -> choice('p, 'n)");
    let env = TopLevelEnv::from_cst(&root).0;
    let scheme = &env.functions["f"][0];
    for (v, k) in [("'p", Kind::Bool), ("'n", Kind::Int), ("'a", Kind::Type), ("'o", Kind::Order)] {
        assert_eq!(scheme.kind_bounds.get(v), Some(&k));
    }
    let alias = &env.alias_schemes["choice"];
    let instantiated = alias.substitute(&[TyArg::Bool(c("true")), TyArg::numeric("8")]).unwrap();
    assert!(!instantiated.display_text().contains("'p"));
    assert!(alias.substitute(&[TyArg::numeric("8"), TyArg::Bool(c("true"))]).is_none());
}
#[test]
fn joins_preserve_large_ranges_and_dependent_alternatives() {
    let joined = refinement::join(
        &ty("range(0, 18446744073709551616)"),
        &ty("range(-1, 18446744073709551617)"),
    )
    .unwrap();
    assert_eq!(joined, ty("range(-1, 18446744073709551617)"));
    let a = ty("bits(8)");
    let b = ty("bits(16)");
    let joined = refinement::join(&a, &b).unwrap();
    let mut table = InferenceTable::new();
    assert!(!matches!(subtype::is_subtype(&mut table, &a, &joined), subtype::SubtypeResult::Fail));
    assert!(!matches!(subtype::is_subtype(&mut table, &b, &joined), subtype::SubtypeResult::Fail));
    assert!(refinement::join(&ty("(int, bool)"), &ty("(int, int)")).is_none());
}
#[test]
fn configuration_aliases_expand_without_inventing_a_width() {
    let (root, _) =
        syntax::parse_text("type xlen : Int\ntype word = bits(xlen)\nval f : word -> bits(xlen)");
    let env = TopLevelEnv::from_cst(&root).0;
    let mut table = InferenceTable::with_aliases(env.type_aliases.clone());
    table.set_alias_schemes(env.alias_schemes.clone());
    assert_eq!(table.normalize_alias_deep(&Ty::named("word")), ty("bits(xlen)"));
    assert!(matches!(
        subtype::is_subtype(&mut table, &Ty::named("word"), &ty("bits(64)")),
        subtype::SubtypeResult::NeedsConstraint(_)
    ));
}
#[test]
#[cfg(feature = "z3-solver")]
fn opaque_boolean_predicates_obey_congruence_but_do_not_choose_arguments() {
    // This tests proof semantics; timeout behavior has separate bounded tests.
    let _budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        crate::solver_budget::SolverLimits {
            total: std::time::Duration::from_secs(5),
            per_check: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    );
    let a = c("pred(8, true)");
    let b = c("pred(16, true)");
    assert_eq!(
        z3_solver::try_solve(&a, &Subst::default(), &[a.clone()]),
        ConstraintStatus::Satisfied
    );
    assert_eq!(z3_solver::try_solve(&b, &Subst::default(), &[a]), ConstraintStatus::Unknown);
    assert_eq!(
        z3_solver::try_solve(&c("'p | not('p)"), &Subst::default(), &[]),
        ConstraintStatus::Satisfied
    );
}

#[test]
fn set_members_keep_signs_and_entire_unbounded_expressions() {
    let expr = c("'n in {-1, 2 ^ 100, 18446744073709551616}");
    let ConstraintExpr::InSet { items, .. } = &expr else { panic!("{expr:?}") };
    assert_eq!(items.len(), 3);
    for value in ["-1", "2 ^ 100", "18446744073709551616"] {
        let mut subst = Subst::default();
        subst.values.insert("'n".into(), value.into());
        assert_eq!(solver::solve_without_smt(&expr, &subst, &[]), ConstraintStatus::Satisfied);
    }
    for facts in [vec![c("'n==8"), c("'n!=8")], vec![c("'a=='b"), c("'a!='b")]] {
        assert_eq!(
            solver::solve_without_smt(&c("false"), &Subst::default(), &facts),
            ConstraintStatus::Satisfied
        );
    }
}

#[test]
fn global_sizeof_retains_symbolic_value_only_for_immutable_bindings() {
    let (root, _) = syntax::parse_text("type width : Int = config base.width\nlet size = sizeof(width)\nvar mutable_size = sizeof(width)");
    let env = TopLevelEnv::from_cst(&root).0;
    assert_eq!(env.values.get("size"), Some(&ty("atom(width)")));
    assert!(env.values.get("mutable_size").unwrap().is_error());
}

#[test]
fn conditional_alternatives_prove_only_properties_of_every_branch() {
    for (goal, expected) in [
        ("(if 'b then 4 else 8) in {4, 8}", ConstraintStatus::Satisfied),
        ("16 <= (if 'b then 8 else 4) * 8", ConstraintStatus::Satisfied),
        ("(if 'b then 4 else 8) == 4", ConstraintStatus::Unknown),
        ("(if 'b then 4 else 8) in {4}", ConstraintStatus::Unknown),
        ("(if 'b then 4 else 8) in {5}", ConstraintStatus::Failed),
    ] {
        assert_eq!(solver::solve_without_smt(&c(goal), &Subst::default(), &[]), expected, "{goal}");
    }
}

#[test]
fn symbolic_chained_facts_survive_substitution_and_opaque_neighbors() {
    let mut subst = Subst::default();
    subst.values.insert("'fresh".into(), "'n".into());
    let facts = vec![c("0 < 'n & 'n <= limit"), ConstraintExpr::Unsupported];
    assert_eq!(
        solver::solve_without_smt(&c("0 < 'fresh & 'fresh <= limit"), &subst, &facts),
        ConstraintStatus::Satisfied
    );
    // A negated conjunction does not make either positive conjunct true.
    assert_eq!(
        solver::solve_without_smt(
            &c("'n <= limit"),
            &Subst::default(),
            &[c("not(0 < 'n & 'n <= limit)")]
        ),
        ConstraintStatus::Unknown
    );
}

#[test]
fn range_value_proofs_do_not_choose_a_favorable_member() {
    let facts = vec![c("'n >= 0 & 'm >= 2 ^ 'n"), c("0 <= 'shift & 'shift <= 2 ^ 'n - 1")];
    assert_eq!(
        solver::solve_without_smt(&c("'m >= 'shift & 'shift >= 0"), &Subst::default(), &facts),
        ConstraintStatus::Satisfied
    );
    assert_eq!(
        solver::solve_without_smt(&c("'shift == 0"), &Subst::default(), &facts),
        ConstraintStatus::Unknown
    );
    assert_eq!(
        solver::solve_without_smt(
            &c("'shift <= 0"),
            &Subst::default(),
            &[c("0 <= 'shift & 'shift <= 31")]
        ),
        ConstraintStatus::Unknown
    );
}

#[test]
fn numeric_sets_and_exclusions_do_not_widen_to_inexact_intervals() {
    for text in ["{8, 16, 32}", "{-3, -1, 2}", "{'n, 0 <= 'n <= 10 & 'n != 5. atom('n)}"] {
        assert!(matches!(type_from_type_text(text).kind(), TyKind::Exist { .. }), "{text}");
    }
    assert_eq!(type_from_type_text("{0, 1, 2}"), type_from_type_text("range(0, 2)"));
}
