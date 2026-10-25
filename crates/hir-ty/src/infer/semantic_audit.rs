//! Counterexamples from the subtype/dependent-constraint audit.
use super::*;
fn n(s: &str) -> NumericExpr {
    NumericExpr::parse(s).unwrap()
}
fn ty(s: &str) -> Ty {
    let (root, _) = syntax::parse_text(&format!("val audit : unit -> {s}"));
    TopLevelEnv::from_cst(&root).0.functions["audit"][0].ret.clone()
}

#[test]
fn numeric_division_matches_sail_euclidean_semantics() {
    for (expression, expected) in [("-5 / 2", -3), ("-5 % 2", 1), ("5 / -2", -2), ("-5 % -2", 1)] {
        let e = n(expression);
        assert_eq!(
            numeric::eval_numeric_expr(&e, &Subst::default(), &[]),
            Some(expected),
            "{expression}"
        );
        assert_eq!(nexp_simplify::try_eval_const(&e), Some(expected), "{expression}");
        assert_eq!(
            constraint::polynomial_from_numeric_expr(&e).and_then(|p| p.as_constant()),
            Some(expected),
            "{expression}"
        );
    }
}
#[test]
fn zero_divisors_are_unknown_without_panicking() {
    for text in ["1 / 0", "1 % 0"] {
        assert_eq!(numeric::eval_numeric_expr(&n(text), &Subst::default(), &[]), None);
    }
}
#[test]
fn arithmetic_overflow_is_not_a_negative_width() {
    for e in [
        n("9223372036854775807 + 1"),
        n("2 ^ 63"),
        NumericExpr::Neg(Box::new(NumericExpr::Const(i64::MIN))),
    ] {
        assert_eq!(numeric::eval_numeric_expr(&e, &Subst::default(), &[]), None);
        assert_eq!(nexp_simplify::try_eval_const(&e), None);
        assert!(constraint::polynomial_from_numeric_expr(&e).is_none());
    }
}
#[test]
fn coercion_does_not_reverse_a_failed_subtype_relation() {
    let mut table = InferenceTable::new();
    assert!(coerce::try_coerce(&mut table, &ty("range(0, 1)"), &ty("range(0, 10)")).is_ok());
    assert!(!coerce::try_coerce(&mut table, &ty("range(0, 10)"), &ty("range(0, 1)")).is_ok());
}
#[test]
fn tuples_check_components_after_a_deferred_width() {
    let mut table = InferenceTable::new();
    let sub = Ty::tuple(vec![ty("bits('n)"), Ty::named("bool")]);
    let sup = Ty::tuple(vec![ty("bits('m)"), Ty::named("int")]);
    assert!(matches!(subtype::is_subtype(&mut table, &sub, &sup), subtype::SubtypeResult::Fail));
}
#[test]
fn symbolic_range_inclusion_is_an_obligation_not_a_proof() {
    let mut table = InferenceTable::new();
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("range(0, 'n)"), &ty("range(0, 'm)")),
        subtype::SubtypeResult::NeedsConstraint(_)
    ));
}
#[test]
fn function_subtyping_is_contravariant_in_parameters() {
    let mut table = InferenceTable::new();
    let sub = Ty::function(vec![ty("range(0, 10)")], ty("range(0, 1)"));
    let sup = Ty::function(vec![ty("range(0, 1)")], ty("range(0, 10)"));
    assert!(matches!(subtype::is_subtype(&mut table, &sub, &sup), subtype::SubtypeResult::Ok));
    assert!(matches!(subtype::is_subtype(&mut table, &sup, &sub), subtype::SubtypeResult::Fail));
}
#[test]
fn negating_an_unknown_existential_constraint_stays_unknown() {
    let mut table = InferenceTable::new();
    let c = ConstraintExpr::Not(Box::new(ConstraintExpr::Unsupported));
    assert!(matches!(
        existential::extract_witnesses(&[], &c, &Ty::named("bool"), &Ty::named("bool"), &mut table),
        existential::WitnessResult::Deferred { .. }
    ));
}
#[test]
#[cfg(feature = "z3-solver")]
fn opaque_numeric_applications_keep_their_arguments() {
    let app =
        |k| NumericExpr::App { name: "audit_opaque".into(), args: vec![NumericExpr::Const(k)] };
    let goal = ConstraintExpr::Compare { lhs: app(1), op: CompareOp::Eq, rhs: app(2) };
    assert_eq!(z3_solver::try_solve(&goal, &Subst::default(), &[]), ConstraintStatus::Unknown);
}
#[test]
#[cfg(feature = "z3-solver")]
fn exponential_facts_do_not_overflow_or_restrict_the_exponent() {
    for (goal, assumptions) in
        [("2 ^ 'n == 1", "'n == 64"), ("2 ^ 'n < 0", "'n > 64"), ("2 ^ 63 < 0", "true")]
    {
        assert_eq!(
            z3_solver::try_solve(
                &parse_constraint_text(goal),
                &Subst::default(),
                &[parse_constraint_text(assumptions)]
            ),
            ConstraintStatus::Failed,
            "{goal} under {assumptions}"
        );
    }
}

#[test]
fn numeric_subtypes_obey_set_inclusion() {
    for (sub, sup, accepted) in [
        ("nat", "int", true),
        ("int", "nat", false),
        ("atom(5)", "range(0, 10)", true),
        ("atom(-1)", "nat", false),
        ("range(0, 10)", "atom(5)", false),
        ("range(2, 1)", "atom(5)", true),
        ("range(0, 10)", "int", true),
        ("int", "range(0, 10)", false),
    ] {
        let result = subtype::is_subtype(&mut InferenceTable::new(), &ty(sub), &ty(sup));
        assert_eq!(
            matches!(result, subtype::SubtypeResult::Ok),
            accepted,
            "{sub} <: {sup}: {result:?}"
        );
        if !accepted {
            assert!(matches!(result, subtype::SubtypeResult::Fail));
        }
    }
}

#[test]
fn boolean_refinements_do_not_allow_narrowing_without_evidence() {
    for (sub, sup, accepted) in [
        ("atom_bool(true)", "bool", true),
        ("bool", "atom_bool(true)", false),
        ("atom_bool(true)", "atom_bool(false)", false),
    ] {
        let result = subtype::is_subtype(&mut InferenceTable::new(), &ty(sub), &ty(sup));
        assert_eq!(
            matches!(result, subtype::SubtypeResult::Ok),
            accepted,
            "{sub} <: {sup}: {result:?}"
        );
    }
}

#[test]
fn tuples_retain_every_deferred_width() {
    let mut table = InferenceTable::new();
    let sub = Ty::tuple(vec![ty("bits('a)"), ty("bits('b)")]);
    let sup = Ty::tuple(vec![ty("bits('c)"), ty("bits('d)")]);
    let subtype::SubtypeResult::NeedsConstraint(Obligation::NumericConstraint(
        ConstraintExpr::And(goals),
    )) = subtype::is_subtype(&mut table, &sub, &sup)
    else {
        panic!("expected both goals")
    };
    assert_eq!(goals.len(), 2);
}

#[test]
fn failed_subtyping_rolls_back_earlier_component_bindings() {
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let sub = Ty::tuple(vec![variable.clone(), Ty::named("bool")]);
    let sup = Ty::tuple(vec![Ty::named("int"), Ty::named("int")]);
    assert!(matches!(subtype::is_subtype(&mut table, &sub, &sup), subtype::SubtypeResult::Fail));
    assert_eq!(table.resolve(&variable), variable);
    assert!(table.drain_obligations().is_empty());
}

