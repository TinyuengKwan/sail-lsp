//! Exercise source-order inference through the configured project input path.
use crate::{apply_change::Change, root_database::RootDatabase};
use base_db::{
    module_graph::{ModuleData, ModuleGraph},
    preprocess::SourceProject,
    FileId,
};
use std::sync::Arc;

fn setup(
    sources: &[(&str, &str)],
    roots: &[usize],
    modules: Vec<ModuleData>,
) -> (RootDatabase, Vec<FileId>) {
    let mut db = RootDatabase::default();
    let ids: Vec<_> = (0..sources.len()).map(|i| FileId::from_raw(i as u32)).collect();
    let mut change = Change::new();
    for (&id, (_, text)) in ids.iter().zip(sources) {
        change.change_file(id, *text);
    }
    change.set_source_project(SourceProject {
        roots: roots.iter().map(|&i| ids[i]).collect(),
        paths: sources
            .iter()
            .zip(&ids)
            .map(|((path, _), &id)| (format!("/project/{path}").into(), id))
            .collect(),
        modules: ModuleGraph { modules, globals: vec![] },
        ..Default::default()
    });
    db.apply_change(change);
    (db, ids)
}
fn infer(db: &RootDatabase, file: FileId, name: &str) -> Arc<hir::TypeCheckResult> {
    let file = db.files().file_text(file).unwrap();
    let id = hir::def_query::file_def_with_body_ids(db, file)
        .iter()
        .find(|id| id.name(db) == name)
        .copied()
        .unwrap();
    hir::ty_query::infer(db, id).0.clone()
}
fn clean(result: &hir::TypeCheckResult) {
    assert!(result.inference_diagnostics().is_empty(), "{:?}", result.inference_diagnostics());
    assert!(result.type_mismatches.is_empty(), "{:?}", result.type_mismatches);
}

#[test]
fn implicit_declarations_preserve_singletons_unless_annotated() {
    for (statements, accepted) in [
        ("x = 1; x = 1; x", true),
        ("x = 1; x = 2; x", false),
        ("x : int = 1; x = 2; x", true),
        ("(x, y) = (1, 2); x = 2; x", false),
        ("x : int = x; x", false),
    ] {
        let source = format!("function run() -> int = {{ {statements} }}\n");
        let (db, ids) = setup(&[("main.sail", &source)], &[0], vec![]);
        let result = infer(&db, ids[0], "run");
        if accepted {
            clean(&result);
        } else {
            assert!(
                !result.inference_diagnostics().is_empty() || !result.type_mismatches.is_empty(),
                "invalid implicit declaration/write accepted: {source}"
            );
        }
    }
}

#[test]
fn implicit_declaration_decisions_track_hidden_globals_without_invalidating_other_bodies() {
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
    ];
    let (mut db, ids) = setup(
        &[
            ("defs.sail", "register x : int\n"),
            (
                "main.sail",
                "function run() -> int = { x : int = 1; x }\nfunction untouched() -> int = 1\n",
            ),
        ],
        &[0, 1],
        modules,
    );
    let untouched = infer(&db, ids[1], "untouched");
    clean(&untouched);
    for declaration in [
        "register x : int\n",
        "private register x : int\n",
        "type x = int\n",
        "private let x : int = 1\n",
        "register x : int\n",
    ] {
        let mut change = Change::new();
        change.change_file(ids[0], declaration);
        db.apply_change(change);
        let result = infer(&db, ids[1], "run");
        if declaration.starts_with("private") {
            assert!(
                !result.inference_diagnostics().is_empty(),
                "hidden global was inferred as an implicit local: {declaration}"
            );
        } else {
            clean(&result);
        }
        assert!(Arc::ptr_eq(&untouched, &infer(&db, ids[1], "untouched")));
    }
}

#[test]
fn repeated_signatures_change_later_inference_without_changing_earlier_bodies() {
    let text = "val f : unit -> int\nval a : unit -> int\nfunction a() = f()\nval f : unit -> bool\nval b : unit -> bool\nfunction b() = f()\n";
    let (mut db, ids) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = infer(&db, ids[0], "a");
    let b = infer(&db, ids[0], "b");
    clean(&a);
    clean(&b);
    let mut change = Change::new();
    change.change_file(ids[0], text.replace("val f : unit -> bool", "val f : unit -> int"));
    db.apply_change(change);
    assert!(
        Arc::ptr_eq(&a, &infer(&db, ids[0], "a")),
        "a later signature reinferred an earlier body"
    );
    assert!(!infer(&db, ids[0], "b").type_mismatches.is_empty());
}

