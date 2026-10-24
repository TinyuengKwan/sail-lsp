//! Boolean index identity, propagation, and counterexamples.
use super::*;
fn infer(source: &str) -> InferenceResult {
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
fn rejected(r: &InferenceResult) -> bool {
    !r.type_mismatches.is_empty()
        || r.inference_diagnostics().iter().any(|d| {
            matches!(
                d,
                InferenceDiagnostic::ConstraintViolation { .. }
                    | InferenceDiagnostic::CallConstraintViolation { proved_false: true, .. }
            )
        })
}
fn bool_ty(c: ConstraintExpr) -> Ty {
    Ty::app("bool", vec![TyArg::Bool(c)], "")
}
fn c(s: &str) -> ConstraintExpr {
    parse_constraint_text(s)
}

#[test]
fn boolean_predicate_rendering_preserves_disjunction_under_conjunction() {
    let original = c("'p & ('q | 'r)");
    assert_eq!(c(&original.to_text()), original);
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let TyKind::Infer(id) = variable.kind() else { panic!() };
    table.register_placeholder("'flex", id.0);
    assert!(table.unify(&bool_ty(c("'flex")), &bool_ty(original.clone())));
    assert_eq!(table.deep_resolve(&bool_ty(c("'flex"))), bool_ty(original));
}

#[test]
fn boolean_index_binding_is_consistent_and_rollback_safe() {
    let mut table = InferenceTable::new();
    let variable = table.new_type_var();
    let TyKind::Infer(id) = variable.kind() else { panic!() };
    table.register_placeholder("'flex", id.0);
    let target = bool_ty(c("'flex"));
    let snap = table.snapshot();
    assert!(table.unify(&target, &bool_ty(c("false"))));
    assert!(!table.unify(&target, &bool_ty(c("true"))));
    table.rollback_to(snap);
    assert!(table.unify(&target, &bool_ty(c("true"))));
    assert_eq!(table.deep_resolve(&target), bool_ty(c("true")));
}

#[test]
fn boolean_index_binding_follows_linked_call_variables_without_cycles() {
    let mut table = InferenceTable::new();
    let first = table.new_type_var();
    let second = table.new_type_var();
    for (name, variable) in [("'first", &first), ("'second", &second)] {
        let TyKind::Infer(id) = variable.kind() else { panic!() };
        table.register_placeholder(name, id.0);
    }
    assert!(table.unify(&first, &second));
    let snap = table.snapshot();
    table.bind_boolean_index(&c("'second"), &c("~('first)"));
    assert!(
        matches!(table.resolve(&first).kind(), TyKind::Infer(_)),
        "a linked alias created a Boolean cycle"
    );
    table.bind_boolean_index(&c("'second"), &c("true"));
    assert_eq!(table.deep_resolve(&bool_ty(c("'first"))), bool_ty(c("true")));
    table.rollback_to(snap);
    assert!(table.unify(&bool_ty(c("'second")), &bool_ty(c("false"))));
    assert_eq!(table.deep_resolve(&bool_ty(c("'first"))), bool_ty(c("false")));
}

#[test]
fn literal_boolean_call_result_keeps_the_chosen_value() {
    for (value, bad) in [("true", false), ("false", true)] {
        let r = infer(&format!("val identity : forall ('p : Bool). bool('p) -> bool('p)\nval audit : unit -> bool(true)\nfunction audit() = identity({value})"));
        assert_eq!(rejected(&r), bad, "{:?}", r.inference_diagnostics());
        if !bad {
            assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
        }
    }
}

#[test]
fn independent_boolean_parameters_are_not_unified() {
    let r = infer("val same : forall ('p : Bool). (bool('p), bool('p)) -> unit\nval audit : (bool, bool) -> unit\nfunction audit(p, q) = same(p, q)");
    assert!(
        rejected(&r) || !r.unsolved_constraints.is_empty(),
        "independent values were treated as equal"
    );
    let good = infer("val same : forall ('p : Bool). (bool('p), bool('p)) -> unit\nval audit : bool -> unit\nfunction audit(p) = same(p, p)");
    assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
}

#[test]
fn boolean_operators_and_conditional_results_keep_predicates() {
    for expression in ["p & q", "if p then q else false", "~(~(p & q))"] {
        let r = infer(&format!("val audit : forall ('p : Bool) ('q : Bool). (bool('p), bool('q)) -> bool('p & 'q)\nfunction audit(p, q) = {expression}"));
        assert!(!rejected(&r), "{expression}: {:?}", r.inference_diagnostics());
        assert!(r.unsolved_constraints.is_empty(), "{expression}: {:?}", r.unsolved_constraints);
    }
}

#[test]
fn mutable_boolean_annotations_allow_changes_without_stale_facts() {
    let r =
        infer("val audit : unit -> bool\nfunction audit() = { var b : bool = true; b = false; b }");
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    let bad = infer("val need : bool(true) -> unit\nval audit : unit -> unit\nfunction audit() = { var b : bool = true; b = false; need(b) }");
    assert!(
        rejected(&bad) || !bad.unsolved_constraints.is_empty(),
        "mutation retained stale true evidence"
    );
}

#[test]
fn boolean_pattern_facts_are_local_to_each_arm() {
    let good = infer("val need : bool(true) -> unit\nval audit : bool -> unit\nfunction audit(b) = match b { true => need(b), false => () }");
    assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
    let bad = infer("val need : bool(true) -> unit\nval audit : bool -> unit\nfunction audit(b) = { match b { true => need(b), false => () }; need(b) }");
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty(), "arm facts leaked after match");
}