#[test]
fn applications_preserve_constructor_arity_and_numeric_arguments() {
    let mut table = InferenceTable::new();
    let named = Ty::adt_with_args("bits", vec![TyArg::numeric("8")]);
    assert!(matches!(
        subtype::is_subtype(&mut table, &named, &ty("bits(16)")),
        subtype::SubtypeResult::Fail
    ));
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("list(range(0, 1))"), &ty("list(range(0, 10))")),
        subtype::SubtypeResult::Ok
    ));
    let wrong_arity = Ty::app("bits", vec![], "bits()");
    assert!(matches!(
        subtype::is_subtype(&mut table, &wrong_arity, &ty("bits(8)")),
        subtype::SubtypeResult::Fail
    ));
}

#[test]
fn existential_membership_with_unknown_members_is_deferred() {
    let mut table = InferenceTable::new();
    let c = ConstraintExpr::InSet { value: NumericExpr::Const(1), items: vec![n("'x")] };
    assert!(matches!(
        existential::extract_witnesses(&[], &c, &Ty::named("bool"), &Ty::named("bool"), &mut table),
        existential::WitnessResult::Deferred { .. }
    ));
}

#[test]
fn cyclic_numeric_substitutions_terminate() {
    let mut subst = Subst::default();
    subst.values.insert("'a".into(), "'b".into());
    subst.values.insert("'b".into(), "'a".into());
    assert_eq!(numeric::eval_numeric_expr(&n("'a"), &subst, &[]), None);
    assert_eq!(constraint::subst_numeric_expr(&n("'a"), &subst), n("'a"));
    assert_eq!(
        solver::solve(&parse_constraint_text("'a == 1"), &subst, &[]),
        ConstraintStatus::Unknown
    );
}

#[test]
fn linear_and_polynomial_overflow_is_deferred() {
    let e = n("9223372036854775807 * 'n + 'n");
    assert!(nexp_simplify::as_linear(&e).is_none());
    assert!(constraint::polynomial_from_numeric_expr(&e).is_none());
    assert!(nexp_simplify::solve_linear(&n("-'n"), i64::MIN).is_none());
}

#[test]
#[cfg(feature = "z3-solver")]
fn solver_cache_distinguishes_nested_application_shapes() {
    // This tests proof semantics; timeout behavior has separate bounded tests.
    let _budget = crate::solver_budget::SolverBudget::enter(
        crate::CancellationToken::never(),
        crate::solver_budget::SolverLimits {
            total: std::time::Duration::from_secs(5),
            per_check: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    );
    let app = |name: &str, args| NumericExpr::App { name: name.into(), args };
    let a = app("audit_f", vec![app("audit_g", vec![n("1")]), n("2")]);
    let b = app("audit_f", vec![app("audit_g", vec![n("1"), n("2")])]);
    let assumption = ConstraintExpr::Compare { lhs: a.clone(), op: CompareOp::Eq, rhs: n("0") };
    assert_eq!(
        z3_solver::try_solve(&assumption, &Subst::default(), &[assumption.clone()]),
        ConstraintStatus::Satisfied
    );
    let goal = ConstraintExpr::Compare { lhs: b, op: CompareOp::Eq, rhs: n("0") };
    assert_eq!(
        z3_solver::try_solve(&goal, &Subst::default(), &[assumption]),
        ConstraintStatus::Unknown
    );
}

#[test]
fn failed_pending_constraints_produce_diagnostics() {
    let source = "val audit : unit -> unit\nfunction audit() = ()";
    let (root, _) = syntax::parse_text(source);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&root);
    let body = bodies.entries().first().unwrap();
    let mut ctx = InferenceContext::new_for_body(
        source,
        body.body.clone(),
        body.source_map.clone(),
        TopLevelEnv::default(),
        HashSet::new(),
    );
    ctx.table.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Bool(false)));
    ctx.table.push_obligation(Obligation::WidthEquality(n("8"), n("16")));
    ctx.table.push_obligation(Obligation::WidthEquality(n("'n"), n("16")));
    let result = ctx.finish_query();
    assert_eq!(
        result
            .inference_diagnostics()
            .iter()
            .filter(|d| matches!(d, InferenceDiagnostic::ConstraintViolation { .. }))
            .count(),
        2
    );
    assert_eq!(result.unsolved_constraints.len(), 1);
}

#[test]
fn global_numeric_as_binders_keep_their_singleton_identity() {
    let (root, _) = syntax::parse_text("let (size as 'size) : range(32, 64) = 32");
    let env = TopLevelEnv::from_cst(&root).0;
    assert_eq!(env.values["size"].display_text(), "atom('size)");
    assert!(!env.global_constraints.is_empty());
}

#[test]
fn directional_check_rejects_nested_numeric_narrowing() {
    let mut table = InferenceTable::new();
    assert!(!table.unify(&ty("(range(0, 1), bool)"), &ty("(range(0, 10), bool)")));
    assert!(table.unify(&ty("(range(0, 10), bool)"), &ty("(range(0, 1), bool)")));
}
#[test]
fn directional_check_expands_nested_parameterized_aliases() {
    let (root, _) = syntax::parse_text("type combined('n : Int, 'nn : Int) = bits('nn + 'n)");
    let env = TopLevelEnv::from_cst(&root).0;
    let mut table = InferenceTable::new();
    table.set_alias_schemes(env.alias_schemes);
    assert!(!table.unify(&ty("combined(4, 8)"), &ty("bits(16)")));
    assert!(!table.unify(&ty("(combined(4, 8), bool)"), &ty("(bits(16), bool)")));
    assert!(table.unify(&ty("combined(4, 8)"), &ty("bits(12)")));
}
#[test]
fn directional_check_function_parameters_are_contravariant() {
    let mut table = InferenceTable::new();
    let narrow = Ty::function(vec![ty("range(0, 1)")], ty("range(0, 1)"));
    let wide = Ty::function(vec![ty("range(0, 10)")], ty("range(0, 1)"));
    assert!(table.unify(&narrow, &wide));
    assert!(!table.unify(&wide, &narrow));
}
#[test]
fn failed_directional_check_rolls_back_nested_binding() {
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let expected = Ty::tuple(vec![variable.clone(), ty("range(0, 1)")]);
    let actual = Ty::tuple(vec![Ty::named("bool"), ty("range(0, 10)")]);
    assert!(!table.unify(&expected, &actual));
    assert_eq!(table.resolve(&variable), variable);
    assert!(table.drain_obligations().is_empty());
}

fn scoped_audit_context() -> InferenceContext<'static> {
    let source = "val audit : unit -> unit\nfunction audit() = if true then () else ()";
    let (root, _) = syntax::parse_text(source);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&root);
    let body = bodies.entries().first().unwrap();
    InferenceContext::new_for_body(
        source,
        body.body.clone(),
        body.source_map.clone(),
        TopLevelEnv::default(),
        HashSet::new(),
    )
}

