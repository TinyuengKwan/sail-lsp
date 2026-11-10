//! Apply workspace refactorings, reload and check resulting semantics.
use super::hierarchy::tests::{setup, snapshot};
use super::*;
use ide_db::source_change::SourceChange;

fn remove(analysis: &Analysis, url: &url::Url, needle: &str) -> SourceChange {
    let file = analysis.file(url).unwrap();
    let at = file.text().find(needle).unwrap();
    analysis
        .assists(url, base_db::text_range(at, at), &Default::default())
        .unwrap()
        .into_iter()
        .find(|assist| assist.id.0 == "remove_unused_param")
        .expect("parameter removal must be offered")
        .source_change
        .unwrap()
}

fn apply(host: &mut AnalysisHost, map: &UrlMap, change: SourceChange) -> HashMap<FileId, String> {
    let analysis = snapshot(host, map);
    let mut updated = HashMap::new();
    let mut inputs = ide_db::apply_change::Change::new();
    for (id, mut edits) in change.source_file_edits {
        let mut source = analysis.file_by_id(id).unwrap().text().to_owned();
        edits.sort_by_key(|edit| edit.range.start());
        for edit in edits.iter().rev() {
            source.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        assert!(syntax::parse_text(&source).1.is_empty(), "{source}");
        inputs.change_file(id, &source);
        updated.insert(id, source);
    }
    drop(analysis);
    host.apply_change(inputs);
    let analysis = snapshot(host, map);
    for &id in updated.keys() {
        let diagnostics = analysis.file_diagnostics(id, &Default::default()).unwrap();
        assert!(
            diagnostics.iter().all(|d| d.severity != ide_diagnostics::Severity::Error),
            "{}\n{diagnostics:?}",
            updated[&id]
        );
    }
    updated
}

#[test]
fn parameter_removal_updates_cross_file_val_implementation_and_effectful_call() {
    let sources = [
        ("spec.sail", "val f : (int, int) -> int\n"),
        ("body.sail", "function f(x, y) = x\n"),
        ("caller.sail", "val tick : unit -> int\nfunction caller() -> int = f(1, tick())\n"),
    ];
    let (mut host, map, urls) = setup(&sources, &[0, 1, 2], vec![]);
    let analysis = snapshot(&host, &map);
    let change = remove(&analysis, &urls[1], "y)");
    assert_eq!(change.source_file_edits.len(), 3);
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    assert!(updated[&FileId::from_raw(0)].contains("val f : int -> int"));
    assert!(updated[&FileId::from_raw(1)].contains("function f(x)"));
    let caller = &updated[&FileId::from_raw(2)];
    assert_eq!(caller.matches("tick()").count(), 1);
    assert!(caller.contains("f(__sail_argument0)"));
}

#[test]
fn parameter_removal_updates_every_scattered_clause() {
    let sources = [
        ("spec.sail", "val f : (int, int) -> int\nscattered function f\n"),
        ("first.sail", "function clause f(x, y) = x\n"),
        ("second.sail", "function clause f(a, b) = a\nend f\nfunction caller() -> int = f(1, 2)\n"),
    ];
    let (mut host, map, urls) = setup(&sources, &[0, 1, 2], vec![]);
    let analysis = snapshot(&host, &map);
    let change = remove(&analysis, &urls[1], "y)");
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    assert!(updated[&FileId::from_raw(1)].contains("function clause f(x)"));
    assert!(updated[&FileId::from_raw(2)].contains("function clause f(a)"));
}

#[test]
fn parameter_removal_keeps_implicit_positions_and_nested_calls() {
    let sources = [
        ("spec.sail", "default Order dec\ntype bits('n : Int) = bitvector('n, dec)\nval f : forall 'n, 'n >= 0. (implicit('n), bits('n), int) -> bits('n)\n"),
        ("body.sail", "function f(n, x, y) = x\n"),
        ("caller.sail", "function caller(x : bits(8)) -> bits(8) = f(f(x, 1), 2)\n"),
    ];
    let (mut host, map, urls) = setup(&sources, &[0, 1, 2], vec![]);
    let analysis = snapshot(&host, &map);
    let change = remove(&analysis, &urls[1], "y)");
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    assert!(updated[&FileId::from_raw(0)].contains("(implicit('n), bits('n))"));
    assert!(updated[&FileId::from_raw(1)].contains("function f(n, x)"));
    let caller = &updated[&FileId::from_raw(2)];
    assert!(!caller.contains("f(x, 1)"));
    assert!(!caller.contains("f(__sail_argument0, "));
}

#[test]
fn parameter_removal_updates_selected_overload_calls_only() {
    let sources = [
        ("defs.sail", "val fint : (int, int) -> int\nfunction fint(x, y) = x\nval fbool : (bool, int) -> bool\nfunction fbool(x, y) = x\noverload choose = { fint, fbool }\n"),
        ("caller.sail", "function a() -> int = choose(1, 2)\nfunction b() -> bool = choose(true, 3)\n"),
    ];
    let (mut host, map, urls) = setup(&sources, &[0, 1], vec![]);
    let analysis = snapshot(&host, &map);
    let change = remove(&analysis, &urls[0], "y)");
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    let caller = &updated[&FileId::from_raw(1)];
    assert!(caller.contains("choose(__sail_argument0)"));
    assert!(caller.contains("choose(true, 3)"));
}

#[test]
#[ignore = "requires SAIL_ORACLE_BIN and SAIL_ORACLE_SRC; executes real Sail interpreter"]
fn upstream_refactoring_execution_preserves_effects_and_argument_order() {
    use std::{path::PathBuf, process::Command};
    let compiler =
        PathBuf::from(std::env::var_os("SAIL_ORACLE_BIN").expect("SAIL_ORACLE_BIN required"));
    let library =
        PathBuf::from(std::env::var_os("SAIL_ORACLE_SRC").expect("SAIL_ORACLE_SRC required"));
    let directory =
        std::env::temp_dir().join(format!("sail-refactor-oracle-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let script = directory.join("run.isail");
    std::fs::write(&script, "initialize_registers()\n:run\nmain()\n:run\n:quit\n").unwrap();
    for (index, (definition, invocation, expected, assist)) in [
        ("function f(x : int) -> int = x + x\n", "f(tick())", 12, "inline_call"),
        (
            "function f(x : int, y : int) -> int = 10 * x + y\n",
            "f(tick(), tick())",
            32,
            "inline_call",
        ),
        (
            "val f : forall ('n : Int). int('n) -> int('n)\nfunction f(x) = { let y : int('n) = x; y }\n",
            "f(7)", 7, "inline_call",
        ),
        (
            "val f : forall ('a : Type). 'a -> 'a\nfunction f(x) = { let y : 'a = x; y }\n",
            "f(7)", 7, "inline_call",
        ),
        (
            "val f : forall ('n : Int). (implicit('n), int('n)) -> int('n)\nfunction f(n, x) = x\n",
            "f(7)", 7, "inline_call",
        ),
        (
            "val f : forall ('n : Int). (implicit('n), int('n), int) -> int('n)\nfunction f(n, x, y) = x\n",
            "f(7, 1)", 7, "remove_unused_param",
        ),
        (
            "val f : (int, int) -> int\nfunction f(x, y) = x\n",
            "f(tick(), tick())",
            21,
            "remove_unused_param",
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let source = format!("register counter : int = 0\nfunction tick() -> int = {{ counter = counter + 1; counter }}\n{definition}function main() -> int = {{ counter = 0; let value = {invocation}; 10 * counter + value }}\n");
        let (host, map, urls) = setup(&[("main.sail", &source)], &[0], vec![]);
        let analysis = snapshot(&host, &map);
        let offset = if assist == "inline_call" {
            source.rfind(invocation).unwrap()
        } else {
            source.find("y)").unwrap()
        };
        let action = analysis
            .assists(&urls[0], base_db::text_range(offset, offset), &Default::default())
            .unwrap()
            .into_iter()
            .find(|a| a.id.0 == assist)
            .expect("oracle fixture must offer assist");
        let mut after = source.clone();
        let mut edits =
            action.source_change.unwrap().source_file_edits.remove(&FileId::from_raw(0)).unwrap();
        edits.sort_by_key(|edit| edit.range.start());
        for edit in edits.iter().rev() {
            after.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        for (label, text) in [("before", source), ("after", after)] {
            let path = directory.join(format!("case{index}-{label}.sail"));
            std::fs::write(&path, format!("default Order dec\n$include <prelude.sail>\n{text}"))
                .unwrap();
            let result = Command::new(&compiler)
                .env("SAIL_DIR", &library)
                .current_dir(&directory)
                .arg("--is")
                .arg(&script)
                .arg(&path)
                .output()
                .unwrap();
            let output = String::from_utf8_lossy(&result.stdout);
            assert!(
                result.status.success()
                    && output.lines().any(|line| line == format!("Result = {expected}")),
                "case {index} {label}: {}\n{output}\n{}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

fn rename_result(
    source: &str,
    needle: &str,
    new_name: &str,
) -> Result<Option<SourceChange>, String> {
    let (host, map, urls) = setup(&[("main.sail", source)], &[0], vec![]);
    let analysis = snapshot(&host, &map);
    let file = analysis.file(&urls[0]).unwrap();
    analysis.rename(&urls[0], file.position_at(source.find(needle).unwrap()), new_name).unwrap()
}

#[test]
#[ignore = "requires SAIL_ORACLE_BIN and SAIL_ORACLE_SRC; executes selected multi-file projects"]
fn upstream_refactoring_execution_preserves_conditional_modules_and_repeated_includes() {
    use std::{path::PathBuf, process::Command};
    let compiler =
        PathBuf::from(std::env::var_os("SAIL_ORACLE_BIN").expect("SAIL_ORACLE_BIN required"));
    let library =
        PathBuf::from(std::env::var_os("SAIL_ORACLE_SRC").expect("SAIL_ORACLE_SRC required"));
    let directory = tempfile::tempdir().unwrap();
    let script = directory.path().join("run.isail");
    std::fs::write(&script, "initialize_registers()\n:run\nmain()\n:run\n:quit\n").unwrap();
    for (repeated, alternate) in [(false, false), (false, true), (true, false), (true, true)] {
        let sources = [
            ("globals.sail", "register counter : int = 0\nfunction tick() -> int = { counter = counter + 1; counter }\n"),
            ("spec.sail", if repeated { "val f : (int, int) -> int\nscattered function f\n" } else { "val f : (int, int) -> int\n" }),
            ("body_a.sail", if repeated { "$include \"shared.sail\"\n" } else { "function f(x, y) = x\n" }),
            ("body_b.sail", if repeated { "$include \"shared.sail\"\nend f\n" } else { "function f(x, y) = x + 2\n" }),
            ("main.sail", "function main() -> int = { counter = 0; let value = f(tick(), tick()); 100 * counter + value }\n"),
            ("shared.sail", "function clause f(x, y) = x\n"),
        ];
        let roots = if repeated {
            vec![0, 1, 2, 3, 4]
        } else if alternate {
            vec![0, 1, 3, 4]
        } else {
            vec![0, 1, 2, 4]
        };
        let (mut host, map, urls) = setup(&sources, &roots, vec![]);
        let analysis = snapshot(&host, &map);
        let implementation = if repeated {
            5
        } else if alternate {
            3
        } else {
            2
        };
        let change = remove(&analysis, &urls[implementation], "y)");
        assert_eq!(
            change.source_file_edits.len(),
            3,
            "one physical edit per signature, implementation and call"
        );
        drop(analysis);
        let updated = apply(&mut host, &map, change);
        let main = &updated[&FileId::from_raw(4)];
        assert_eq!(
            main.matches("tick()").count(),
            2,
            "both effectful arguments must still execute once"
        );
        let manifest = if repeated {
            "variable ALT = false\nCommon { files globals.sail, spec.sail }\nFirst { requires Common files body_a.sail }\nSecond { requires Common, First files body_b.sail }\nMain { requires Common, First, Second files main.sail }\n"
        } else {
            "variable ALT = false\nCommon { files globals.sail, spec.sail }\nBody { requires Common files if $ALT then body_b.sail else body_a.sail }\nMain { requires Common, Body files main.sail }\n"
        };
        let expected = if !repeated && alternate { 203 } else { 201 };
        for (label, edited) in [("before", false), ("after", true)] {
            let project = directory.path().join(format!("{repeated}-{alternate}-{label}"));
            std::fs::create_dir_all(&project).unwrap();
            std::fs::write(project.join("sail.proj"), manifest).unwrap();
            for (index, (name, original)) in sources.iter().enumerate() {
                let text = if edited {
                    updated.get(&FileId::from_raw(index as u32)).map_or(*original, String::as_str)
                } else {
                    *original
                };
                let text = if index == 0 {
                    format!("default Order dec\n$include <prelude.sail>\n{text}")
                } else {
                    text.to_owned()
                };
                std::fs::write(project.join(name), text).unwrap();
            }
            let result = Command::new(&compiler)
                .env("SAIL_DIR", &library)
                .current_dir(&project)
                .args(["--project", "sail.proj", "--variable", &format!("ALT={alternate}"), "--is"])
                .arg(&script)
                .arg("Main")
                .output()
                .unwrap();
            let stdout = String::from_utf8_lossy(&result.stdout);
            assert!(
                result.status.success()
                    && stdout.lines().any(|line| line == format!("Result = {expected}")),
                "repeated={repeated} alternate={alternate} {label}: {}\n{stdout}\n{}",
                result.status,
                String::from_utf8_lossy(&result.stderr)
            );
        }
    }
}

#[test]
fn rename_rejects_capture_declaration_collisions_and_discarding_used_bindings() {
    for (source, needle, name) in [
        ("function f(x : int) -> int = { let y = 1; x + y }\n", "x :", "y"),
        ("function f(x : int, y : int) -> int = x + y\n", "x :", "y"),
        ("function g() -> int = 1\nfunction f() -> int = 2\n", "f()", "g"),
        ("function f(x : int) -> int = x\n", "x :", "_"),
    ] {
        assert!(rename_result(source, needle, name).is_err(), "{source}: {needle} -> {name}");
    }
}

#[test]
fn rename_allows_identical_names_in_disjoint_local_scopes() {
    let source = "function f(x : int) -> int = x\nfunction g(y : int) -> int = y\n";
    let change = rename_result(source, "x :", "y").unwrap().unwrap();
    assert_eq!(change.source_file_edits[&FileId::from_raw(0)].len(), 2);
}

#[test]
fn rename_type_variables_preserves_quantifier_identity_and_rejects_capture() {
    let source = "val f : forall ('n : Int). int('n) -> int('n)\nval g : forall ('n : Int). int('n) -> int('n)\n";
    let change = rename_result(source, "'n :", "'m").unwrap().unwrap();
    let edits = &change.source_file_edits[&FileId::from_raw(0)];
    assert_eq!(edits.len(), 3, "other function's quantifier must not be renamed: {edits:?}");
    assert!(edits
        .iter()
        .all(|edit| base_db::range_start(edit.range) < source.find("val g").unwrap()));
    let source = "val f : forall ('n : Int) ('m : Int). (int('n), int('m)) -> int\n";
    assert!(rename_result(source, "'n :", "'m").is_err());
}

#[test]
fn rename_type_variable_reaches_the_implementation_without_touching_inner_binders() {
    let sources = [
        ("spec.sail", "val f : forall ('n : Int). int('n) -> int('n)\n"),
        ("body.sail", "function f(x : int('n)) -> int('n) = x\n"),
    ];
    let (mut host, map, urls) = setup(&sources, &[0, 1], vec![]);
    let analysis = snapshot(&host, &map);
    let file = analysis.file(&urls[0]).unwrap();
    let change = analysis
        .rename(&urls[0], file.position_at(sources[0].1.find("'n :").unwrap()), "'m")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(change.source_file_edits.len(), 2);
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    assert!(updated[&FileId::from_raw(1)].contains("int('m)"));
}

#[test]
fn signature_change_declines_indirect_alias_and_infix_uses_without_leaving_stale_callers() {
    for uses in [
        "function caller() -> int = { let g = choose; g(1, 2) }\n",
        "infix 6 +++\noverload operator +++ = { f }\nfunction caller() -> int = 1 +++ 2\n",
    ] {
        let defs = "val f : (int, int) -> int\nfunction f(x, y) = x\noverload choose = { f }\n";
        let (host, map, urls) = setup(&[("defs.sail", defs), ("uses.sail", uses)], &[0, 1], vec![]);
        let analysis = snapshot(&host, &map);
        let offset = defs.find("y)").unwrap();
        let actions = analysis
            .assists(&urls[0], base_db::text_range(offset, offset), &Default::default())
            .unwrap();
        assert!(!actions.iter().any(|action| action.id.0 == "remove_unused_param"), "{uses}");
    }
}

#[test]
fn inlining_supplies_proven_implicit_values_and_keeps_quantified_annotations_in_scope() {
    let source = "val f : forall ('n : Int). (implicit('n), int('n)) -> int('n)\nfunction f(n, x) = x\nfunction caller() -> int = f(7)\n";
    let (mut host, map, urls) = setup(&[("main.sail", source)], &[0], vec![]);
    let analysis = snapshot(&host, &map);
    let offset = source.rfind("f(7)").unwrap();
    let action = analysis
        .assists(&urls[0], base_db::text_range(offset, offset), &Default::default())
        .unwrap()
        .into_iter()
        .find(|action| action.id.0 == "inline_call")
        .expect("proven implicit application can inline");
    drop(analysis);
    let updated = apply(&mut host, &map, action.source_change.unwrap());
    let caller = updated[&FileId::from_raw(0)].split("function caller").nth(1).unwrap();
    assert!(!caller.contains("'n"), "{caller}");
    assert!(caller.contains(" = 7;"), "{caller}");
}

#[test]
fn parameter_removal_updates_all_callers_of_equivalent_shared_include_declarations() {
    use base_db::module_graph::ModuleData;
    let sources = [
        ("a.sail", "$include \"shared.sail\"\nfunction a() -> int = f(1, 2)\n"),
        ("b.sail", "$include \"shared.sail\"\nfunction b() -> int = f(3, 4)\n"),
        ("shared.sail", "val f : (int, int) -> int\nfunction f(x, y) = x\n"),
    ];
    let (mut host, map, urls) = setup(
        &sources,
        &[0, 1],
        vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
        ],
    );
    let input = base_db::preprocess::WorkspaceSourceProject::try_get(host.raw_database()).unwrap();
    let mut project = input.project(host.raw_database()).clone();
    project.module_options = [
        (
            0,
            base_db::preprocess::ModuleOptions {
                project: Some("/project/a.sail_project".into()),
                ..Default::default()
            },
        ),
        (
            1,
            base_db::preprocess::ModuleOptions {
                project: Some("/project/b.sail_project".into()),
                ..Default::default()
            },
        ),
    ]
    .into();
    let mut change = ide_db::apply_change::Change::new();
    change.set_source_project(project);
    host.apply_change(change);
    let analysis = snapshot(&host, &map);
    for id in [FileId::from_raw(0), FileId::from_raw(1), FileId::from_raw(2)] {
        let diagnostics = analysis.file_diagnostics(id, &Default::default()).unwrap();
        assert!(
            diagnostics
                .iter()
                .all(|diagnostic| diagnostic.severity != ide_diagnostics::Severity::Error),
            "initial project must be valid: {diagnostics:?}"
        );
    }
    let change = remove(&analysis, &urls[2], "y)");
    assert_eq!(change.source_file_edits.len(), 3);
    drop(analysis);
    let updated = apply(&mut host, &map, change);
    assert!(updated[&FileId::from_raw(0)].contains("f(__sail_argument0)"));
    assert!(updated[&FileId::from_raw(1)].contains("f(__sail_argument0)"));
    assert!(updated[&FileId::from_raw(2)].contains("val f : int -> int"));
}

#[test]
fn rename_preserves_automatic_conversions_and_rejects_unreferenced_generated_name_collisions() {
    for source in [
        "val num_of_New : int -> int\nenum Old = { First, Second }\n",
        "enum Old = { First, Second }\nval num_of_New : int -> int\n",
        "bitfield Old : bits(8) = { HI : 7 .. 4 }\nval _get_New_HI : int -> int\n",
    ] {
        assert!(
            rename_result(source, "Old", "New").is_err(),
            "generated collision was accepted: {source}"
        );
    }
    assert!(rename_result("enum Old = { First, Second }\n", "Old", "New").unwrap().is_some());
    assert!(rename_result("function f(x : int) -> int = x\n", "int", "Other").is_err());
}
