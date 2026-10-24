//! Numeric evidence at checking boundaries, with compiler-oracle fixtures.
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

#[test]
fn numeric_compiler_oracle_fixtures() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = infer(include_str!(concat!(
                "../../test_data/numeric_audit/positive/",
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
                "../../test_data/numeric_audit/negative/",
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
    pair!("range_argument");
    pair!("symbolic_initializer");
    pair!("pattern_scope");
    pair!("large_literal");
    pair!("negative_literal");
    pair!("branch_join");
    pair!("mutation");
    pair!("dependent_result");
    pair!("loop_up");
    pair!("loop_down");
    pair!("immutable_literal");
    pair!("nested_pattern");
}

#[test]
fn loop_bounds_follow_direction_and_do_not_freeze_the_initial_value() {
    for header in ["0 to 7", "7 downto 0", "7 to 0 by 1 in dec", "0 to 7 by 2 in inc"] {
        let good = infer(&format!(
            "val audit_need : range(0, 7) -> unit\nval audit : unit -> unit\nfunction audit() = foreach (i from {header}) audit_need(i)"
        ));
        assert!(!rejected(&good), "{header}: {:?}", good.inference_diagnostics());
        assert!(good.unsolved_constraints.is_empty(), "{header}: {:?}", good.unsolved_constraints);
    }
    for header in ["0 to 8", "8 downto 0"] {
        let bad = infer(&format!(
            "val audit_need : range(0, 7) -> unit\nval audit : unit -> unit\nfunction audit() = foreach (i from {header}) audit_need(i)"
        ));
        assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
    }
    let bad = infer(
        "val audit_need : atom(0) -> unit\nval audit : unit -> unit\nfunction audit() = foreach (i from 0 to 3) audit_need(i)",
    );
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn unknown_or_negative_loop_steps_do_not_supply_monotonic_bounds() {
    for step in ["-1", "step"] {
        let bad = infer(&format!(
            "val audit_need : nat -> unit\nval audit : int -> unit\nfunction audit(step) = foreach (i from 0 to 7 by {step}) audit_need(i)"
        ));
        assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
    }
}

#[test]
fn numeric_pattern_facts_do_not_leak_out_of_nested_patterns() {
    let good = infer(
        "val audit_need : atom(0) -> unit\nval audit : forall 'n. (int('n), bool) -> unit\nfunction audit(n, b) = match (n, b) { (0, _) => audit_need(n), _ => () }",
    );
    assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
    let bad = infer(
        "val audit_need : atom(0) -> unit\nval audit : forall 'n. (int('n), bool) -> unit\nfunction audit(n, b) = { match (n, b) { (0, _) => audit_need(n), _ => () }; audit_need(n) }",
    );
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn an_impossible_range_pattern_does_not_require_global_membership() {
    let r =
        infer("val audit : range(0, 3) -> unit\nfunction audit(n) = match n { 4 => (), _ => () }");
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn immutable_numeric_literals_survive_aliases_and_arithmetic() {
    for binding in ["let x = 3", "let x = 1 + 2"] {
        let good = infer(&format!(
            "val audit_need : atom(3) -> unit\nval audit : unit -> unit\nfunction audit() = {{ {binding}; let y = x; audit_need(y) }}"
        ));
        assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
        assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
        let bad = infer(&format!(
            "val audit_need : atom(4) -> unit\nval audit : unit -> unit\nfunction audit() = {{ {binding}; let y = x; audit_need(y) }}"
        ));
        assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
    }
}

#[test]
fn coarse_numeric_annotations_do_not_retain_literal_values() {
    let bad = infer(
        "val audit_need : atom(3) -> unit\nval audit : unit -> unit\nfunction audit() = { let x : int = 3; audit_need(x) }",
    );
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn partial_loop_bounds_prove_nonnegativity_without_inventing_counterexamples() {
    let r = infer(
        "val audit_need : nat -> unit\nval audit : (nat, nat) -> unit\nfunction audit(lo, hi) = foreach (i from lo to hi) audit_need(i)",
    );
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn omitted_stride_cannot_manufacture_a_loop_counterexample() {
    let r = infer(
        "val audit_need : forall 'n, 'n % 2 == 0. int('n) -> unit\nval audit : unit -> unit\nfunction audit() = foreach (i from 0 to 6 by 2) audit_need(i)",
    );
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
}

#[test]
fn overloaded_arithmetic_preserves_literal_operand_values() {
    let r = infer(
        "val audit_sub : forall 'n 'm. (int('n), int('m)) -> int('n - 'm)\noverload operator - = { audit_sub }\nval audit : forall 'n. int('n) -> int('n - 1)\nfunction audit(n) = n - 1",
    );
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn vector_length_witness_survives_generic_result_payload() {
    let r = infer(
        "union audit_result('a : Type) = { AuditOk : 'a, AuditErr : unit }\nval audit_make : forall 'n, 'n >= 0. int('n) -> audit_result((vector('n, bits(8)), bits('n)))\nval audit : range(1, 16) -> unit\nfunction audit(count) = { let 'n = count; let (v, mask) : (vector('n, bits(8)), bits('n)) = match audit_make(count) { AuditOk(v) => v, AuditErr(()) => exit() }; () }",
    );
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn range_call_result_keeps_one_length_across_repeated_calls() {
    let r = infer(
        "val audit_count : unit -> range(1, 16)\nval audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)\nval audit : unit -> unit\nfunction audit() = { let count = audit_count(); let 'n = count; let a : bits('n) = audit_make(count); let b : bits('n) = audit_make(count); () }",
    );
    assert!(!rejected(&r), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn numeric_witness_compiler_oracle_fixtures() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = infer(include_str!(concat!(
                "../../test_data/witness_audit/positive/",
                $name,
                ".sail"
            )));
            assert!(
                !rejected(&good),
                "{}: {:?} {:?}",
                $name,
                good.inference_diagnostics(),
                good.type_mismatches
            );
            assert!(
                good.unsolved_constraints.is_empty(),
                "{}: {:?}",
                $name,
                good.unsolved_constraints
            );
            let bad = infer(include_str!(concat!(
                "../../test_data/witness_audit/negative/",
                $name,
                ".sail"
            )));
            assert!(
                rejected(&bad) || !bad.unsolved_constraints.is_empty(),
                "{}: invalid witness silently proved",
                $name
            );
        }};
    }
    pair!("generic_payload");
    pair!("call_identity");
    pair!("distinct_ranges");
    pair!("value_shadow");
    pair!("witness_shadow");
    pair!("symbolic_range");
    pair!("natural_length");
    pair!("nominal_overload");
    pair!("clause_guard");
    pair!("mutation");
}

#[test]
fn inline_range_parameter_has_the_same_witness_as_its_type_binding() {
    let r = infer(
        "union audit_result('a : Type) = { AuditOk : 'a, AuditErr : unit }\nval audit_make : forall 'n, 'n >= 0. int('n) -> audit_result((vector('n, bits(8)), bits('n)))\nfunction audit(count : range(1, 16)) -> unit = { let 'n = count; let (v, mask) : (vector('n, bits(8)), bits('n)) = match audit_make(count) { AuditOk(v) => v, AuditErr(()) => exit() }; () }",
    );
    assert!(!rejected(&r), "{:?} {:?}", r.inference_diagnostics(), r.type_mismatches);
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn numeric_fallback_arithmetic_does_not_return_the_operand_identity() {
    let good = infer("val audit : range(0, 3) -> range(3, 6)\nfunction audit(n) = n + 3");
    assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
    let bad = infer("val audit : range(0, 3) -> range(0, 3)\nfunction audit(n) = n + 3");
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn symbolic_range_bounds_preserve_length_witnesses() {
    let r = infer(
        "type audit_limit : Int\nconstraint audit_limit >= 1\nval audit_make : forall 'n, 'n >= 0. int('n) -> bits('n)\nfunction audit(count : range(1, audit_limit)) -> unit = { let 'n = count; let a : bits('n) = audit_make(count); () }",
    );
    assert!(!rejected(&r), "{:?} {:?}", r.inference_diagnostics(), r.type_mismatches);
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}

#[test]
fn a_value_binder_does_not_capture_a_range_bound_with_the_same_name() {
    let good = infer(
        "val audit : forall 'n, 'n >= 0. range('n, 'n + 1) -> range('n, 'n + 1)\nfunction audit(n) = n",
    );
    assert!(!rejected(&good), "{:?}", good.inference_diagnostics());
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
    let bad = infer(
        "val audit : forall 'n, 'n >= 0. range('n, 'n + 1) -> int('n)\nfunction audit(n) = n",
    );
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn nominal_overloads_preserve_the_selected_payload_width() {
    let prelude = "newtype audit_a = AuditA : bits(34)\nnewtype audit_b = AuditB : bits(32)\ntype audit_alias = audit_b\nval audit_a_bits : audit_a -> bits(34)\nval audit_b_bits : audit_b -> bits(32)\noverload audit_bits = { audit_a_bits, audit_b_bits }\n";
    let good = infer(&format!(
        "{prelude}val audit : (nat, audit_alias) -> bits(32)\nfunction audit(step, addr) = audit_bits(addr)"
    ));
    assert!(!rejected(&good), "{:?} {:?}", good.inference_diagnostics(), good.type_mismatches);
    assert!(good.unsolved_constraints.is_empty(), "{:?}", good.unsolved_constraints);
    let bad = infer(&format!(
        "{prelude}val audit : (nat, audit_alias) -> bits(34)\nfunction audit(step, addr) = audit_bits(addr)"
    ));
    assert!(rejected(&bad) || !bad.unsolved_constraints.is_empty());
}

#[test]
fn bounded_arithmetic_compiler_oracle_fixtures() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = infer(include_str!(concat!(
                "../../test_data/arithmetic_audit/positive/",
                $name,
                ".sail"
            )));
            assert!(
                !rejected(&good),
                "{}: {:?} {:?}",
                $name,
                good.inference_diagnostics(),
                good.type_mismatches
            );
            assert!(
                good.unsolved_constraints.is_empty(),
                "{}: {:?}",
                $name,
                good.unsolved_constraints
            );
            let bad = infer(include_str!(concat!(
                "../../test_data/arithmetic_audit/negative/",
                $name,
                ".sail"
            )));
            assert!(
                rejected(&bad) || !bad.unsolved_constraints.is_empty(),
                "{}: invalid arithmetic silently proved",
                $name
            );
        }};
    }
    pair!("symbolic_scale");
    pair!("negative_scale");
    pair!("symbolic_add");
    pair!("mixed_sign");
    pair!("natural_calls");
    pair!("wide_integer");
    pair!("exponent_range");
    pair!("exponent_sparse");
    pair!("exponent_parameter");
}

#[test]
fn bounded_exponents_keep_call_and_mutation_identities_separate() {
    for source in [
        "val audit_source : unit -> {3, 5}\nval audit : unit -> atom(0)\nfunction audit() = 2 ^ audit_source() - 2 ^ audit_source()",
        "val audit : unit -> atom(8)\nfunction audit() = { var n : range(3, 5) = 3; n = 5; 2 ^ n }",
        "val audit : range(3, 5) -> {8, 32}\nfunction audit(n) = 2 ^ n",
    ] {
        let bad = infer(source);
        assert!(
            rejected(&bad) || !bad.unsolved_constraints.is_empty(),
            "{source}: invalid power silently proved"
        );
    }
    let good = infer("val audit : (bits(8), bits(8)) -> bits(8)\nfunction audit(a, b) = a ^ b");
    assert!(!rejected(&good));
    assert!(good.unsolved_constraints.is_empty());
}