#[test]
fn scoped_obligations_use_local_assumptions_without_leaking_them() {
    let mut ctx = scoped_audit_context();
    let hir_def::Expr::If { then_branch, else_branch: Some(other), .. } =
        ctx.body.expr(ctx.body.root()).unwrap()
    else {
        panic!()
    };
    let (then_branch, other) = (*then_branch, *other);
    let predicate = parse_constraint_text("'n == 8");
    ctx.table.push_obligation(Obligation::NumericConstraint(predicate.clone()));
    ctx.table.scope_obligations_since(0, then_branch, vec![predicate.clone()]);
    let next = ctx.table.obligation_count();
    ctx.table.push_obligation(Obligation::NumericConstraint(predicate.clone()));
    ctx.table.scope_obligations_since(next, other, vec![parse_constraint_text("'n == 16")]);
    let next = ctx.table.obligation_count();
    ctx.table.push_obligation(Obligation::NumericConstraint(predicate));
    ctx.table.scope_obligations_since(next, ctx.body.root(), Vec::new());
    let result = ctx.finish_query();
    let violations: Vec<_> = result
        .inference_diagnostics()
        .iter()
        .filter_map(|d| match d {
            InferenceDiagnostic::ConstraintViolation { expr, .. } => Some(*expr),
            _ => None,
        })
        .collect();
    assert_eq!(violations, vec![other]);
    assert_eq!(result.unsolved_constraints.len(), 1);
    assert!(result.unsolved_constraints[0].assumptions.is_empty());
}

#[test]
fn deferred_width_uses_its_captured_live_binding() {
    let mut ctx = scoped_audit_context();
    let variable = ctx.table.new_type_var();
    let TyKind::Infer(id) = variable.kind() else { panic!() };
    ctx.table.register_placeholder("'_fv_audit", id.0);
    ctx.table.push_obligation(Obligation::WidthEquality(n("'_fv_audit"), n("8")));
    ctx.table.scope_obligations_since(0, ctx.body.root(), Vec::new());
    assert!(ctx.table.bind(id.0, Ty::param("8")));
    let result = ctx.finish_query();
    assert!(result.unsolved_constraints.is_empty());
    assert!(!result
        .inference_diagnostics()
        .iter()
        .any(|d| matches!(d, InferenceDiagnostic::ConstraintViolation { .. })));
}

#[test]
fn distinct_unresolved_witnesses_are_not_both_erased_to_one_symbol() {
    let mut ctx = scoped_audit_context();
    for name in ["'_fv_left", "'_fv_right"] {
        let variable = ctx.table.new_type_var();
        let TyKind::Infer(id) = variable.kind() else { panic!() };
        ctx.table.register_placeholder(name, id.0);
    }
    ctx.table.push_obligation(Obligation::WidthEquality(n("'_fv_left"), n("'_fv_right")));
    ctx.table.scope_obligations_since(0, ctx.body.root(), Vec::new());
    let result = ctx.finish_query();
    assert_eq!(result.unsolved_constraints.len(), 1);
    assert!(result.unsolved_constraints[0].substitution.types.entries.is_empty());
}

#[test]
fn numeric_subtyping_solves_only_registered_flexible_witnesses() {
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let TyKind::Infer(id) = variable.kind() else { panic!() };
    table.register_placeholder("'_fv_width", id.0);
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("bits(32)"), &ty("bits('_fv_width * 8)")),
        subtype::SubtypeResult::Ok
    ));
    assert_eq!(table.placeholder_value("'_fv_width").as_deref(), Some("4"));
    assert!(!matches!(
        subtype::is_subtype(&mut table, &ty("bits(33)"), &ty("bits('_fv_width * 8)")),
        subtype::SubtypeResult::Ok
    ));
    assert!(!matches!(
        subtype::is_subtype(&mut table, &ty("bits(32)"), &ty("bits('caller * 8)")),
        subtype::SubtypeResult::Ok
    ));
}

#[test]
fn common_nonzero_width_factors_preserve_symbolic_witnesses() {
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let TyKind::Infer(id) = variable.kind() else { panic!() };
    table.register_placeholder("'_fv_width", id.0);
    assert!(table
        .bind_numeric_equality(&n("'_fv_width * 8"), &n("8 * (if 'mode == 32 then 4 else 8)"),));
    assert_eq!(
        table.placeholder_value("'_fv_width").map(|s| n(&s)),
        Some(n("if 'mode == 32 then 4 else 8"))
    );
    let other = table.new_type_var();
    let TyKind::Infer(id) = other.kind() else { panic!() };
    table.register_placeholder("'_fv_other", id.0);
    assert!(!table.bind_numeric_equality(&n("'_fv_other * 0"), &n("'mode * 0")));
    assert!(table.placeholder_value("'_fv_other").is_none());
}

#[test]
fn polynomial_cancellation_treats_conditional_values_as_opaque_atoms() {
    let goal = parse_constraint_text("(if 'v == 32 then 22 else 44) - 1 - 'k + 1 + ('k - 1 + 1) == (if 'v == 32 then 22 else 44)");
    assert_eq!(
        solver::solve_without_smt(&goal, &Subst::default(), &[]),
        ConstraintStatus::Satisfied
    );
    assert_eq!(
        solver::solve_without_smt(
            &parse_constraint_text("(if 'v == 32 then 22 else 44) == 22"),
            &Subst::default(),
            &[],
        ),
        ConstraintStatus::Unknown
    );
}

#[test]
fn conditional_equality_cancels_unbounded_common_terms_before_splitting() {
    let goal = parse_constraint_text("(if 8 * 2 ^ (if 'v == 32 then 2 else 3) == 32 then 22 else 44) - 'k + 'k == (if 'v == 32 then 22 else 44)");
    assert_eq!(
        solver::solve_without_smt(&goal, &Subst::default(), &[]),
        ConstraintStatus::Satisfied
    );
}

#[test]
fn compatibility_reports_proof_pending_and_recovery_separately() {
    use super::unify::Compatibility;
    let mut table = InferenceTable::new();
    assert_eq!(table.check_compatibility(&Ty::error(), &Ty::error()), Compatibility::Recovery);
    assert_eq!(table.check_compatibility(&ty("int"), &ty("nat")), Compatibility::Proven);
    assert_eq!(
        table.check_compatibility(&ty("bits('n)"), &ty("bits('m)")),
        Compatibility::Deferred
    );
    assert_eq!(table.check_compatibility(&ty("nat"), &ty("int")), Compatibility::Recovery);
    assert_eq!(
        table.check_compatibility(&ty("range(0,1)"), &ty("range(0,10)")),
        Compatibility::Mismatch
    );
}

#[test]
fn unknown_numeric_bounds_are_distinct_from_infinite_bounds() {
    let mut table = InferenceTable::new();
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("range(0, _)"), &ty("nat")),
        subtype::SubtypeResult::Ok
    ));
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("range(0, _)"), &ty("range(0, 10)")),
        subtype::SubtypeResult::NeedsConstraint(_)
    ));
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("range(0, 10)"), &ty("range(0, _)")),
        subtype::SubtypeResult::NeedsConstraint(_)
    ));
    assert!(matches!(
        subtype::is_subtype(&mut table, &ty("int"), &ty("range(0, 10)")),
        subtype::SubtypeResult::Fail
    ));
}

#[test]
fn bit_and_single_bit_vector_compatibility_checks_width() {
    let mut table = InferenceTable::new();
    assert!(table.unify(&ty("bits(1)"), &ty("bit")));
    assert!(table.unify(&ty("bit"), &ty("vector(1, bit)")));
    assert!(!table.unify(&ty("bits(2)"), &ty("bit")));
    assert!(!table.unify(&ty("bit"), &ty("vector(2, bit)")));
}

#[test]
fn pending_compatibility_is_retained_without_warning_but_failure_is_reported() {
    let mut ctx = scoped_audit_context();
    ctx.table.push_subtype_obligation(Obligation::WidthEquality(n("'n"), n("16")));
    ctx.table.push_subtype_obligation(Obligation::WidthEquality(n("8"), n("16")));
    ctx.table.scope_obligations_since(0, ctx.body.root(), Vec::new());
    let result = ctx.finish_query();
    assert_eq!(result.unsolved_constraints.len(), 1);
    assert!(!result
        .inference_diagnostics()
        .iter()
        .any(|d| matches!(d, InferenceDiagnostic::UnsolvedConstraint { .. })));
    assert_eq!(
        result
            .inference_diagnostics()
            .iter()
            .filter(|d| matches!(d, InferenceDiagnostic::ConstraintViolation { .. }))
            .count(),
        1
    );
}