#[test]
fn include_uses_preceding_parent_signature_and_body_edits_reuse_unrelated_results() {
    let parent = "val f : unit -> int\n$include \"child.sail\"\nval f : unit -> bool\nval after : unit -> bool\nfunction after() = f()\n";
    let child = "val inside : unit -> int\nfunction inside() = f()\nval untouched : unit -> int\nfunction untouched() = 1\n";
    let (mut db, ids) = setup(&[("main.sail", parent), ("child.sail", child)], &[0], vec![]);
    let inside = infer(&db, ids[1], "inside");
    let untouched = infer(&db, ids[1], "untouched");
    let after = infer(&db, ids[0], "after");
    clean(&inside);
    clean(&untouched);
    clean(&after);
    let mut change = Change::new();
    change.change_file(ids[1], child.replace("= f()", "= {\n f()\n}"));
    db.apply_change(change);
    assert!(!Arc::ptr_eq(&inside, &infer(&db, ids[1], "inside")));
    assert!(Arc::ptr_eq(&untouched, &infer(&db, ids[1], "untouched")));
    assert!(Arc::ptr_eq(&after, &infer(&db, ids[0], "after")));
}

#[test]
fn earlier_body_cannot_infer_from_a_future_signature() {
    let (db, ids) = setup(
        &[(
            "main.sail",
            "val caller : unit -> int\nfunction caller() = later()\nval later : unit -> int\n",
        )],
        &[0],
        vec![],
    );
    let result = infer(&db, ids[0], "caller");
    assert!(!result.inference_diagnostics().is_empty(), "a future signature satisfied a call");
}

#[test]
fn latest_signature_ownership_hides_a_previously_public_binding() {
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
    ];
    let (db, ids) = setup(
        &[
            ("a.sail", "val f : unit -> int\n"),
            (
                "b.sail",
                "private val f : unit -> bool\nval own : unit -> bool\nfunction own() = f()\n",
            ),
            ("c.sail", "val caller : unit -> int\nfunction caller() = f()\n"),
        ],
        &[0, 1, 2],
        modules,
    );
    clean(&infer(&db, ids[1], "own"));
    assert!(!infer(&db, ids[2], "caller").inference_diagnostics().is_empty());
}

#[test]
fn exported_signature_keeps_its_private_alias_support() {
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
    ];
    let (db, ids) = setup(
        &[
            ("a.sail", "private type Word = int\nval f : unit -> Word\n"),
            ("b.sail", "val caller : unit -> int\nfunction caller() = f()\n"),
        ],
        &[0, 1],
        modules,
    );
    clean(&infer(&db, ids[1], "caller"));
}

#[test]
fn ordered_constructor_checks_every_tuple_payload_component() {
    let declarations = "union Pair = { Pair : (int, bool) }\n";
    for (arguments, valid) in [("1, true", true), ("1, 2", false)] {
        let caller = format!("val caller : unit -> Pair\nfunction caller() = Pair({arguments})\n");
        let (db, ids) =
            setup(&[("types.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if valid {
            clean(&result);
        } else {
            assert!(
                !result.inference_diagnostics().is_empty() || !result.type_mismatches.is_empty()
            );
        }
    }
}

#[test]
fn ordered_implicit_result_width_comes_from_call_context() {
    let declarations = "val zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)\n";
    let caller = "val caller : unit -> bits(8)\nfunction caller() = zeros()\n";
    let (db, ids) = setup(&[("lib.sail", declarations), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "caller");
    clean(&result);
    assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()), "{:?}", result.type_of_expr);
}

#[test]
fn ordered_result_quantifiers_follow_alias_and_initializer_contexts() {
    let declarations = "type width : Int = 8\ntype Word = bits(width)\nval zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\n";
    let caller = "let zero : Word = zeros()\nregister R : Word = zeros()\nval zero_fn : unit -> Word\nfunction zero_fn() = zeros()\nval widened : bits(4) -> Word\nfunction widened(x) = extend(x)\n";
    let (db, ids) = setup(&[("lib.sail", declarations), ("main.sail", caller)], &[0, 1], vec![]);
    for name in ["zero_fn", "widened", "zero", "R"] {
        let result = infer(&db, ids[1], name);
        clean(&result);
        assert!(
            result.unsolved_constraints.is_empty(),
            "{name}: {:?}",
            result.unsolved_constraints
        );
    }
}

#[test]
fn ordered_initializer_annotations_check_width_constraints() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations =
            format!("val zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\n");
        let caller = "let zero : bits(8) = zeros()\nregister R : bits(8) = zeros()\n";
        let (db, ids) =
            setup(&[("lib.sail", &declarations), ("main.sail", caller)], &[0, 1], vec![]);
        for name in ["zero", "R"] {
            let result = infer(&db, ids[1], name);
            if valid {
                clean(&result);
                assert!(result.unsolved_constraints.is_empty());
            } else {
                assert!(
                    !result.inference_diagnostics().is_empty()
                        || !result.type_mismatches.is_empty(),
                    "{name}: lost initializer constraint"
                );
            }
        }
    }
}

