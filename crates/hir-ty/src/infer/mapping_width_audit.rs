//! Mapping refinements and conditional bit widths at checking boundaries.
use super::*;
fn infer(source: &str, mapping: bool) -> InferenceResult {
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
    if mapping {
        ctx.infer_mapping_body_hir("audit", body);
    } else {
        ctx.infer_callable_body_hir("audit", body);
    }
    ctx.finish_query()
}
fn clean(r: &InferenceResult) {
    assert!(r.type_mismatches.is_empty(), "{:?}", r.type_mismatches);
    assert!(r.inference_diagnostics().is_empty(), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
}
#[test]
fn mapping_constructor_keeps_sparse_numeric_payload() {
    clean(&infer("default Order dec\nunion instruction = { Move : {1, 2, 4, 8} }\nmapping nreg : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }\nval audit : instruction <-> bits(6)\nmapping clause audit = Move(n) <-> 0b1 @ nreg(n)", true));
}
#[test]
fn conditional_width_call_keeps_the_complete_numeric_index() {
    clean(&infer("val consume : forall 'n, 'n in {32, 64}. bits('n) -> unit\nval audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> unit\nfunction audit(pte) = consume(pte)", false));
}
#[test]
fn conditional_width_alias_checks_correlated_branches() {
    clean(&infer("type pte_bits('v) = bits(if 'v == 32 then 32 else 64)\nval audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> pte_bits('v)\nfunction audit(pte) = pte", false));
}

#[salsa::db]
#[derive(Default, Clone)]
struct TestDb {
    storage: salsa::Storage<Self>,
}
#[salsa::db]
impl salsa::Database for TestDb {}
fn workspace(sources: &[&str], owner: &str) -> std::sync::Arc<InferenceResult> {
    use std::sync::Arc;
    let mut db = TestDb::default();
    let files: Vec<_> = sources
        .iter()
        .enumerate()
        .map(|(i, s)| {
            base_db::FileText::new(&db, Arc::from(*s), base_db::FileId::from_raw(i as u32))
        })
        .collect();
    hir_def::def_query::set_workspace_files(&mut db, files.clone());
    for file in files {
        for &id in hir_def::def_query::file_def_with_body_ids(&db, file) {
            if id.name(&db).as_str() == owner {
                return crate::query::infer(&db, id).0.clone();
            }
        }
    }
    panic!("missing {owner}")
}
#[test]
fn scattered_mapping_sparse_payload_has_no_recovery() {
    clean(&workspace(&[
        "default Order dec\nscattered union instruction\nval audit : instruction <-> bits(6)",
        "union clause instruction = Move : (bits(1), {1, 2, 4, 8})\nmapping nreg : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }\nmapping clause audit = Move(b, n) <-> b @ nreg(n)"
    ], "audit"));
}
#[test]
fn conditional_width_cross_file_call_has_no_recovery() {
    clean(&workspace(&[
        "val consume : forall 'n, 'n in {32, 64}. bits('n) -> unit",
        "val audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> unit\nfunction audit(pte) = consume(pte)"
    ], "audit"));
}
#[test]
fn conditional_width_cross_file_alias_has_no_recovery() {
    clean(&workspace(&[
        "type pte_bits('v) = bits(if 'v == 32 then 32 else 64)",
        "val audit : forall 'v, 'v in {32, 39, 48, 57}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> pte_bits('v)\nfunction audit(pte) = pte"
    ], "audit"));
}
#[test]
fn inferred_conditional_width_survives_calls_and_return_alias() {
    clean(&workspace(&[
        "type pte_bits('v) = bits(if 'v == 32 then 32 else 64)\nval consume : forall 'n, 'n in {32, 64}. bits('n) -> unit\nval load : forall 'n, 'n in {4, 8}. int('n) -> bits(8 * 'n)",
        "val audit : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> pte_bits('v)\nfunction audit(v) = { let 'log_bytes = if 'v == 32 then 2 else 3; let pte = load(2 ^ log_bytes); consume(pte); pte }"
    ], "audit"));
}
#[test]
fn conditional_width_from_generic_constructor_payload() {
    clean(&workspace(&[
        "union result('a : Type, 'e : Type) = { Ok : 'a, Err : 'e }\ntype pte_bits('v) = bits(if 'v == 32 then 32 else 64)\nval consume : forall 'n, 'n in {32, 64}. bits('n) -> unit\nval load : forall 'n, 'n in {4, 8}. int('n) -> result(bits(8 * 'n), unit)",
        "val audit : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> unit\nfunction audit(v) = { let 'log_bytes = if 'v == 32 then 2 else 3; match load(2 ^ log_bytes) { Err(_) => (), Ok(pte) => consume(pte) } }"
    ], "audit"));
}
#[test]
fn conditional_width_through_result_alias_keeps_type_kind() {
    clean(&workspace(&[
        "union result('a : Type, 'e : Type) = { Ok : 'a, Err : 'e }\ntype MemoryOpResult('a : Type) = result('a, unit)\ntype pte_bits('v) = bits(if 'v == 32 then 32 else 64)\nval consume : forall 'n, 'n in {32, 64}. bits('n) -> unit\nprivate function load forall 'n, 'n in {4, 8}. (size : int('n)) -> MemoryOpResult(bits(8 * 'n)) = undefined",
        "val audit : forall 'v, 'v in {32, 39, 48, 57}. int('v) -> unit\nfunction audit(v) = { let 'log_bytes = if 'v == 32 then 2 else 3; match load(2 ^ log_bytes) { Err(_) => (), Ok(pte) => consume(pte) } }"
    ], "audit"));
}

fn unproved(r: &InferenceResult) -> bool {
    !r.type_mismatches.is_empty()
        || !r.unsolved_constraints.is_empty()
        || r.inference_diagnostics().iter().any(|d| {
            matches!(
                d,
                InferenceDiagnostic::ConstraintViolation { .. }
                    | InferenceDiagnostic::CallConstraintViolation { proved_false: true, .. }
                    | InferenceDiagnostic::NoOverloading { .. }
            )
        })
}
#[test]
fn sparse_mapping_calls_accept_members_and_reject_gaps() {
    for n in [1, 2, 3, 4, 5, 8] {
        let r = workspace(&[
            "mapping encode : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }",
            &format!("val audit : unit -> bits(5)\nfunction audit() = encode({n})")
        ], "audit");
        if [1, 2, 4, 8].contains(&n) {
            clean(&r);
        } else {
            assert!(unproved(&r), "gap {n} silently proved");
        }
    }
}
#[test]
fn constructor_match_keeps_each_arms_constraints_local() {
    let r = workspace(&[
        "scattered union instruction\nval audit : instruction -> bits(5)",
        "union clause instruction = Narrow : {1, 2, 4, 8}\nunion clause instruction = Wide : range(1, 8)\nmapping encode : bits(5) <-> {1, 2, 4, 8} = { 0b00000 <-> 1, 0b00001 <-> 2, 0b00011 <-> 4, 0b00111 <-> 8 }\nfunction audit(x) = match x { Narrow(n) => encode(n), Wide(n) => encode(n) }"
    ], "audit");
    assert!(unproved(&r), "first arm's membership leaked into second arm");
}
#[test]
fn conditional_widths_reject_swapped_or_independent_branches() {
    for expected in
        ["bits(if 'v == 32 then 64 else 32)", "bits(if 'w == 32 then 32 else 64)", "bits(32)"]
    {
        let r = infer(&format!("val audit : forall 'v 'w, 'v in {{32, 39}} & 'w in {{32, 39}}. bits(8 * 2 ^ (if 'v == 32 then 2 else 3)) -> {expected}\nfunction audit(pte) = pte"), false);
        assert!(unproved(&r), "{expected}: unrelated branches silently proved");
    }
}
#[test]
fn conditional_proof_preserves_correlations_and_has_a_budget() {
    let solve = |text| {
        super::integer::conditional(&super::parse_constraint_text(text), &Subst::default(), &[])
    };
    assert_eq!(
        solve("8 * 2 ^ (if 'v == 32 then 2 else 3) == (if 'v == 32 then 32 else 64)"),
        Some(true)
    );
    assert_eq!(
        solve("(if 'v == 32 then 32 else 64) == (if 'v == 32 then 64 else 32)"),
        Some(false)
    );
    assert_eq!(solve("(if 'v == 32 then 32 else 64) == (if 'w == 32 then 32 else 64)"), None);
    assert_eq!(solve("(if 'v == 32 then 32 else 64) == 32"), None);
    assert_eq!(solve("2 ^ (if 'v == 32 then -1 else 3) == 8"), None);
    let sum =
        (0..5).map(|i| format!("(if 'v{i} == 0 then 0 else 1)")).collect::<Vec<_>>().join(" + ");
    assert_eq!(solve(&format!("{sum} == {sum}")), None, "conditional search exceeded four choices");
}

#[test]
fn mapping_and_width_compiler_oracle_fixtures() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = workspace(
                &[include_str!(concat!(
                    "../../test_data/mapping_width_audit/positive/",
                    $name,
                    ".sail"
                ))],
                "audit",
            );
            clean(&good);
            let bad = workspace(
                &[include_str!(concat!(
                    "../../test_data/mapping_width_audit/negative/",
                    $name,
                    ".sail"
                ))],
                "audit",
            );
            assert!(unproved(&bad), "{}: invalid fixture silently proved", $name);
        }};
    }
    pair!("mapping_sparse");
    pair!("mapping_string");
    pair!("mapping_backwards");
    pair!("mapping_gap");
    pair!("conditional_alias");
    pair!("conditional_call");
    pair!("result_payload");
    pair!("type_alias_kind");
}