#[test]
fn bit_vector_keeps_element_inference_when_checked_as_a_generic_vector() {
    use super::unify::Compatibility;
    let mut table = InferenceTable::new();
    let element = table.new_type_var();
    let expected = Ty::app(
        "vector",
        vec![TyArg::numeric("32"), TyArg::Type(element.clone())],
        "vector(32, _)",
    );
    assert_eq!(table.check_compatibility(&expected, &ty("vector(32, bit)")), Compatibility::Proven);
    assert_eq!(table.resolve(&element), Ty::named("bit"));
}

#[test]
fn conditional_numeric_equality_is_not_an_unconditional_proof() {
    let e = NumericExpr::If {
        cond: Box::new(ConstraintExpr::Bool(true)),
        then_expr: Box::new(n("8")),
        else_expr: Box::new(n("16")),
    };
    assert!(!numeric::unify_numeric_expr(&e, &n("32"), &mut Subst::default()));
    assert!(numeric::unify_numeric_expr(&e, &n("8"), &mut Subst::default()));
}
#[test]
fn conditional_numeric_text_round_trips_nested_expressions() {
    for text in [
        "if true then 8 else 16",
        "2 * (if false then 8 else 16)",
        "if true then (if false then 1 else 8) else 16",
    ] {
        let expr = NumericExpr::parse(text).expect(text);
        assert_eq!(NumericExpr::parse(&expr.to_string_repr()), Some(expr));
    }
}
#[test]
fn unknown_conditional_does_not_bind_a_branch_as_unconditional() {
    let e = NumericExpr::If {
        cond: Box::new(ConstraintExpr::BoolVar("'p".into())),
        then_expr: Box::new(n("'n")),
        else_expr: Box::new(n("16")),
    };
    let mut subst = Subst::default();
    assert!(!numeric::unify_numeric_expr(&e, &n("8"), &mut subst));
    assert!(subst.values.entries.is_empty());
}

#[test]
fn existential_introduction_checks_the_chosen_witness() {
    let expected = ty("{'n, 'n > 0. bits('n)}");
    let mut table = InferenceTable::new();
    assert!(table.unify(&expected, &ty("bits(8)")));
    assert!(!table.unify(&expected, &ty("bits(0)")));
}
#[test]
fn existential_elimination_uses_assumptions_without_choosing_the_witness() {
    let mut ctx = scoped_audit_context();
    assert!(ctx.table.unify(&ty("bits(8)"), &ty("{'n, 'n == 8. bits('n)}")));
    assert!(ctx.table.unify(&ty("bits(8)"), &ty("{'n, 'n in {8, 16}. bits('n)}")));
    ctx.table.scope_obligations_since(0, ctx.body.root(), Vec::new());
    let result = ctx.finish_query();
    assert_eq!(
        result
            .inference_diagnostics()
            .iter()
            .filter(|d| matches!(d, InferenceDiagnostic::ConstraintViolation { .. }))
            .count(),
        1
    );
}
#[test]
fn existential_repacking_preserves_the_original_witness() {
    let mut table = InferenceTable::new();
    let from = ty("{'n, 'n in {8, 16}. bits('n)}");
    let to = ty("{'m, 'm > 0. bits('m)}");
    assert!(table.unify(&to, &from));
    let obligations = table.drain_obligations();
    for ob in obligations {
        assert_ne!(
            solver::solve(&ob.goal.constraint(), &ob.substitution, &ob.assumptions),
            ConstraintStatus::Failed
        );
    }
}
#[test]
fn existential_predicates_with_unresolved_witnesses_are_retained() {
    let mut table = InferenceTable::new();
    assert!(table.unify(&ty("{'n, 'n > 0. bool}"), &ty("bool")));
    let obligations = table.drain_obligations();
    assert!(!obligations.is_empty());
    assert!(obligations.iter().any(|o| o.goal.constraint().to_text().contains("witness")));
    assert!(obligations.iter().any(|o| !o.substitution.types.entries.is_empty()));
}
#[test]
fn rigid_witnesses_cannot_escape_into_older_variables_or_aliases() {
    let mut table = InferenceTable::new();
    let outer = table.new_type_var();
    let opened =
        table.open_existential(&["'n".into()], &ConstraintExpr::Bool(true), &ty("bits('n)"));
    let TyKind::Infer(crate::ty::InferTy(id)) = outer.kind() else { panic!() };
    assert!(!table.bind(*id, opened.inner_ty));
    assert_eq!(table.resolve(&outer), outer);
}
#[test]
fn universe_levels_follow_nested_flexible_bindings_and_rollback() {
    let mut table = InferenceTable::new();
    let outer = table.new_type_var();
    let opened =
        table.open_existential(&["'n".into()], &ConstraintExpr::Bool(true), &ty("bits('n)"));
    let younger = table.new_type_var();
    let TyKind::Infer(crate::ty::InferTy(outer_id)) = outer.kind() else { panic!() };
    let TyKind::Infer(crate::ty::InferTy(younger_id)) = younger.kind() else { panic!() };
    let snapshot = table.snapshot();
    assert!(table.bind(*outer_id, Ty::tuple(vec![younger.clone()])));
    assert!(!table.bind(*younger_id, opened.inner_ty.clone()));
    table.rollback_to(snapshot);
    assert!(table.bind(*younger_id, opened.inner_ty));
    assert_eq!(table.resolve(&outer), outer);
}
#[test]
fn failed_quantified_checks_restore_names_universes_and_obligations() {
    let mut table = InferenceTable::new();
    let actual = Ty::tuple(vec![ty("{'n, 'n > 0. bits('n)}"), ty("bool")]);
    let expected = Ty::tuple(vec![ty("bits(8)"), ty("int")]);
    assert!(!table.unify(&expected, &actual));
    assert_eq!(table.universe(), 0);
    assert!(table.drain_obligations().is_empty());
    let first =
        table.open_existential(&["'n".into()], &ConstraintExpr::Bool(true), &ty("bits('n)"));
    assert!(first.inner_ty.display_text().contains("sk1_"));
}
#[test]
fn mapping_direction_does_not_accept_numeric_narrowing() {
    let mut table = InferenceTable::new();
    assert!(matches!(
        mapping::resolve_mapping_call(
            &mut table,
            &ty("range(0, 1)"),
            &ty("string"),
            &ty("range(0, 10)")
        ),
        mapping::MappingCallResult::NoMatch
    ));
}
#[test]
fn semantic_equality_is_stricter_than_subtyping() {
    use super::unify::Compatibility;
    let mut table = InferenceTable::new();
    assert_eq!(
        table.check_type_equality(&ty("range(0, 1)"), &ty("range(0, 10)")),
        Compatibility::Mismatch
    );
    assert!(table.drain_obligations().is_empty());
}
#[test]
fn mapping_clause_sides_keep_their_own_types_in_both_directions() {
    let mut table = InferenceTable::new();
    assert!(matches!(
        mapping::check_bidir_consistency(
            &mut table,
            &ty("int"),
            &ty("string"),
            &ty("int"),
            &ty("string")
        ),
        mapping::MappingCheckResult::Ok
    ));
}

