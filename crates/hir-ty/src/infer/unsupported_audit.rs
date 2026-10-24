//! Unsupported compatibility goals and retained numeric evidence.
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
fn symbolic_slice_retains_width() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\nval audit : forall 'n, 'n > 0. bits(2 * 'n) -> bits('n)\nfunction audit(x) = x['n - 1 .. 0]",
    ], "audit"));
}
#[test]
fn bit_argument_binds_the_polymorphic_width() {
    clean(&workspace(
        &[
            "type bits('n) = bitvector('n)\nval identity : forall 'n. bits('n) -> bits('n)",
            "val audit : bit -> bits(1)\nfunction audit(x) = identity(x)",
        ],
        "audit",
    ));
}
#[test]
fn literal_singleton_evidence_survives_candidate_confirmation() {
    clean(&workspace(
        &[
            "val shift : forall 'n, 'n >= 0. int('n) -> unit",
            "val audit : unit -> unit\nfunction audit() = shift(7)",
        ],
        "audit",
    ));
}

#[test]
fn explicit_implicit_length_is_not_dropped_across_files() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\nval zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)",
        "val audit : unit -> bits(5)\nfunction audit() = zeros(5)",
    ], "audit"));
}
#[test]
fn configured_alias_checks_symbolic_slice_width() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\ntype xlen : Int = config base.xlen\nconstraint xlen in {32, 64}\nlet xlen = sizeof(xlen)\ntype xlenbits = bits(xlen)",
        "val audit : bits(64) -> xlenbits\nfunction audit(x) = x[xlen - 1 .. 0]",
    ], "audit"));
}
#[test]
fn contextual_width_for_cross_file_zeros() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\ntype xlen : Int = config base.xlen\nconstraint xlen in {32, 64}\nlet xlen = sizeof(xlen)\ntype xlenbits = bits(xlen)\nval zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)",
        "val audit : unit -> xlenbits\nfunction audit() = zeros()",
    ], "audit"));
}

#[test]
fn length_inside_slice_endpoint() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\nval bitvector_length : forall 'n. bits('n) -> int('n)\noverload length = {bitvector_length}",
        "val audit : forall 'n, 'n > 0. bits('n) -> bits('n)\nfunction audit(x) = x[length(x) - 1 .. 0]",
    ], "audit"));
}

#[test]
fn unsupported_compiler_oracles() {
    macro_rules! pair {
        ($name:literal) => {{
            let good = include_str!(concat!(
                "../../test_data/unsupported_audit/positive/",
                $name,
                ".sail"
            ));
            let bad = include_str!(concat!(
                "../../test_data/unsupported_audit/negative/",
                $name,
                ".sail"
            ));
            clean(&workspace(&good.split("// audit-body").collect::<Vec<_>>(), "audit"));
            let result = workspace(&bad.split("// audit-body").collect::<Vec<_>>(), "audit");
            assert!(
                !result.type_mismatches.is_empty()
                    || !result.unsolved_constraints.is_empty()
                    || !result.inference_diagnostics().is_empty(),
                "{}: invalid fixture silently accepted",
                $name
            );
        }};
    }
    pair!("symbolic_slice");
    pair!("bit_width");
    pair!("explicit_implicit");
    pair!("extension_constraint");
    pair!("range_singleton");
    pair!("length_slice");
}

#[test]
fn nested_flexible_placeholder_is_not_a_universal_counterexample() {
    let mut table = InferenceTable::new();
    let outer = table.new_type_var();
    let inner = table.new_type_var();
    let TyKind::Infer(outer_id) = outer.kind() else { panic!() };
    let TyKind::Infer(inner_id) = inner.kind() else { panic!() };
    table.register_placeholder("'_outer", outer_id.0);
    table.register_placeholder("'_inner", inner_id.0);
    assert!(table.bind(outer_id.0, Ty::param("8 * '_inner")));
    let goal = parse_constraint_text("'_outer >= 0");
    let (resolved, flexible) = table.resolve_deferred_constraint(&goal);
    assert!(flexible);
    assert_eq!(resolved, parse_constraint_text("8 * '_inner >= 0"));
    assert!(table.bind(inner_id.0, Ty::param("4")));
    let (resolved, flexible) = table.resolve_deferred_constraint(&goal);
    assert!(!flexible);
    assert_eq!(solver::solve(&resolved, &Subst::default(), &[]), ConstraintStatus::Satisfied);
}

#[test]
fn nested_conditionals_preserve_numeric_ast() {
    let text = "(if 8 * 2 ^ (if 'v == 32 then 2 else 3) == 32 then 22 else 44)";
    let n = NumericExpr::parse(text).expect("nested then must not terminate the outer condition");
    let NumericExpr::If { ref cond, .. } = n else { panic!("{n:?}") };
    assert!(!super::obligation_stats::unsupported(cond));
    assert_eq!(NumericExpr::parse(&n.to_string_repr()), Some(n.clone()));
    let goal = ConstraintExpr::Compare {
        lhs: NumericExpr::Const(64),
        op: crate::ty::CompareOp::Gte,
        rhs: n,
    };
    assert_eq!(solver::solve(&goal, &Subst::default(), &[]), ConstraintStatus::Satisfied);
}

#[test]
fn unknown_runtime_slice_width_is_not_chosen_from_the_return_type() {
    let result = workspace(&[
        "type bits('n) = bitvector('n)\nval audit : (bits(32), int) -> bits(8)\nfunction audit(x, i) = x[i .. 0]",
    ], "audit");
    assert!(
        !result.unsolved_constraints.is_empty()
            || !result.refuted_constraints.is_empty()
            || !result.type_mismatches.is_empty(),
        "runtime slice was silently assigned width 8"
    );
}
#[test]
fn conditional_runtime_arithmetic_retains_its_identity() {
    clean(&workspace(&[
        "type bits('n) = bitvector('n)\nval ones : forall 'n, 'n >= 0. implicit('n) -> bits('n)",
        "val audit : forall 'v, 'v in {32, 64}. (int('v), range(0, 3)) -> bits(3 * (if 'v == 32 then 10 else 9))\nfunction audit(v, level) = { let shift = 3 * (if 'v == 32 then 10 else 9); ones(shift) }",
    ], "audit"));
}