#[test]
fn ordered_vector_updates_determine_replacement_widths() {
    let declarations = "val zeros : forall 'n, 'n >= 0. implicit('n) -> bits('n)\n";
    for (replacement, valid) in [("zeros()", true), ("0b00", true), ("0b000", false)] {
        let caller = format!(
            "val update : bits(8) -> bits(8)\nfunction update(x) = [x with 1..0 = {replacement}]\n"
        );
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "update");
        if valid {
            clean(&result);
            assert!(result.unsolved_constraints.is_empty(), "{:?}", result.unsolved_constraints);
        } else {
            assert!(!result.type_mismatches.is_empty());
        }
    }
}

#[test]
fn ordered_comparisons_determine_call_widths_in_both_operand_orders() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations = format!("type Word = bits(8)\nval zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\n");
        for expression in ["x == zeros()", "zeros() != x"] {
            let caller =
                format!("val compare : Word -> bool\nfunction compare(x) = {expression}\n");
            let (db, ids) =
                setup(&[("lib.sail", &declarations), ("main.sail", &caller)], &[0, 1], vec![]);
            let result = infer(&db, ids[1], "compare");
            if valid {
                clean(&result);
                assert!(
                    result.unsolved_constraints.is_empty(),
                    "{:?}",
                    result.unsolved_constraints
                );
            } else {
                assert!(
                    !result.inference_diagnostics().is_empty()
                        || !result.type_mismatches.is_empty(),
                    "comparison lost call predicate"
                );
            }
        }
    }
}

#[test]
fn ordered_assignments_supply_the_target_type_to_calls() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations = format!("type Word = bits(8)\nval zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\nregister R : Word\nbitfield B : bits(16) = {{ field : 7..0 }}\nregister BR : B\n");
        for expression in ["R = zeros()", "BR[field] = zeros()"] {
            let caller =
                format!("val assign : unit -> unit\nfunction assign() = {{ {expression} }}\n");
            let (db, ids) =
                setup(&[("lib.sail", &declarations), ("main.sail", &caller)], &[0, 1], vec![]);
            let result = infer(&db, ids[1], "assign");
            if valid {
                clean(&result);
                assert!(
                    result.unsolved_constraints.is_empty(),
                    "{:?}",
                    result.unsolved_constraints
                );
            } else {
                assert!(
                    !result.inference_diagnostics().is_empty()
                        || !result.type_mismatches.is_empty(),
                    "assignment lost call predicate"
                );
            }
        }
    }
}

#[test]
fn ordered_winning_call_keeps_nested_argument_result_bindings() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations = format!("val zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\nval fill : forall 'n 'a, 'n >= 0. 'a -> vector('n, 'a)\n");
        let caller = "let rows : vector(2, vector(4, bits(8))) = fill(fill(zeros()))\n";
        let (db, ids) =
            setup(&[("lib.sail", &declarations), ("main.sail", caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "rows");
        if valid {
            clean(&result);
            assert!(result.unsolved_constraints.is_empty(), "{:?}", result.unsolved_constraints);
        } else {
            assert!(
                !result.inference_diagnostics().is_empty() || !result.type_mismatches.is_empty(),
                "winner confirmation lost nested call constraint"
            );
        }
    }
}

#[test]
fn ordered_boolean_calls_instantiate_compound_argument_predicates() {
    let declarations =
        "val negate : forall 'p. bool('p) -> bool(not('p))\nval get : unit -> bool\n";
    for expression in [
        "negate(x)",
        "negate(x | y)",
        "{ let b = x | y; negate(b) }",
        "negate(get() | get())",
        "{ let b = get() | get(); negate(b) }",
    ] {
        let caller =
            format!("val compare : (bool, bool) -> bool\nfunction compare(x, y) = {expression}\n");
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "compare");
        clean(&result);
        assert!(
            result.unsolved_constraints.is_empty(),
            "{expression}: {:?}",
            result.unsolved_constraints
        );
    }
}