#[test]
fn existential_integer_witnesses_reject_fractional_solutions() {
    let mut table = InferenceTable::new();
    let expected = ty("{'q, 'q in {0, 1}. atom(2 * 'q)}");
    assert!(table.unify(&expected, &ty("atom(2)")));
    assert!(!table.unify(&expected, &ty("atom(3)")));
}
#[test]
fn unknown_conditional_width_retains_a_goal_and_rejects_impossible_values() {
    let mut table = InferenceTable::new();
    let condition = NumericExpr::If {
        cond: Box::new(ConstraintExpr::BoolVar("'p".into())),
        then_expr: Box::new(n("8")),
        else_expr: Box::new(n("16")),
    };
    let width = Ty::app("bits", vec![TyArg::Nexp(condition)], "");
    assert!(table.unify(&ty("bits(8)"), &width));
    assert!(table.drain_obligations().iter().any(|o| o.goal.constraint().to_text().contains("if")));
    assert!(!table.unify(&ty("bits(32)"), &width));
}

#[test]
fn repackaged_rigid_witness_is_not_a_free_variable_escape() {
    let mut table = InferenceTable::new();
    let outer = table.new_type_var();
    let opened =
        table.open_existential(&["'n".into()], &parse_constraint_text("'n > 0"), &ty("bits('n)"));
    let package = table.close_universe(&opened.inner_ty, 0);
    assert!(matches!(package.kind(), TyKind::Exist { .. }));
    let TyKind::Infer(crate::ty::InferTy(id)) = outer.kind() else { panic!() };
    assert!(table.bind(*id, package));
}

#[test]
fn generated_witness_names_do_not_capture_user_variables() {
    use super::unify::Compatibility;
    let mut table = InferenceTable::new();
    let actual = ty("{'n, 'n in {8, 16}. bits('n)}");
    assert_ne!(table.check_compatibility(&ty("bits('_sk1_n)"), &actual), Compatibility::Proven);
    let mut table = InferenceTable::new();
    let (inner, _) = table.instantiate_existential_avoiding(
        &["'n".into()],
        &ConstraintExpr::Bool(true),
        &ty("bits('n)"),
        &ty("bits('_witness0)"),
    );
    assert_ne!(inner, ty("bits('_witness0)"));
    let mut table = InferenceTable::new();
    table.reserve_source_names("let '_sk1_n = 8");
    let opened =
        table.open_existential(&["'n".into()], &ConstraintExpr::Bool(true), &ty("bits('n)"));
    assert!(!opened.skolem_map.values().any(|n| n == "'_sk1_n"));
}

fn obligation_context() -> InferenceContext<'static> {
    let source = "val audit : unit -> unit\nfunction audit() = ()";
    let (root, _) = syntax::parse_text(source);
    let bodies = hir_def::bodies::CallableBodies::from_cst(&root);
    let body = bodies.entries().first().unwrap();
    InferenceContext::new_for_body(
        source,
        body.body.clone(),
        body.source_map.clone(),
        TopLevelEnv::default(),
        HashSet::new(),
    )
}

#[test]
fn obligation_coverage_counts_only_committed_final_outcomes() {
    let mut ctx = obligation_context();
    let snapshot = ctx.table.snapshot();
    ctx.table.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Bool(false)));
    ctx.table.rollback_to(snapshot);
    for goal in
        [ConstraintExpr::Bool(true), ConstraintExpr::Bool(false), ConstraintExpr::Unsupported]
    {
        ctx.table.push_obligation(Obligation::NumericConstraint(goal));
    }
    let result = ctx.finish_query();
    assert_eq!(result.obligation_stats, ObligationStats { proved: 1, refuted: 1, unknown: 1 });
    assert_eq!(result.obligation_stats.total(), 3);
    assert_eq!(result.refuted_constraints.len(), 1);
    assert_eq!(
        result.unsolved_constraints[0].unknown_reason,
        Some(UnknownConstraintReason::UnsupportedGoal)
    );
    let mut aggregate = InferenceResult::default();
    aggregate.merge(result);
    assert_eq!(aggregate.obligation_stats.total(), 3);
    assert_eq!(aggregate.obligation_stats.unknown, aggregate.unsolved_constraints.len());
}

#[test]
fn recovery_inventory_keeps_the_unproved_type_relation() {
    let mut ctx = obligation_context();
    assert!(ctx.table.unify(&ty("range(0, 7)"), &Ty::named("int")));
    let result = ctx.finish_query();
    assert_eq!(result.obligation_stats, ObligationStats { proved: 0, refuted: 0, unknown: 1 });
    let ob = &result.unsolved_constraints[0];
    assert_eq!(ob.unknown_reason, Some(UnknownConstraintReason::Recovery));
    assert_eq!(ob.recovery_types, Some((Ty::named("int"), ty("range(0, 7)"))));
}

#[test]
#[cfg(feature = "z3-solver")]
fn obligation_budget_reason_is_local_to_the_attempt_and_retryable() {
    use crate::solver_budget::{SolverBudget, SolverLimits};
    let limited = SolverBudget::enter(
        CancellationToken::never(),
        SolverLimits { checks: 0, ..Default::default() },
    );
    let mut ctx = obligation_context();
    let goal = parse_constraint_text("'coverage_budget_unique * 'coverage_budget_unique >= 0");
    ctx.table.push_obligation(Obligation::NumericConstraint(goal.clone()));
    ctx.table.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Unsupported));
    ctx.table.push_obligation(Obligation::NumericConstraint(ConstraintExpr::Bool(true)));
    let result = ctx.finish_query();
    assert_eq!(result.obligation_stats.proved, 1);
    assert_eq!(
        result.unsolved_constraints[0].unknown_reason,
        Some(UnknownConstraintReason::BudgetExhausted)
    );
    assert_eq!(
        result.unsolved_constraints[1].unknown_reason,
        Some(UnknownConstraintReason::UnsupportedGoal)
    );
    drop(limited);
    let _retry = SolverBudget::enter(
        CancellationToken::never(),
        SolverLimits {
            total: std::time::Duration::from_secs(5),
            per_check: std::time::Duration::from_secs(1),
            ..Default::default()
        },
    );
    let mut ctx = obligation_context();
    ctx.table.push_obligation(Obligation::NumericConstraint(goal));
    assert_eq!(ctx.finish_query().obligation_stats.proved, 1);
}

#[test]
fn erased_numeric_values_neither_prove_equalities_nor_supply_counterexamples() {
    for text in ["int - int == 0", "int == int", "0 <= int + int"] {
        let goal = parse_constraint_text(text);
        assert_eq!(solver::solve(&goal, &Subst::default(), &[]), ConstraintStatus::Unknown);
        assert_eq!(
            solver::prove_quantified(&goal, &Subst::default(), &[], &["'rigid".into()]),
            ConstraintStatus::Unknown
        );
        let mut ctx = obligation_context();
        ctx.table.push_obligation(Obligation::NumericConstraint(goal));
        let result = ctx.finish_query();
        assert_eq!(result.obligation_stats, ObligationStats { proved: 0, refuted: 0, unknown: 1 });
        assert_eq!(
            result.unsolved_constraints[0].unknown_reason,
            Some(UnknownConstraintReason::ErasedNumeric)
        );
    }
}

fn infer_obligation_source(source: &str) -> InferenceResult {
    let (root, errors) = syntax::parse_text(source);
    assert!(errors.is_empty(), "{errors:?}");
    let bodies = hir_def::bodies::CallableBodies::from_cst(&root);
    let body = bodies.entries().iter().find(|b| b.name == "audit").unwrap();
    let (env, constants) = TopLevelEnv::from_cst(&root);
    let mut ctx = InferenceContext::new_for_body(
        source,
        body.body.clone(),
        body.source_map.clone(),
        env,
        constants,
    );
    ctx.infer_callable_body_hir("audit", body);
    ctx.finish_query()
}

