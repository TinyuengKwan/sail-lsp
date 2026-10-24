//! Exception identity and dependent branch equality regressions.
use super::*;
fn clean(r: &InferenceResult) {
    assert!(
        r.expression_types().all(|(_, ty)| !ty.is_error() && !ty.references_error()),
        "unknown expression type"
    );
    assert!(r.type_mismatches.is_empty(), "{:?}", r.type_mismatches);
    assert!(r.inference_diagnostics().is_empty(), "{:?}", r.inference_diagnostics());
    assert!(r.unsolved_constraints.is_empty(), "{:?}", r.unsolved_constraints);
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
fn catch_patterns_have_exception_type() {
    clean(&workspace(&[
        "union exception = { ErrorA : string, ErrorB : string, ErrorC : string }",
        "val audit : unit -> string\nfunction audit() = try { \"\" } catch { ErrorA(s) => s, ErrorB(s) => s, ErrorC(s) => s }",
    ], "audit"));
}
#[test]
fn imported_constructor_uses_payload_identity() {
    clean(&workspace(&[
        "union exception = { Exception : unit }",
        "enum ExceptionType = { Fault }\nunion TrapCause = { Exception : ExceptionType }",
        "val consume : TrapCause -> unit\nval audit : ExceptionType -> unit\nfunction audit(e) = consume(Exception(e))",
    ], "audit"));
}
#[test]
fn equal_branch_widths_use_numeric_evidence() {
    clean(&workspace(&[
        "val audit : forall 'n 'm, 'n >= 'm & 'm >= 0. (implicit('m), bits('n), bits('m)) -> bits('m)\nfunction audit(m, x, fallback) = if 'n == 'm then x else fallback",
    ], "audit"));
}

fn unproved(r: &InferenceResult) -> bool {
    !r.type_mismatches.is_empty()
        || !r.unsolved_constraints.is_empty()
        || r.inference_diagnostics().iter().any(|d| {
            matches!(
                d,
                InferenceDiagnostic::InvalidTypeUse { .. }
                    | InferenceDiagnostic::ConstraintViolation { .. }
                    | InferenceDiagnostic::CallConstraintViolation { proved_false: true, .. }
                    | InferenceDiagnostic::NoOverloading { .. }
            )
        })
}
#[test]
fn exception_and_branch_compiler_oracles() {
    macro_rules! pair {
        ($name:literal) => {{
            clean(&workspace(
                &[include_str!(concat!(
                    "../../test_data/exception_branch_audit/positive/",
                    $name,
                    ".sail"
                ))],
                "audit",
            ));
            let bad = workspace(
                &[include_str!(concat!(
                    "../../test_data/exception_branch_audit/negative/",
                    $name,
                    ".sail"
                ))],
                "audit",
            );
            assert!(unproved(&bad), "{}: invalid fixture silently proved", $name);
        }};
    }
    pair!("catch_payload");
    pair!("catch_union");
    pair!("catch_guard");
    pair!("branch_equality");
    pair!("branch_scope");
    pair!("constructor_payload");
    pair!("generic_type_kind");
}

#[test]
fn imported_constructor_resolution_is_independent_of_file_order() {
    let standard = "union exception = { Exception : unit }";
    let model = "enum ExceptionType = { Fault }\nunion TrapCause = { Exception : ExceptionType }";
    for defs in [[standard, model], [model, standard]] {
        for (argument, result) in [("()", "exception"), ("Fault", "TrapCause")] {
            clean(&workspace(
                &[
                    defs[0],
                    defs[1],
                    &format!(
                        "val audit : unit -> {result}\nfunction audit() = Exception({argument})"
                    ),
                ],
                "audit",
            ));
        }
    }
}
#[test]
fn catch_constructor_selection_uses_exception_identity() {
    clean(&workspace(&[
        "union Other = { Shared : int }",
        "union exception = { Shared : string }",
        "val audit : unit -> string\nfunction audit() = try { \"ok\" } catch { Shared(s) => s }",
    ], "audit"));
}
#[test]
fn catch_branch_checks_dependent_width_under_its_guard() {
    clean(&workspace(&[
        "union exception = { Failure : unit }",
        "val audit : forall 'n 'm, 'n >= 'm & 'm >= 0. (implicit('m), bits('n), bits('m)) -> bits('m)\nfunction audit(m, x, fallback) = try { fallback } catch { Failure() if 'n == 'm => x, _ => fallback }",
    ], "audit"));
}

#[test]
fn colliding_generic_constructor_keeps_its_payload_instantiation() {
    clean(&workspace(
        &[
            "union Old = { Shared : unit }",
            "union Wrapped('a : Type) = { Shared : 'a }",
            "val audit : bits(8) -> Wrapped(bits(8))\nfunction audit(x) = Shared(x)",
        ],
        "audit",
    ));
}
#[test]
fn catch_guard_equality_does_not_escape_to_the_next_arm() {
    let r = workspace(&[
        "union exception = { Failure : unit }",
        "val audit : forall 'n 'm, 'n >= 'm & 'm >= 0. (implicit('m), bits('n), bits('m)) -> bits('m)\nfunction audit(m, x, fallback) = try { fallback } catch { Failure() if 'n == 'm => x, _ => x }",
    ], "audit");
    assert!(unproved(&r), "guard equality escaped to an unguarded catch arm");
}
#[test]
fn constructor_candidate_edit_invalidates_the_dependent_body() {
    use salsa::Setter;
    use std::sync::Arc;
    let mut db = TestDb::default();
    let lib = base_db::FileText::new(
        &db,
        Arc::from("union exception = { Exception : unit }"),
        base_db::FileId::from_raw(0),
    );
    let model = base_db::FileText::new(
        &db,
        Arc::from("enum Fault = { Failed }\nunion TrapCause = { Exception : Fault }"),
        base_db::FileId::from_raw(1),
    );
    let caller = base_db::FileText::new(
        &db,
        Arc::from("val audit : Fault -> TrapCause\nfunction audit(e) = Exception(e)"),
        base_db::FileId::from_raw(2),
    );
    hir_def::def_query::set_workspace_files(&mut db, vec![lib, model, caller]);
    let id = hir_def::def_query::file_def_with_body_ids(&db, caller)[0];
    clean(&crate::query::infer(&db, id).0);
    model
        .set_text(&mut db)
        .to(Arc::from("enum Fault = { Failed }\nunion TrapCause = { Exception : bool }"));
    let id = hir_def::def_query::file_def_with_body_ids(&db, caller)[0];
    assert!(unproved(&crate::query::infer(&db, id).0), "constructor edit left a stale proof");
    model
        .set_text(&mut db)
        .to(Arc::from("enum Fault = { Failed }\nunion TrapCause = { Exception : Fault }"));
    let id = hir_def::def_query::file_def_with_body_ids(&db, caller)[0];
    clean(&crate::query::infer(&db, id).0);
}

#[test]
fn expected_union_disambiguates_identical_constructor_payloads() {
    for result in ["First", "Second"] {
        clean(&workspace(
            &[
                "union First = { Shared : int }",
                "union Second = { Shared : int }",
                &format!("val audit : int -> {result}\nfunction audit(x) = Shared(x)"),
            ],
            "audit",
        ));
    }
}

#[test]
fn numeric_vector_slots_keep_inference_bindings_and_type_elements() {
    clean(&workspace(&[
        "val zeros : forall 'n, 'n >= 0. unit -> bits('n)\nval vector_init : forall 'n ('a : Type), 'n >= 0. 'a -> vector('n, 'a)",
        "val audit : forall 'n 'm 'q, 'n >= 0 & 'm >= 0 & 'q > 0. unit -> vector('q, vector('n, bits('m)))\nfunction audit() = { var result : vector('q, vector('n, bits('m))) = vector_init(vector_init(zeros())); result }",
    ], "audit"));
}