#[test]
fn ordered_inline_boolean_quantifiers_keep_their_kind_and_predicate() {
    let declarations =
        "function require_true forall ('b : Bool), 'b . (_ : bool('b)) -> unit = ()\n";
    for (value, valid) in [("true", true), ("false", false)] {
        let caller = format!("function caller() -> unit = require_true({value})\n");
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if valid {
            clean(&result);
            assert!(result.unsolved_constraints.is_empty(), "{:?}", result.unsolved_constraints);
        } else {
            assert!(
                result.inference_diagnostics().iter().any(|d| matches!(
                    d,
                    hir::InferenceDiagnostic::CallConstraintViolation { proved_false: true, .. }
                )),
                "{:?}",
                result.inference_diagnostics()
            );
        }
    }
}

#[test]
fn ordered_directional_mapping_bodies_receive_the_output_context() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations =
            format!("val zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\n");
        for direction in ["forwards", "backwards"] {
            let caller =
                format!("mapping encode : bits(4) <-> bits(8) = {{ {direction} _ => zeros() }}\n");
            let (db, ids) =
                setup(&[("lib.sail", &declarations), ("main.sail", &caller)], &[0, 1], vec![]);
            let result = infer(&db, ids[1], "encode");
            if valid {
                clean(&result);
                assert!(
                    result.unsolved_constraints.is_empty(),
                    "{:?}",
                    result.unsolved_constraints
                );
            } else {
                assert!(
                    !result.inference_diagnostics().is_empty()
                        || !result.type_mismatches.is_empty(),
                    "mapping lost call constraint"
                );
            }
        }
    }
}

#[test]
fn ordered_constructor_comparisons_share_generic_type_arguments() {
    let declarations = "union Option('a : Type) = { None : unit, Some : 'a }\nval eq_anything : forall 'a. ('a, 'a) -> bool\noverload operator == = {eq_anything}\noverload operator != = {eq_anything}\n";
    for expression in ["x == None()", "None() != x"] {
        let caller =
            format!("val compare : Option(bits(8)) -> bool\nfunction compare(x) = {expression}\n");
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "compare");
        clean(&result);
        assert!(result.unsolved_constraints.is_empty(), "{:?}", result.unsolved_constraints);
    }
}

#[test]
fn ordered_if_without_else_checks_its_branch_as_unit() {
    let declarations = "val fail : forall 'a. unit -> 'a\n";
    for (branch, valid) in [("fail()", true), ("1", false)] {
        let caller =
            format!("function caller(b : bool) -> bool = {{ if b then {branch}; true }}\n");
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if valid {
            clean(&result);
        } else {
            assert!(!result.type_mismatches.is_empty());
        }
    }
}