#[test]
fn accepted_opaque_call_retains_its_instantiated_obligation() {
    let result = infer_obligation_source("val need : forall 'n, opaque('n). bits('n) -> unit\nval audit : bits(8) -> unit\nfunction audit(x) = need(x)");
    assert!(
        result.unsolved_constraints.iter().any(|ob| ob.goal.constraint().to_text() == "opaque(8)"),
        "{:?}",
        result.unsolved_constraints
    );
    assert!(result.unsolved_constraints.iter().all(|ob| ob.origin.is_some()));
}

#[test]
fn known_call_return_does_not_discard_an_unresolved_constraint() {
    let result = infer_obligation_source("val need : forall 'n 'm, 'n == 'm * 'm. bits('n) -> unit\nval audit : bits(16) -> unit\nfunction audit(x) = need(x)");
    assert!(
        result.unsolved_constraints.iter().any(|ob| ob
            .goal
            .constraint()
            .to_text()
            .contains("16 ==")),
        "{:?}",
        result.unsolved_constraints
    );
    assert!(result.type_of_expr.iter().any(|(_, ty)| *ty == Ty::named("unit")));
}

#[test]
fn unselected_overload_does_not_contribute_opaque_obligations() {
    let result = infer_obligation_source("val exact : bits(8) -> unit\nval other : forall 'n, opaque('n). bits('n) -> unit\noverload pick = {exact, other}\nval audit : bits(8) -> unit\nfunction audit(x) = pick(x)");
    assert!(
        !result.unsolved_constraints.iter().any(|ob| ob
            .goal
            .constraint()
            .to_text()
            .contains("opaque")),
        "{:?}",
        result.unsolved_constraints
    );
}

#[test]
fn upstream_oracle_constraint_pairs_agree_with_inference() {
    let pairs = [
        (
            include_str!("../../test_data/constraint_audit/positive/width.sail"),
            include_str!("../../test_data/constraint_audit/negative/width.sail"),
        ),
        (
            include_str!("../../test_data/constraint_audit/positive/range.sail"),
            include_str!("../../test_data/constraint_audit/negative/range.sail"),
        ),
        (
            include_str!("../../test_data/constraint_audit/positive/constraint.sail"),
            include_str!("../../test_data/constraint_audit/negative/constraint.sail"),
        ),
    ];
    for (positive, negative) in pairs {
        for (source, rejected) in [(positive, false), (negative, true)] {
            let result = infer_obligation_source(source);
            let error = !result.type_mismatches.is_empty()
                || result.inference_diagnostics().iter().any(|d| {
                    matches!(
                        d,
                        InferenceDiagnostic::ConstraintViolation { .. }
                            | InferenceDiagnostic::CallConstraintViolation {
                                proved_false: true,
                                ..
                            }
                    )
                });
            assert_eq!(error, rejected, "{source}\n{:?}", result.inference_diagnostics());
        }
    }
}

#[test]
fn erased_numeric_assumptions_are_not_evidence_of_unreachability() {
    let goal = ConstraintExpr::Bool(false);
    let facts = [parse_constraint_text("int < int")];
    assert_eq!(solver::solve(&goal, &Subst::default(), &facts), ConstraintStatus::Unknown);
    assert_eq!(
        solver::solve_without_smt(&goal, &Subst::default(), &facts),
        ConstraintStatus::Unknown
    );
    #[cfg(feature = "z3-solver")]
    assert_eq!(z3_solver::try_solve(&goal, &Subst::default(), &facts), ConstraintStatus::Unknown);
}

#[test]
fn numeric_fallback_substitutions_cannot_make_assumptions_contradictory() {
    let mut subst = Subst::default();
    subst.values.insert("'a".into(), "int".into());
    subst.values.insert("'b".into(), "int".into());
    let facts = [parse_constraint_text("'a < 'b")];
    assert_eq!(
        solver::solve(&ConstraintExpr::Bool(false), &subst, &facts),
        ConstraintStatus::Unknown
    );
    #[cfg(feature = "z3-solver")]
    assert_eq!(
        z3_solver::try_solve(&ConstraintExpr::Bool(false), &subst, &facts),
        ConstraintStatus::Unknown
    );
}

#[test]
fn deferred_obligations_expand_predicate_synonyms_before_solving() {
    let mut ctx = obligation_context();
    ctx.env
        .constraint_synonyms
        .insert("valid_width".into(), (vec!["'n".into()], parse_constraint_text("'n in {16, 32}")));
    for text in ["valid_width(16)", "valid_width(8)"] {
        ctx.table.push_obligation(Obligation::NumericConstraint(parse_constraint_text(text)));
    }
    let result = ctx.finish_query();
    assert_eq!(result.obligation_stats, ObligationStats { proved: 1, refuted: 1, unknown: 0 });
}