#[test]
fn logical_rhs_requires_its_own_evidence() {
    for (operator, expected) in [("&", "true"), ("|", "false")] {
        let bad = infer(&format!("val need : bool({expected}) -> bool\nval audit : bool -> bool\nfunction audit(b) = b {operator} need(b)"));
        assert!(rejected(&bad), "RHS borrowed an unsupported assumption");
    }
}

#[test]
fn equality_guards_preserve_boolean_identity_through_annotations() {
    let r = infer("val need : bool(true) -> unit\nfunction audit(b : bool) -> unit = if b == true then need(b) else ()");
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn propositional_solver_bounds_work_and_does_not_drop_unknown_facts_for_counterexamples() {
    assert_eq!(
        boolean::decide(&c("'p | ~('p)"), &Subst::default(), &[], false),
        Some(ConstraintStatus::Satisfied)
    );
    assert_eq!(
        boolean::decide(&c("'p"), &Subst::default(), &[ConstraintExpr::Unsupported], true),
        Some(ConstraintStatus::Unknown)
    );
    let too_large =
        ConstraintExpr::And((0..9).map(|i| ConstraintExpr::BoolVar(format!("'p{i}"))).collect());
    assert_eq!(boolean::decide(&too_large, &Subst::default(), &[], false), None);
}

#[test]
fn type_substitutions_keep_boolean_literals_and_formula_structure() {
    for text in ["true", "false", "'q & ('r | 's)"] {
        let mut subst = Subst::default();
        subst.types.insert("'p".into(), Ty::param(text));
        assert_eq!(substitution::constraint(&c("'p"), &subst), c(text));
    }
}

#[test]
fn boolean_compiler_oracle_fixtures() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = infer(include_str!(concat!(
                "../../test_data/boolean_audit/positive/",
                $name,
                ".sail"
            )));
            assert!(!rejected(&good), "{}: {:?}", $name, good.inference_diagnostics());
            assert!(
                good.unsolved_constraints.is_empty(),
                "{}: {:?}",
                $name,
                good.unsolved_constraints
            );
            let bad = infer(include_str!(concat!(
                "../../test_data/boolean_audit/negative/",
                $name,
                ".sail"
            )));
            assert!(
                rejected(&bad) || !bad.unsolved_constraints.is_empty(),
                "{}: invalid fixture silently proved",
                $name
            );
        }};
    }
    pair!("identity");
    pair!("distinct");
    pair!("conditional");
    pair!("short_circuit");
    pair!("mutation");
    pair!("grouping");
    pair!("match_scope");
    pair!("negation");
}

#[test]
fn inferred_mutable_boolean_type_keeps_its_initializer_constraint() {
    let r = infer("val audit : unit -> bool\nfunction audit() = { var b = true; b = false; b }");
    assert!(rejected(&r), "changing a bool(true) binding must fail: {r:?}");
}

#[test]
fn refined_boolean_comparisons_still_reject_numbers() {
    let bad = infer("val audit : bool -> bool\nfunction audit(b) = b == 0");
    assert!(rejected(&bad));
}

#[test]
fn overloaded_boolean_return_preserves_the_selected_arguments() {
    for (expr, bad) in [("true & true", false), ("true & false", true)] {
        let r = infer(&format!("val bit_and : (bits(8), bits(8)) -> bits(8)\nval bool_and : forall ('p : Bool) ('q : Bool). (bool('p), bool('q)) -> bool('p & 'q)\noverload operator & = {{bit_and, bool_and}}\nval audit : unit -> bool(true)\nfunction audit() = {expr}"));
        assert_eq!(rejected(&r), bad, "{:?}", r.inference_diagnostics());
        if !bad {
            assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
        }
    }
}

#[test]
fn flexible_boolean_indices_cannot_capture_younger_values_or_themselves() {
    let mut table = InferenceTable::new();
    let outer = table.new_type_var();
    let TyKind::Infer(id) = outer.kind() else { panic!() };
    table.register_placeholder("'outer", id.0);
    let opened = table.open_existential(&["'p".into()], &c("true"), &bool_ty(c("'p")));
    table.bind_boolean_index(&c("'outer"), &subtype::bool_value(&opened.inner_ty).unwrap());
    assert_eq!(table.resolve(&outer), outer, "younger predicate escaped its universe");
    table.bind_boolean_index(&c("'outer"), &c("~('outer)"));
    assert_eq!(table.resolve(&outer), outer, "cyclic predicate was accepted");
}

#[test]
fn generic_constructor_boolean_payloads_keep_one_identity_per_value() {
    for (args, bad) in [("p, p", false), ("p, q", true)] {
        let r = infer(&format!("union Boxed('a : Type) = {{ Box : 'a }}\nval same : forall ('p : Bool). (bool('p), bool('p)) -> unit\nval audit : (Boxed(bool), bool) -> unit\nfunction audit(b, q) = match b {{ Box(p) => same({args}) }}"));
        if bad {
            assert!(rejected(&r) || !r.unsolved_constraints.is_empty());
        } else {
            assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
            assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
        }
    }
}