#[test]
fn ordered_nested_call_keeps_contextual_width_and_checks_its_constraint() {
    for (minimum, valid) in [(0, true), (16, false)] {
        let declarations = format!("val zeros : forall 'n, 'n >= {minimum}. implicit('n) -> bits('n)\nval consume : bits(8) -> bool\n");
        let caller = "val caller : unit -> bool\nfunction caller() = consume(zeros())\n";
        let (db, ids) =
            setup(&[("lib.sail", &declarations), ("main.sail", caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if valid {
            clean(&result);
            assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
        } else {
            assert!(
                !result.inference_diagnostics().is_empty() || !result.type_mismatches.is_empty(),
                "late width binding lost the callee constraint"
            );
        }
    }
}

#[test]
fn ordered_values_infer_initializers_without_exposing_later_bindings() {
    let definitions = "type flag : Bool = config base.flag\nlet enabled = constraint(flag)\nenum E = { A, B }\nlet members = [A, B]\n";
    let caller = "val caller : unit -> bool\nfunction caller() = enabled\nval first : unit -> E\nfunction first() = members[0]\n";
    let (mut db, ids) =
        setup(&[("values.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    let before = infer(&db, ids[1], "first");
    for name in ["caller", "first"] {
        let result = infer(&db, ids[1], name);
        clean(&result);
        assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
    }
    let mut change = Change::new();
    change.change_file(ids[0], definitions.replace("[A, B]", "[B, A]"));
    db.apply_change(change);
    assert!(
        Arc::ptr_eq(&before, &infer(&db, ids[1], "first")),
        "same initializer type reinferred the consumer"
    );
}

#[test]
fn ordered_alias_support_includes_preceding_configuration_facts() {
    let definitions = "type bits('n) = bitvector('n)\ntype xlen : Int = config base.xlen\nconstraint xlen in {32, 64}\ntype xlenbits = bits(xlen)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\n";
    let caller = "val caller : bits(32) -> xlenbits\nfunction caller(x) = extend(x)\n";
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "caller");
    clean(&result);
    assert!(result.unsolved_constraints.is_empty(), "{:?}", result.unsolved_constraints);
}

#[test]
fn ordered_alias_support_uses_facts_at_the_body_but_not_future_facts() {
    let definitions = "type bits('n) = bitvector('n)\ntype xlen : Int = config base.xlen\ntype xlenbits = bits(xlen)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\nval caller : bits(32) -> xlenbits\n";
    let body = "function caller(x) = extend(x)\n";
    let fact = "constraint xlen in {32, 64}\n";
    for before in [false, true] {
        let text = if before {
            format!("{definitions}{fact}{body}")
        } else {
            format!("{definitions}{body}{fact}")
        };
        let (db, ids) = setup(&[("main.sail", &text)], &[0], vec![]);
        let result = infer(&db, ids[0], "caller");
        clean(&result);
        assert_eq!(
            result.unsolved_constraints.is_empty(),
            before,
            "{before}: {:?}",
            result.unsolved_constraints
        );
    }
}

#[test]
fn ordered_generic_constructor_matches_use_instantiated_tuple_payloads() {
    let definitions = "union result('a : Type, 'b : Type) = { Ok : 'a, Err : 'b }\n";
    for (arms, exhaustive) in [
        ("Ok(true, _) => (), Ok(false, _) => (), Err() => ()", true),
        ("Ok(true, _) => (), Err() => ()", false),
        ("Ok((true, _)) => (), Ok(false, _) => (), Err() => ()", true),
    ] {
        let caller = format!("val caller : result((bool, bool), unit) -> unit\nfunction caller(x) = match x {{ {arms} }}\n");
        let (db, ids) =
            setup(&[("lib.sail", definitions), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        assert!(result.type_mismatches.is_empty(), "{:?}", result.type_mismatches);
        if exhaustive {
            clean(&result);
        } else {
            assert!(!result.inference_diagnostics().is_empty(), "missing tuple cases accepted");
        }
    }
}

#[test]
fn ordered_enum_attributes_generate_bindings_at_the_enum_declaration() {
    let definitions = "$[enum_vector all_e]\n$[enum_number_conversions { to_enum = to_e, from_enum = from_e }]\nenum E = { A, B }\n";
    let caller = "val members : unit -> vector(2, E)\nfunction members() = all_e\nval number : E -> int\nfunction number(e) = from_e(e)\nval item : range(0, 1) -> E\nfunction item(n) = to_e(n)\n";
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    for name in ["members", "number", "item"] {
        let result = infer(&db, ids[1], name);
        clean(&result);
        assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
    }
    let invalid = caller.replace("range(0, 1)", "int");
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", &invalid)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "item");
    assert!(
        result
            .inference_diagnostics()
            .iter()
            .any(|d| matches!(d, hir::InferenceDiagnostic::CallConstraintViolation { .. })),
        "unbounded integers cannot be converted to this enum: {:?}",
        result.inference_diagnostics()
    );
}

#[test]
fn ordered_scattered_members_are_known_at_the_match_and_end_closes_the_type() {
    let text = "scattered union I\nunion R = { Done : unit, Redirect : I }\nunion clause I = A : unit\nval early : I -> unit\nfunction early(x) = match x { A() => () }\nunion clause I = B : unit\nend I\nval late : I -> unit\nfunction late(x) = match x { A() => () }\nval nested : R -> unit\nfunction nested(x) = match x { Done() => (), Redirect(_) => () }\n";
    let (db, ids) = setup(&[("main.sail", text)], &[0], vec![]);
    clean(&infer(&db, ids[0], "early"));
    clean(&infer(&db, ids[0], "nested"));
    let late = infer(&db, ids[0], "late");
    assert!(
        late.inference_diagnostics()
            .iter()
            .any(|d| matches!(d, hir::InferenceDiagnostic::IncompleteMatch { .. })),
        "closed scattered type lost missing constructor: {:?}",
        late.inference_diagnostics()
    );
}

#[test]
fn ordered_initializers_do_not_see_their_own_binding() {
    for definition in ["let x : int = x\n", "register x : int = x\n"] {
        let (db, ids) = setup(&[("main.sail", definition)], &[0], vec![]);
        let result = infer(&db, ids[0], "x");
        assert!(
            result.inference_diagnostics().iter().any(|d| matches!(d,
            hir::InferenceDiagnostic::UnresolvedIdent { name, .. } if name == "x")),
            "initializer resolved its own declaration: {:?}",
            result.inference_diagnostics()
        );
    }
    let (db, ids) =
        setup(&[("main.sail", "let earlier : int = 1\nlet x : int = earlier\n")], &[0], vec![]);
    clean(&infer(&db, ids[0], "x"));
}

#[test]
fn ordered_overloads_include_mapping_members() {
    let definitions = "enum E = { A, B }\nmapping display_e : E <-> string = { A <-> \"a\", B <-> \"b\" }\noverload display = {display_e}\n";
    let caller = "val caller : E -> string\nfunction caller(x) = display(x)\n";
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "caller");
    clean(&result);
    assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
}

#[test]
fn ordered_mapping_constructor_keeps_its_own_side_bindings() {
    let definitions = "type bits('n) = bitvector('n)\ntype Count = {1, 2}\nmapping count_bits : bits(1) <-> Count = { 0b0 <-> 1, 0b1 <-> 2 }\nunion I = { C : (Count, bits(1)) }\n";
    let caller = "val encode : I <-> bits(2)\nmapping encode = { C(n, b) <-> count_bits(n) @ b }\n";
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "encode");
    clean(&result);
    assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
}

#[test]
fn ordered_runtime_width_is_opened_before_the_enclosing_call() {
    let definitions = "type bits('n) = bitvector('n)\nval ones : forall 'n, 'n >= 0. implicit('n) -> bits('n)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\n";
    for width in [8, 2] {
        let caller = format!(
            "val caller : range(1, 4) -> bits({width})\nfunction caller(n) = extend(ones(n))\n"
        );
        let (db, ids) =
            setup(&[("lib.sail", definitions), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if width == 8 {
            clean(&result);
            assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
        } else {
            assert!(
                !result.refuted_constraints.is_empty()
                    || !result.inference_diagnostics().is_empty()
            );
        }
    }
}

#[test]
fn ordered_constructor_context_can_determine_a_nested_extension() {
    let definitions = "type bits('n) = bitvector('n)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\nunion Address = { Address : bits(64) }\n";
    let caller = "val caller : forall 'n, 'n in {32, 64}. bits('n) -> Address\nfunction caller(x) = Address(extend(x))\n";
    let (db, ids) = setup(&[("lib.sail", definitions), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "caller");
    clean(&result);
    assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
}

#[test]
fn ordered_boolean_branch_join_does_not_require_equal_values() {
    let declarations = "val negate_bool : forall 'p. bool('p) -> bool(not('p))\n";
    let caller = "val caller : (bits(2), bool) -> bool\nfunction caller(flags, value) = { let result = if flags == 0b00 then false else negate_bool(value); result }\n";
    let (db, ids) = setup(&[("lib.sail", declarations), ("main.sail", caller)], &[0, 1], vec![]);
    clean(&infer(&db, ids[1], "caller"));
}

#[test]
fn ordered_generic_alias_checks_a_tuple_constructor() {
    let declarations = "type bits('n) = bitvector('n)\nunion result('a, 'b) = { Ok : 'a, Err : 'b }\nstruct Output('n : Int) = { data : bits('n) }\ntype Context = unit\ntype Outcome('n : Int) = result((Output('n), Context), (int, Context))\n";
    let caller = "val caller : forall 'n. (bits('n), Context) -> Outcome('n)\nfunction caller(data, ctx) = Ok(struct { data = data }, ctx)\n";
    let (db, ids) = setup(&[("lib.sail", declarations), ("main.sail", caller)], &[0, 1], vec![]);
    clean(&infer(&db, ids[1], "caller"));
}

#[test]
fn ordered_bitwise_operand_supplies_nested_unary_width() {
    let declarations = "type bits('n) = bitvector('n)\nval ones : forall 'n, 'n >= 0. implicit('n) -> bits('n)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\nval complement_bit : bit -> bit\nval complement_bits : forall 'n. bits('n) -> bits('n)\noverload complement = {complement_bit, complement_bits}\n";
    let caller = "val caller : (bits(64), range(1, 8)) -> bits(64)\nfunction caller(x, n) = x & complement(extend(ones(n)))\n";
    let (db, ids) = setup(&[("lib.sail", declarations), ("main.sail", caller)], &[0, 1], vec![]);
    let result = infer(&db, ids[1], "caller");
    clean(&result);
    assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
}

#[test]
fn ordered_existential_return_retains_its_argument_bounds() {
    let declarations = "type bits('n) = bitvector('n)\nval ones : forall 'n, 'n >= 0. implicit('n) -> bits('n)\nval extend : forall 'n 'm, 'm >= 'n. bits('n) -> bits('m)\nval minimum : forall 'a 'b. (int('a), int('b)) -> { 'z, ('a < 'b & 'z == 'a) | ('a >= 'b & 'z == 'b). int('z) }\n";
    for (argument, valid) in [("n", true), ("-1", false)] {
        let caller = format!("val caller : range(1, 4) -> bits(8)\nfunction caller(n) = extend(ones(minimum({argument}, 4)))\n");
        let (db, ids) =
            setup(&[("lib.sail", declarations), ("main.sail", &caller)], &[0, 1], vec![]);
        let result = infer(&db, ids[1], "caller");
        if valid {
            clean(&result);
            assert!(result.type_of_expr.iter().all(|(_, ty)| !ty.is_error()));
        } else {
            assert!(
                !result.inference_diagnostics().is_empty()
                    || !result.refuted_constraints.is_empty()
            );
        }
    }
}

#[test]
fn inline_program_keeps_selected_signatures_and_prefix_dependencies() {
    let source = "$project# A {}\n$start_module# A\nval f : unit -> int\nfunction early() -> int = f()\nval f : unit -> bool\nfunction late() -> bool = f()\n$end_module#\n";
    let mut db = RootDatabase::default();
    let id = FileId::from_raw(0);
    let mut change = Change::new();
    change.change_file(id, source);
    change.set_workspace_files(vec![id]);
    db.apply_change(change);
    let early = infer(&db, id, "early");
    clean(&early);
    clean(&infer(&db, id, "late"));
    let mut change = Change::new();
    change.change_file(id, source.replace("val f : unit -> bool", "val f : unit -> int"));
    db.apply_change(change);
    assert!(
        Arc::ptr_eq(&early, &infer(&db, id, "early")),
        "future signature invalidated early body"
    );
    assert!(!infer(&db, id, "late").type_mismatches.is_empty());
}

#[test]
fn inline_program_keeps_external_support_without_importing_future_local_bindings() {
    for future in [false, true] {
        let source = if future {
            "$project# A {}\n$start_module# A\nfunction run() -> int = future()\nval future : unit -> int\n$end_module#\n"
        } else {
            "$project# A {}\n$start_module# A\nfunction run() -> int = external(true)\n$end_module#\n"
        };
        let mut db = RootDatabase::default();
        let id = FileId::from_raw(0);
        let mut change = Change::new();
        change.change_file(id, source);
        change.change_file(
            FileId::from_raw(1),
            "val external : bool -> int\nval future : unit -> int\n",
        );
        change.set_workspace_files(vec![id, FileId::from_raw(1)]);
        db.apply_change(change);
        let result = infer(&db, id, "run");
        if future {
            assert!(
                !result.inference_diagnostics().is_empty(),
                "future binding leaked from workspace fallback"
            );
        } else {
            clean(&result);
        }
    }
}

#[test]
fn inline_program_shared_resolution_and_signatures_exclude_hidden_alternatives() {
    let source = "$project# A {} B {} C { requires A }\n$start_module# A\nval one : int -> int\noverload inner = {one}\n$end_module#\n$start_module# B\nval two : bool -> int\noverload inner = {two}\noverload outer = {inner}\n$end_module#\n$start_module# C\nfunction run(x : int) -> int = outer(x)\n$end_module#\n";
    let file = crate::test_utils::TestFile::new(source);
    let at = source.rfind("outer(x)").unwrap();
    let signatures =
        crate::signatures::callable_signatures_at(file.db(), file.input(), at, "outer").unwrap();
    assert_eq!(signatures.len(), 1, "{signatures:?}");
    assert!(signatures[0].label.contains("one"), "{signatures:?}");
    let sema = hir::Semantics::new(file.db());
    assert!(sema.resolve_path(file.db(), file.input(), at, "one").is_some());
    assert!(sema.resolve_path(file.db(), file.input(), at, "two").is_none());
    let names = sema.names_in_scope(file.input(), at);
    assert!(names.iter().any(|n| n.as_str() == "one"));
    assert!(!names.iter().any(|n| n.as_str() == "two"));
}

#[test]
fn constructor_privacy_edits_invalidate_users_but_not_unrelated_bodies() {
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
    ];
    let (mut db, ids) = setup(
        &[
            ("defs.sail", "union U = { Hidden : unit, Public : unit }\n"),
            ("main.sail", "function use() -> U = Hidden()\nfunction untouched() -> int = 1\n"),
        ],
        &[0, 1],
        modules,
    );
    clean(&infer(&db, ids[1], "use"));
    let untouched = infer(&db, ids[1], "untouched");
    for private in [true, false] {
        let mut change = Change::new();
        change.change_file(
            ids[0],
            format!(
                "union U = {{ {}Hidden : unit, Public : unit }}\n",
                if private { "private " } else { "" }
            ),
        );
        db.apply_change(change);
        let result = infer(&db, ids[1], "use");
        if private {
            assert!(
                !result.inference_diagnostics().is_empty(),
                "private constructor was still inferred: {result:?}"
            );
        } else {
            clean(&result);
        }
        assert!(Arc::ptr_eq(&untouched, &infer(&db, ids[1], "untouched")));
    }
}

#[test]
fn enum_helper_calls_check_nominal_visibility_after_selecting_public_signatures() {
    for private in [false, true] {
        for scope in ["owner", "requires", "unrelated"] {
            let definitions = format!("default Order dec\n$[enum_vector \"all_E\"]\n{}enum E with label -> string = {{ A => \"A\", B => \"B\" }}\n", if private { "private " } else { "" });
            for expression in
                ["E_of_num(0)", "num_of_E(E_of_num(0))", "E_label(E_of_num(0))", "all_E"]
            {
                let body = format!("function run() -> unit = {{ let _x = {expression}; () }}\n");
                let modules = if scope == "owner" {
                    vec![ModuleData {
                        files: vec![FileId::from_raw(0), FileId::from_raw(1)],
                        requires: vec![],
                    }]
                } else {
                    vec![
                        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                        ModuleData {
                            files: vec![FileId::from_raw(1)],
                            requires: if scope == "requires" { vec![0] } else { vec![] },
                        },
                    ]
                };
                let (db, ids) =
                    setup(&[("defs.sail", &definitions), ("main.sail", &body)], &[0, 1], modules);
                let result = infer(&db, ids[1], "run");
                if scope == "owner" || (!private && scope == "requires") {
                    clean(&result);
                } else {
                    assert!(
                        !result.inference_diagnostics().is_empty(),
                        "private={private} scope={scope} expr={expression}: {result:?}"
                    );
                    if private && scope == "requires" && expression != "all_E" {
                        assert!(result.inference_diagnostics().iter().any(|d| matches!(d, hir::InferenceDiagnostic::InvalidTypeUse { message, .. } if message.contains("type E"))), "{result:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn enum_type_privacy_edits_invalidate_helper_users_selectively() {
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
    ];
    let (mut db, ids) = setup(&[
        ("defs.sail", "enum E = { A, B }\n"),
        ("main.sail", "function run() -> unit = { let _x = E_of_num(0); () }\nfunction untouched() -> int = 1\n"),
    ], &[0,1], modules);
    clean(&infer(&db, ids[1], "run"));
    let untouched = infer(&db, ids[1], "untouched");
    for private in [true, false] {
        let mut change = Change::new();
        change.change_file(
            ids[0],
            format!("{}enum E = {{ A, B }}\n", if private { "private " } else { "" }),
        );
        db.apply_change(change);
        let result = infer(&db, ids[1], "run");
        if private {
            assert!(result.inference_diagnostics().iter().any(|d| matches!(d, hir::InferenceDiagnostic::InvalidTypeUse { message, .. } if message.contains("type E"))), "{result:?}");
        } else {
            clean(&result);
        }
        assert!(Arc::ptr_eq(&untouched, &infer(&db, ids[1], "untouched")));
    }
}

#[test]
fn scattered_enum_vector_edits_invalidate_length_and_availability_selectively() {
    let (mut db, ids) = setup(
        &[
            ("head.sail", "default Order dec\n$[enum_vector \"all_E\"]\nscattered enum E\n"),
            ("clause.sail", "enum clause E = EA\n"),
            ("end.sail", "end E\n"),
            (
                "main.sail",
                "function values() -> vector(1, E) = all_E\nfunction untouched() -> int = 1\n",
            ),
        ],
        &[0, 1, 2, 3],
        vec![],
    );
    clean(&infer(&db, ids[3], "values"));
    let untouched = infer(&db, ids[3], "untouched");
    for (clause, end, valid) in [
        ("enum clause E = EA\nenum clause E = EB\n", "end E\n", false),
        ("enum clause E = EA\n", "end E\n", true),
        ("enum clause E = EA\n", "", false),
        ("enum clause E = EA\n", "end E\n", true),
    ] {
        let mut change = Change::new();
        change.change_file(ids[1], clause);
        change.change_file(ids[2], end);
        db.apply_change(change);
        let result = infer(&db, ids[3], "values");
        if valid {
            clean(&result);
        } else {
            assert!(
                !result.inference_diagnostics().is_empty() || !result.type_mismatches.is_empty(),
                "{result:?}"
            );
        }
        assert!(
            Arc::ptr_eq(&untouched, &infer(&db, ids[3], "untouched")),
            "clause={clause} end={end}"
        );
    }
}