#[test]
fn reflexive_width_equality_does_not_need_opaque_assumptions() {
    let facts = [
        parse_constraint_text(r#"3 <= vlen_exp & vlen_exp <= 16"#),
        parse_constraint_text(r#"3 <= elen_exp & elen_exp <= 16 & elen_exp <= vlen_exp"#),
        parse_constraint_text(r#"xlen in {32, 64}"#),
        parse_constraint_text(r#"'_sk1__set in {8, 16, 32, 64}"#),
        parse_constraint_text(r#"-3 <= '_sk2_LMUL_pow & '_sk2_LMUL_pow <= 3"#),
        parse_constraint_text(r#"1 <= '_sk3_num_elem & '_sk3_num_elem <= vlen"#),
        parse_constraint_text(r#"true"#),
        parse_constraint_text(r#"<unsupported>"#),
        parse_constraint_text(
            r#"~(<unsupported> | ~(valid_reg_overlap(vs1, vd, LMUL_pow, LMUL_pow_widen)) | ~(valid_reg_overlap(vs2, vd, LMUL_pow, LMUL_pow_widen)))"#,
        ),
        parse_constraint_text(r#"'_sk1__set >= 16 & '_sk1__set * 2 <= 64"#),
        parse_constraint_text(r#"SEW >= 16 & SEW_widen <= 64"#),
        parse_constraint_text(r#"'_sk2_LMUL_pow + 1 <= 3"#),
        parse_constraint_text(r#"LMUL_pow_widen <= 3"#),
        parse_constraint_text(
            r#"0 <= '_sk14__loop_index & '_sk14__loop_index <= '_sk3_num_elem - 1"#,
        ),
        parse_constraint_text(r#"1 <= '_sk3_num_elem & '_sk3_num_elem <= 2 ^ vlen_exp"#),
    ];
    assert_eq!(
        solver::solve(
            &ConstraintExpr::And(vec![parse_constraint_text(
                r#"'_sk1__set * 2 == '_sk1__set * 2"#
            )]),
            &Subst::default(),
            &facts
        ),
        ConstraintStatus::Satisfied
    );
}

#[test]
#[cfg(feature = "z3-solver")]
fn opaque_assumptions_do_not_block_supported_entailment() {
    let bound = parse_constraint_text("'n in {8,16,32} & 'n <= 16");
    let facts = [ConstraintExpr::And(vec![bound, ConstraintExpr::Unsupported])];
    for (goal, status) in [
        ("'n * 2 in {16,32}", ConstraintStatus::Satisfied),
        ("'n * 2 == 64", ConstraintStatus::Failed),
        ("'n == 8", ConstraintStatus::Unknown),
    ] {
        assert_eq!(
            z3_solver::try_solve(&parse_constraint_text(goal), &Subst::default(), &facts),
            status
        );
    }
    let disjunction =
        [ConstraintExpr::Or(vec![parse_constraint_text("'n == 8"), ConstraintExpr::Unsupported])];
    assert_eq!(
        z3_solver::try_solve(&parse_constraint_text("'n == 8"), &Subst::default(), &disjunction),
        ConstraintStatus::Unknown
    );
}

#[test]
#[cfg(feature = "z3-solver")]
fn negated_disjunction_retains_each_supported_negative_fact() {
    let comparison = parse_constraint_text("'n > 5");
    for (inner, expected) in [
        (
            ConstraintExpr::Or(vec![comparison.clone(), ConstraintExpr::Unsupported]),
            ConstraintStatus::Satisfied,
        ),
        (
            ConstraintExpr::And(vec![comparison, ConstraintExpr::Unsupported]),
            ConstraintStatus::Unknown,
        ),
    ] {
        assert_eq!(
            z3_solver::try_solve(
                &parse_constraint_text("'n <= 5"),
                &Subst::default(),
                &[ConstraintExpr::Not(Box::new(inner))],
            ),
            expected
        );
    }
}

#[test]
#[cfg(feature = "z3-solver")]
fn focused_smt_context_keeps_transitive_facts_and_checks_unreachable_paths() {
    let facts = ["'n == 'k", "'k == 'm", "'m == 8", "'unrelated >= 0", "'other <= 100"]
        .map(parse_constraint_text);
    for (goal, expected) in
        [("'n % 2 == 0", ConstraintStatus::Satisfied), ("'n % 2 == 1", ConstraintStatus::Failed)]
    {
        assert_eq!(
            z3_solver::try_solve(&parse_constraint_text(goal), &Subst::default(), &facts),
            expected
        );
    }
    let unreachable = ["'n == 0", "'z < 0", "'z > 0", "'other > 0"].map(parse_constraint_text);
    assert_eq!(
        z3_solver::try_solve(&parse_constraint_text("'n == 1"), &Subst::default(), &unreachable),
        ConstraintStatus::Satisfied
    );
    let reachable = ["'n >= 0", "'z < 0", "'other > 0", "true"].map(parse_constraint_text);
    assert_eq!(
        z3_solver::try_solve(&parse_constraint_text("'n == 1"), &Subst::default(), &reachable),
        ConstraintStatus::Unknown
    );
}

#[test]
fn arithmetic_ordering_proves_only_with_sign_and_divisor_evidence() {
    let facts = [
        parse_constraint_text("'n in {8, 16, 32, 64}"),
        parse_constraint_text("1 <= 'k & 'k <= 3"),
        parse_constraint_text("'m >= 0"),
        parse_constraint_text("'q >= 1"),
    ];
    for goal in ["'n >= 'n / 2 ^ 'k", "'q * 'm >= 0", "'q * 'm >= 'm", "0 <= 4 * 'm + 'q"] {
        assert_eq!(
            solver::solve_without_smt(&parse_constraint_text(goal), &Subst::default(), &facts),
            ConstraintStatus::Satisfied,
            "{goal}"
        );
    }
    for (goal, fact) in [
        ("'n >= 'n / 'd", "'n >= 0 & 'd >= 0"),
        ("'n >= 'n / 2 ^ 'k", "'n >= 0 & 'k >= -1"),
        ("'q * 'm >= 'm", "'q >= 0 & 'm >= 0"),
        ("'q * 'm >= 0", "'q >= 1"),
        ("'q * 'm >= 0", "'q >= 0 | 'm >= 0"),
    ] {
        assert_ne!(
            solver::solve_without_smt(
                &parse_constraint_text(goal),
                &Subst::default(),
                &[parse_constraint_text(fact)]
            ),
            ConstraintStatus::Satisfied,
            "{goal}: {fact}"
        );
    }
}

#[test]
fn arithmetic_ordering_agrees_with_small_signed_integer_domains() {
    let domains = [(-2, -1), (-1, 1), (0, 0), (0, 2), (1, 2)];
    for (alo, ahi) in domains {
        for (blo, bhi) in domains {
            let facts = [
                parse_constraint_text(&format!("{alo} <= 'a & 'a <= {ahi}")),
                parse_constraint_text(&format!("{blo} <= 'b & 'b <= {bhi}")),
            ];
            for source in [
                "0 <= 'a * 'b",
                "'a <= 'a * 'b",
                "'a >= 'a / 'b",
                "0 <= 'a + 'b",
                "'a >= 'a / 2 ^ 'b",
            ] {
                let goal = parse_constraint_text(source);
                if solver::solve_without_smt(&goal, &Subst::default(), &facts)
                    != ConstraintStatus::Satisfied
                {
                    continue;
                }
                for a in alo..=ahi {
                    for b in blo..=bhi {
                        let mut subst = Subst::default();
                        subst.values.insert("'a".into(), a.to_string());
                        subst.values.insert("'b".into(), b.to_string());
                        assert_eq!(
                            integer::condition(&goal, &subst, &[]),
                            Some(true),
                            "{source}, a={a}, b={b}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn power_ordering_requires_natural_ordered_exponents() {
    for (goal, facts, expected) in [
        ("2 ^ 'a <= 2 ^ 'b", "0 <= 'a & 'a <= 'b", true),
        ("1 <= 2 ^ 'a", "0 <= 'a", true),
        ("2 ^ 'a <= 2 ^ ('a + 'b)", "0 <= 'a & 0 <= 'b", true),
        ("2 ^ 'a <= 2 ^ 'b", "'a <= 'b", false),
        ("2 ^ 'a <= 2 ^ 'b", "0 <= 'a", false),
        ("2 ^ 'a <= 2 ^ 'b", "0 <= 'a | 'a <= 'b", false),
    ] {
        let result = solver::solve_without_smt(
            &parse_constraint_text(goal),
            &Subst::default(),
            &[parse_constraint_text(facts)],
        );
        assert_eq!(
            result == ConstraintStatus::Satisfied,
            expected,
            "{goal} under {facts}: {result:?}"
        );
    }
    for alo in -2..=3 {
        for blo in -2..=3 {
            let facts = [
                parse_constraint_text(&format!("{alo} <= 'a & 'a <= 3")),
                parse_constraint_text(&format!("{blo} <= 'b & 'b <= 3")),
            ];
            let goal = parse_constraint_text("2 ^ 'a <= 2 ^ ('a + 'b)");
            if solver::solve_without_smt(&goal, &Subst::default(), &facts)
                != ConstraintStatus::Satisfied
            {
                continue;
            }
            for a in alo..=3 {
                for b in blo..=3 {
                    let mut subst = Subst::default();
                    subst.values.insert("'a".into(), a.to_string());
                    subst.values.insert("'b".into(), b.to_string());
                    assert_eq!(integer::condition(&goal, &subst, &[]), Some(true), "a={a}, b={b}");
                }
            }
        }
    }
}

#[test]
fn positive_integer_factor_is_bounded_by_its_product() {
    for (facts, expected) in [
        ("'width in {1, 2, 4, 8} & 'width == 'n * 'bytes & 'bytes > 0", true),
        ("'width in {1, 2, 4, 8} & 'bytes * 'n == 'width & 'bytes > 0", true),
        ("'width in {0, 1, 2, 4, 8} & 'width == 'n * 'bytes & 'bytes > 0", false),
        ("'width in {1, 2, 4, 8} & ('width == 'n * 'bytes | 'bytes > 0)", false),
    ] {
        let goal = parse_constraint_text("'bytes <= 4096");
        let result =
            solver::solve_without_smt(&goal, &Subst::default(), &[parse_constraint_text(facts)]);
        assert_eq!(result == ConstraintStatus::Satisfied, expected, "{facts}: {result:?}");
    }
    for bound in -2..=4 {
        for minimum in -2..=2 {
            let facts = [parse_constraint_text(&format!(
                "'product <= {bound} & 'product > 0 & 'product == 'a * 'b & 'a >= {minimum}"
            ))];
            let goal = parse_constraint_text(&format!("'a <= {bound}"));
            if solver::solve_without_smt(&goal, &Subst::default(), &facts)
                != ConstraintStatus::Satisfied
            {
                continue;
            }
            for a in minimum..=8 {
                for b in -8..=8 {
                    let product = a * b;
                    if product > 0 && product <= bound {
                        assert!(a <= bound, "a={a}, b={b}, bound={bound}");
                    }
                }
            }
        }
    }
}

#[test]
#[cfg(feature = "z3-solver")]
fn vector_length_power_bound_uses_linear_exponent_premises() {
    let facts = [
        parse_constraint_text("3 <= 'elen & 'elen <= 'vlen & 'vlen <= 16"),
        parse_constraint_text("-4 <= 'lmul & 'lmul <= 3"),
        parse_constraint_text("3 <= 'sew & 'sew <= 10"),
        ConstraintExpr::Not(Box::new(ConstraintExpr::Or(vec![
            parse_constraint_text("'sew > 'lmul + 'elen"),
            ConstraintExpr::Unsupported,
        ]))),
    ];
    let goal = parse_constraint_text(
        "1 <= 2 ^ ('lmul + 'vlen - 'sew) & 2 ^ ('lmul + 'vlen - 'sew) <= 2 ^ 'vlen",
    );
    assert_eq!(solver::solve(&goal, &Subst::default(), &facts), ConstraintStatus::Satisfied);
    assert_ne!(solver::solve(&goal, &Subst::default(), &facts[..3]), ConstraintStatus::Satisfied);
}

#[test]
#[cfg(feature = "z3-solver")]
fn vector_element_count_uses_product_quotient_premises() {
    let facts = [
        "-3 <= 'lmul & 'lmul <= 3",
        "3 <= 'vlen & 'vlen <= 16",
        "'sew in {8, 16, 32, 64}",
        "2 ^ 'vlen >= 'sew",
        "(0 >= 'lmul & 'z == 0 | 0 < 'lmul & 'z == 'lmul)",
    ]
    .map(parse_constraint_text);
    let goal = parse_constraint_text(
        "1 <= div(2 ^ 'z * 2 ^ 'vlen, 'sew) & div(2 ^ 'z * 2 ^ 'vlen, 'sew) <= 2 ^ 'vlen",
    );
    assert_eq!(solver::solve(&goal, &Subst::default(), &facts), ConstraintStatus::Satisfied);
    for facts in [
        "'a == 0 & 'b == 8 & 'd == 8",
        "'a == 9 & 'b == 8 & 'd == 8",
        "'a == 1 & 'b == 8 & 'd == -1",
    ] {
        let goal = parse_constraint_text("1 <= div('a * 'b, 'd) & div('a * 'b, 'd) <= 'b");
        assert_ne!(
            solver::solve(&goal, &Subst::default(), &[parse_constraint_text(facts)]),
            ConstraintStatus::Satisfied
        );
    }
}

#[test]
fn finite_domains_prove_arithmetic_without_smt_or_guessed_values() {
    for (goal, facts, expected) in [
        ("'n * 8 in {16,32}", "'n in {2,4}", true),
        ("div('n, 8) in {1,2,4,8}", "'n in {8,16,32,64}", true),
        ("'i <= 63", "'n in {0,16,64} & 'i <= 'n - 1", true),
        ("'n * 8 in {16,32}", "'n in {2,4,5}", false),
        ("'n * 8 in {16,32}", "'n in {2,4} | 'n == 5", false),
        ("'n * 8 in {16,32}", "~('n in {2,4})", false),
        ("'n * 8 in {16,32}", "'n in {'m,4}", false),
        ("'n >= 0", "~('n < 0)", true),
    ] {
        let result = solver::solve_without_smt(
            &parse_constraint_text(goal),
            &Subst::default(),
            &[parse_constraint_text(facts)],
        );
        assert_eq!(
            result == ConstraintStatus::Satisfied,
            expected,
            "{goal} under {facts}: {result:?}"
        );
    }
    // An incomplete search cannot certify a property of every member.
    let members = (0..65).map(|n| n.to_string()).collect::<Vec<_>>().join(",");
    let facts = parse_constraint_text(&format!("'n in {{{members}}}"));
    let allowed = (0..64).map(|n| (n * 2).to_string()).collect::<Vec<_>>().join(",");
    let goal = parse_constraint_text(&format!("'n * 2 in {{{allowed}}}"));
    assert_ne!(
        solver::solve_without_smt(&goal, &Subst::default(), &[facts]),
        ConstraintStatus::Satisfied
    );
}

#[test]
fn finite_arithmetic_entailment_agrees_with_exact_signed_evaluation() {
    for left in -3..=3 {
        for right in -3..=3 {
            for factor in -2..=2 {
                let facts = parse_constraint_text(&format!("'n in {{{left},{right}}}"));
                for bound in -3..=3 {
                    let goal = parse_constraint_text(&format!("'n * {factor} <= {bound}"));
                    if solver::solve_without_smt(&goal, &Subst::default(), &[facts.clone()])
                        == ConstraintStatus::Satisfied
                    {
                        assert!(left * factor <= bound && right * factor <= bound);
                    }
                }
            }
        }
    }
}

#[test]
fn negated_comparison_edges_preserve_complements_and_disjunctions() {
    for (operator, complement) in [("<", ">="), ("<=", ">"), (">", "<="), (">=", "<")] {
        let facts = parse_constraint_text(&format!("~('n {operator} 3)"));
        let goal = parse_constraint_text(&format!("'n {complement} 3"));
        assert_eq!(
            solver::solve_without_smt(&goal, &Subst::default(), &[facts]),
            ConstraintStatus::Satisfied
        );
        let facts = parse_constraint_text(&format!("~('n {operator} 3) | 'n == 3"));
        let strict = parse_constraint_text("'n < 3");
        assert_ne!(
            solver::solve_without_smt(&strict, &Subst::default(), &[facts]),
            ConstraintStatus::Satisfied
        );
    }
}

#[test]
fn bounded_vector_width_division_does_not_need_smt() {
    let goal = parse_constraint_text("'n >= 0 & 'width / 2 ^ 'shift in {8,16,32,64}");
    for (facts, expected) in [
        ("1 <= 'n & 'width in {8,16,32,64} & 1 <= 'shift & 'shift <= 3 & 'width / 2 ^ 'shift >= 8", true),
        ("1 <= 'n & 'width in {8,16,32,64} & 1 <= 'shift & 'shift <= 3", false),
        ("1 <= 'n & 'width in {8,16,32,64} & (1 <= 'shift | 'shift <= 3)", false),
        ("1 <= 'n & 'width in {8,16,32,64} & -1 <= 'shift & 'shift <= 3", false),
    ] {
        assert_eq!(solver::solve_without_smt(&goal, &Subst::default(), &[parse_constraint_text(facts)]) == ConstraintStatus::Satisfied, expected, "{facts}");
    }
}
