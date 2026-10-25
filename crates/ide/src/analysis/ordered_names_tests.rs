//! Ordinary IDE requests must select the same declaration as inference.
use super::hierarchy::tests::{setup, snapshot};
use super::*;

#[test]
fn ordered_mapping_concat_uses_inferred_payload_types() {
    let bits = "default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\n";
    for (definitions, mapping, accepted) in [
        ("enum E = { A, B }\nunion U = { Mk : E }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", false),
        ("struct E = { field : int }\nunion U = { Mk : E }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", false),
        ("union E = { Inner : unit }\nunion U = { Mk : E }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", false),
        ("struct E('a : Type) = { field : 'a }\nunion U = { Mk : E(int) }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", false),
        ("union U = { Mk : bits(1) }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", true),
        ("type Alias = bits(1)\nunion U = { Mk : Alias }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", true),
        ("enum E = { A, B }\ntype Alias = E\nunion U = { Mk : Alias }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ x }\n", false),
        ("enum E = { A, B }\nunion U = { Mk : E }\nmapping enc : E <-> bits(1) = { A <-> 0b0, B <-> 0b1 }\n", "mapping m : U <-> bits(2) = { Mk(x) <-> 0b0 @ enc(x) }\n", true),
    ] {
        let definitions = format!("{bits}{definitions}");
      for reverse in [false, true] {
        let mapping = if reverse {
            let operand = if mapping.contains("enc(x)") { "enc(x)" } else { "x" };
            format!("mapping m : bits(2) <-> U = {{ 0b0 @ {operand} <-> Mk(x) }}\n")
        } else { mapping.to_owned() };
        for split in [false, true] {
            let combined = format!("{definitions}{mapping}");
            let (host, map, _) = if split { setup(&[("defs.sail", &definitions), ("main.sail", &mapping)], &[0, 1], vec![]) } else { setup(&[("main.sail", &combined)], &[0], vec![]) };
            let diagnostics = snapshot(&host, &map).file_diagnostics(FileId::from_raw(if split {1} else {0}), &Default::default()).unwrap();
            assert_eq!(!diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), accepted, "split={split}\n{combined}{diagnostics:?}");
        }
      }
    }
}

#[test]
fn ordered_recursive_types_are_rejected_in_their_declaration_environment() {
    for source in [
        "struct S = { field : S }\n",
        "struct S = { field : list(S) }\n",
        "private struct S = { field : S }\n",
        "union U = { Mk : U }\n",
        "type T = T\n",
        "type T = list(T)\n",
        "scattered union U\nunion clause U = Mk : U\nend U\n",
        "struct A = { field : B }\nstruct B = { field : A }\n",
    ] {
        let (host, map, _) = setup(&[("main.sail", source)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{source}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_type_references_keep_cross_file_origins_and_ignore_inactive_types() {
    let first = "struct A = { field : list(B) }\n";
    let second = "struct B = { field : int }\n";
    for future in [false, true] {
        let order: &[usize] = if future { &[0, 1] } else { &[1, 0] };
        let (host, map, _) = setup(&[("a.sail", first), ("b.sail", second)], order, vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            future,
            "{diagnostics:?}"
        );
        if future {
            assert!(
                diagnostics.iter().any(|d| d
                    .related
                    .iter()
                    .any(|(at, _)| at.file_id == FileId::from_raw(1)
                        && base_db::range_start(at.range) == second.find('B').unwrap())),
                "{diagnostics:?}"
            );
        }
    }
    let source = "$ifdef NEVER\nstruct S = { field : S }\n$endif\nval use : int -> int\nfunction use(x) = x\n";
    let (host, map, _) = setup(&[("main.sail", source)], &[0], vec![]);
    let diagnostics =
        snapshot(&host, &map).file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_implicit_arguments_use_call_site_witnesses() {
    let declaration = "default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\nstruct Box('n : Int), 'n >= 0 = { value : bits('n) }\nval opaque : forall 'n, 'n >= 0. (implicit('n), Box('n)) -> int\nval bit_size : forall 'n, 'n >= 0. bits('n) -> int('n)\noverload __size = {bit_size}\n";
    for (source, accepted) in [
        ("function use(x : Box(8)) -> int = opaque(x)\n", true),
        ("function use(x : Box(8)) -> int = opaque(8, x)\n", true),
        ("val use : forall 'n, 'n >= 0. Box('n) -> int\nfunction use(x) = opaque(x)\n", false),
        ("val use : forall 'n, 'n >= 0. (int('n), Box('n)) -> int\nfunction use(n, x) = opaque(x)\n", true),
        ("val use : forall 'n, 'n >= 0. (bits('n), Box('n)) -> int\nfunction use(b, x) = opaque(x)\n", true),
        ("val use : forall 'n, 'n >= 0. (int('n), Box('n)) -> int\nfunction use(_, x) = opaque(x)\n", false),
        ("val use : forall 'n, 'n == 8. Box('n) -> int\nfunction use(x) = opaque(x)\n", true),
        ("val use : forall 'n, 'n >= 8 & 'n <= 8. Box('n) -> int\nfunction use(x) = opaque(x)\n", true),
        ("val use : forall 'n 'm, 'n >= 0 & 'm == 'n. (int('m), Box('n)) -> int\nfunction use(m, x) = opaque(x)\n", true),
        ("val use : forall 'n 'm, 'n >= 0 & 'm == 'n. (bits('m), Box('n)) -> int\nfunction use(m, x) = opaque(x)\n", false),
        ("val use : forall 'n, 'n >= 0. Box('n) -> int\nfunction use(x) = { let w = x.value; opaque(x) }\n", true),
        ("val use : forall 'n, 'n >= 0. Box('n) -> int\nfunction use(x) = { let result = opaque(x); let w = x.value; result }\n", false),
    ] {
      for overloaded in [false, true] {
        let source = if overloaded {
            format!("overload invoke = {{opaque}}\n{}", source.replace("opaque(", "invoke("))
        } else { source.to_owned() };
        for split in [false, true] {
            let combined = format!("{declaration}{source}");
            let (host, map, _) = if split {
                setup(&[("defs.sail", declaration), ("main.sail", &source)], &[0, 1], vec![])
            } else {
                setup(&[("main.sail", &combined)], &[0], vec![])
            };
            let diagnostics = snapshot(&host, &map).file_diagnostics(FileId::from_raw(if split { 1 } else { 0 }), &Default::default()).unwrap();
            assert_eq!(!diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), accepted, "split={split}\n{source}{diagnostics:?}");
            if !accepted {
                assert!(diagnostics.iter().any(|d| d.code.as_str() == "unrecoverable-implicit"), "{diagnostics:?}");
            }
        }
      }
    }
}

#[test]
fn ordered_implicit_checks_follow_signature_edits_and_ignore_later_replacements() {
    let implicit = "val opaque : forall 'n, 'n >= 0. (implicit('n), Box('n)) -> int\n";
    let ordinary = "val opaque : forall 'n, 'n >= 0. Box('n) -> int\n";
    let types = "default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\nstruct Box('n : Int), 'n >= 0 = { value : bits('n) }\n";
    let caller = "val use : forall 'n, 'n >= 0. Box('n) -> int\nfunction use(x) = opaque(x)\n";
    let initial = format!("{types}{implicit}");
    let (mut host, map, _) = setup(
        &[("defs.sail", &initial), ("main.sail", caller), ("later.sail", ordinary)],
        &[0, 1, 2],
        vec![],
    );
    for (selected, rejected) in [(implicit, true), (ordinary, false), (implicit, true)] {
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), format!("{types}{selected}"));
        host.apply_change(change);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| d.code.as_str() == "unrecoverable-implicit"),
            rejected,
            "{diagnostics:?}"
        );
        assert!(
            diagnostics
                .iter()
                .filter(|d| d.severity == ide_diagnostics::Severity::Error)
                .all(|d| d.code.as_str() == "unrecoverable-implicit"),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_type_instantiations_check_declared_constraints_instead_of_width_names() {
    for (declaration, size, accepted) in [
        ("type U('n : Int) = bitvector('n - 2, dec)\n", 1, true),
        ("union U('n : Int) = { Mk : bitvector('n - 2, dec) }\n", 1, true),
        ("union U('n : Int) = { Mk : bitvector('n, dec) }\n", 1, true),
        ("struct U('n : Int) = { value : bitvector('n, dec) }\n", 1, true),
        ("union U('n : Int), 'n >= 2 = { Mk : bitvector('n - 2, dec) }\n", 1, false),
        ("union U('n : Int), 'n >= 2 = { Mk : bitvector('n - 2, dec) }\n", 3, true),
        ("type U('n : Int), 'n >= 2 = bitvector('n - 2, dec)\n", 1, false),
        ("type U('n : Int), 'n >= 2 = bitvector('n - 2, dec)\n", 3, true),
        ("type U('n : Int), /* = a comment */ 'n >= 2 = bitvector('n - 2, dec)\n", 1, false),
    ] {
        let declaration = format!("default Order dec\n{declaration}");
        let use_site = format!("val consume : U({size}) -> unit\n");
        for split in [false, true] {
            let combined = format!("{declaration}{use_site}");
            let (host, map, _) = if split {
                setup(&[("defs.sail", &declaration), ("main.sail", &use_site)], &[0, 1], vec![])
            } else {
                setup(&[("main.sail", &combined)], &[0], vec![])
            };
            let diagnostics = snapshot(&host, &map)
                .file_diagnostics(FileId::from_raw(if split { 1 } else { 0 }), &Default::default())
                .unwrap();
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                accepted,
                "split={split}\n{combined}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_type_constraints_preserve_alias_binders_and_private_support() {
    let bits = "default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\n";
    for (source, accepted) in [
        ("default Order dec\nval consume : vector(-1, dec, int) -> unit\n".to_owned(), true),
        ("default Order dec\ntype bits('n : Int) = bitvector('n, dec)\nval consume : bits(-1) -> unit\n".to_owned(), true),
        (format!("{bits}val consume : bits(-1) -> unit\n"), false),
        (format!("{bits}type Pack('a : Type) = {{'n, 'n == 1. ('a, bits('n))}}\nval use : forall 'n, 'n == 4. Pack(bits('n)) -> unit\n"), true),
        (format!("{bits}type Pack('a : Type) = {{'n, 'n == -1. ('a, bits('n))}}\n"), false),
        (format!("{bits}type Shadow('n : Int), 'n >= 0 = {{'n, 'n == -1. bits('n)}}\n"), false),
        (format!("{bits}val use : forall 'n, 'n >= 0. {{'n, 'n == -1. bits('n)}} -> unit\n"), false),
    ] {
        let (host, map, _) = setup(&[("main.sail", &source)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map).file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        assert_eq!(!diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), accepted, "{source}{diagnostics:?}");
    }
    let declaration = "default Order dec\nprivate type Small('n : Int), 'n >= 2 = bitvector('n - 2, dec)\ntype Public('n : Int), 'n >= 2 = Small('n)\n";
    for size in [1, 3] {
        let source = format!("val consume : Public({size}) -> unit\n");
        let modules = vec![
            base_db::module_graph::ModuleData {
                files: vec![FileId::from_raw(0)],
                requires: vec![],
            },
            base_db::module_graph::ModuleData {
                files: vec![FileId::from_raw(1)],
                requires: vec![0],
            },
        ];
        let (host, map, _) =
            setup(&[("defs.sail", declaration), ("main.sail", &source)], &[0, 1], modules);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            size == 3,
            "{source}{diagnostics:?}"
        );
        if size == 1 {
            assert!(
                diagnostics.iter().any(|d| d
                    .related
                    .iter()
                    .any(|(at, _)| at.file_id == FileId::from_raw(0)
                        && base_db::range_start(at.range) == declaration.find("Public").unwrap())),
                "{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_type_constraints_refresh_after_provider_edits() {
    let use_site = "val consume : U(1) -> unit\n";
    let (mut host, map, _) = setup(&[("defs.sail", ""), ("main.sail", use_site)], &[0, 1], vec![]);
    for (prefix, bound) in [("", 2), ("", 1), ("// moved declaration\n", 2)] {
        let definition = format!(
            "{prefix}default Order dec\ntype U('n : Int), 'n >= {bound} = bitvector('n, dec)\n"
        );
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), definition.clone());
        host.apply_change(change);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        let errors: Vec<_> =
            diagnostics.iter().filter(|d| d.severity == ide_diagnostics::Severity::Error).collect();
        assert_eq!(errors.is_empty(), bound == 1, "{diagnostics:?}");
        if bound == 2 {
            assert!(
                errors.iter().any(|d| d
                    .related
                    .iter()
                    .any(|(at, _)| at.file_id == FileId::from_raw(0)
                        && base_db::range_start(at.range) == definition.find("U(").unwrap())),
                "{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_implicit_locals_have_one_identity_for_declaration_writes_and_reads() {
    for text in [
        "function run() -> int = { x = 1; x = 1; x }\n",
        "function run() -> int = { x : int = 1; x = 2; x }\n",
        "function run() -> int = { (x, y) = (1, 2); x = 1; x }\n",
    ] {
        let (mut host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[0]).unwrap();
        let binding = text.find('x').unwrap();
        for (offset, _) in text.match_indices('x') {
            let targets = a.goto_definition(&urls[0], file.position_at(offset)).unwrap();
            assert_eq!(targets.len(), 1, "{text} at {offset}: {targets:?}");
            assert_eq!(base_db::range_start(targets[0].focus_range), binding);
            let hover = a
                .hover(FileId::from_raw(0), file.position_at(offset), &Default::default())
                .unwrap()
                .unwrap();
            assert!(
                hover.markup.contains("int") || hover.markup.contains("atom"),
                "{}",
                hover.markup
            );
        }
        let edit = a
            .rename(&urls[0], file.position_at(text.rfind('x').unwrap()), "renamed")
            .unwrap()
            .unwrap()
            .unwrap();
        let mut updated = text.to_owned();
        let mut edits = edit.source_file_edits[&FileId::from_raw(0)].clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
        for edit in edits {
            updated.replace_range(
                usize::from(edit.range.start())..usize::from(edit.range.end()),
                &edit.new_text,
            );
        }
        assert_eq!(updated, text.replace('x', "renamed"));
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), updated);
        host.apply_change(change);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_implicit_binding_selection_rechecks_global_visibility_after_edits() {
    let body = "function run() -> int = { x : int = 1; x = 2; x }\n";
    let (mut host, map, urls) =
        setup(&[("defs.sail", "register x : int\n"), ("main.sail", body)], &[0, 1], vec![]);
    for (defs, local) in
        [("register x : int\n", false), ("type x = int\n", true), ("register x : int\n", false)]
    {
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), defs);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        for (offset, _) in body.match_indices('x') {
            let targets = a.goto_definition(&urls[1], file.position_at(offset)).unwrap();
            assert_eq!(targets.len(), 1, "{defs} {targets:?}");
            assert_eq!(targets[0].file_id, Some(FileId::from_raw(if local { 1 } else { 0 })));
            assert_eq!(
                base_db::range_start(targets[0].focus_range),
                if local { body.find('x').unwrap() } else { defs.find('x').unwrap() }
            );
        }
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_implicit_locals_are_unavailable_in_initializers_and_sibling_scopes() {
    for (text, needle) in [
        ("function run() -> int = { x : int = x; x }\n", "= x;"),
        ("function run() -> int = { { x : int = 1; () }; x }\n", "; x }"),
        ("function run(c : bool) -> int = { if c then x = 1 else (); x }\n", "; x }"),
        ("function run() -> int = { let v = (x = 1); x }\n", "; x }"),
    ] {
        let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[0]).unwrap();
        let offset = text.find(needle).unwrap() + 2;
        assert!(
            a.goto_definition(&urls[0], file.position_at(offset)).unwrap().is_empty(),
            "{text}"
        );
        let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{text}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_implicit_declarations_do_not_bypass_private_globals() {
    for declaration in ["private register x : int\n", "private let x : int = 1\n"] {
        let body = "function run() -> int = { x : int = 1; x }\n";
        let modules = vec![
            base_db::module_graph::ModuleData {
                files: vec![FileId::from_raw(0)],
                requires: vec![],
            },
            base_db::module_graph::ModuleData {
                files: vec![FileId::from_raw(1)],
                requires: vec![0],
            },
        ];
        let (host, map, urls) =
            setup(&[("defs.sail", declaration), ("main.sail", body)], &[0, 1], modules);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        for (offset, _) in body.match_indices('x') {
            assert!(
                a.goto_definition(&urls[1], file.position_at(offset)).unwrap().is_empty(),
                "{declaration}"
            );
        }
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_explicit_local_bindings_check_hidden_global_names() {
    for declaration in ["private register x : int\n", "private let x : int = 1\n"] {
        for body in [
            "function run() -> int = { let x = 1; x }\n",
            "function run() -> int = { var x : int = 1; x }\n",
            "function run() -> int = var x : int = 1 in x\n",
            "function run(x : int) -> int = x\n",
        ] {
            let modules = vec![
                base_db::module_graph::ModuleData {
                    files: vec![FileId::from_raw(0)],
                    requires: vec![],
                },
                base_db::module_graph::ModuleData {
                    files: vec![FileId::from_raw(1)],
                    requires: vec![0],
                },
            ];
            let (host, map, _) =
                setup(&[("defs.sail", declaration), ("main.sail", body)], &[0, 1], modules);
            let diagnostics = snapshot(&host, &map)
                .file_diagnostics(FileId::from_raw(1), &Default::default())
                .unwrap();
            assert!(
                diagnostics.iter().any(|d| {
                    d.severity == ide_diagnostics::Severity::Error
                        && base_db::range_start(d.range.range) == body.find('x').unwrap()
                        && d.related.iter().any(|(range, _)| {
                            range.file_id == FileId::from_raw(0)
                                && base_db::range_start(range.range)
                                    == declaration.find('x').unwrap()
                        })
                }),
                "{declaration}{body}{diagnostics:?}"
            );
        }
    }
    for text in [
        "function run() -> int = { let x = 1; x }\nprivate register x : int\n",
        "private type x = int\nfunction run() -> int = var x : int = 1 in x\n",
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{text}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_local_var_expressions_retain_binding_identity_and_type() {
    let text = "function run() -> int = var x : int = 1 in { x = 2; x }\n";
    let (mut host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let binding = text.find("x : int").unwrap();
    for at in [text.find("x = 2").unwrap(), text.rfind("x }").unwrap()] {
        let targets = a.goto_definition(&urls[0], file.position_at(at)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(base_db::range_start(targets[0].focus_range), binding);
    }
    let edit = a
        .rename(&urls[0], file.position_at(text.rfind("x }").unwrap()), "renamed")
        .unwrap()
        .expect("local binding rename")
        .unwrap();
    let mut updated = text.to_owned();
    let mut edits = edit.source_file_edits[&FileId::from_raw(0)].clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
    for edit in edits {
        updated.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    assert_eq!(updated, text.replace("x", "renamed"));
    drop(a);
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), updated);
    host.apply_change(change);
    let diagnostics =
        snapshot(&host, &map).file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_local_blocks_do_not_leak_into_siblings_or_outer_uses() {
    let text =
        "function run(c : bool, x : int) -> int = { let result = if c then { let x = 2; x } else x; x }\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (at, binding) in [
        (text.find("x } else").unwrap(), text.find("x = 2").unwrap()),
        (text.find("else x").unwrap() + 5, text.find("x : int").unwrap()),
        (text.rfind("x }").unwrap(), text.find("x : int").unwrap()),
    ] {
        let targets = a.goto_definition(&urls[0], file.position_at(at)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(base_db::range_start(targets[0].focus_range), binding, "at {at}");
    }
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_local_assignment_checks_use_the_binding_at_the_write() {
    for (text, expected) in [
        ("function run() -> unit = { x = 1; { let x = 2; () }; () }\n", None),
        ("function run(c : bool) -> unit = { if c then { var x = 1; () } else { let x = 2; x = 3; () }; () }\n", Some("assign-to-immutable")),
        ("function run() -> unit = { var x = 1; { let x = 2; x = 3; () }; () }\n", Some("assign-to-immutable")),
        ("struct S = { f : int }\nfunction run() -> unit = { x.f = 1; var x : S = struct { f = 2 }; () }\n", Some("assign-to-undeclared")),
        ("struct S = { f : int }\nfunction run(c : bool) -> unit = { if c then { var x : S = struct { f = 2 }; () } else { x.f = 1; () }; () }\n", Some("assign-to-undeclared")),
        ("struct S = { f : int }\nfunction run() -> unit = { x : S = struct { f = 2 }; x.f = 1; () }\n", None),
        ("struct S = { f : int }\nfunction run() -> unit = { x.f = 1; x : S = struct { f = 2 }; () }\n", Some("assign-to-undeclared")),
        ("struct S = { f : int }\nfunction run() -> unit = { { x : S = struct { f = 2 }; () }; x.f = 1; () }\n", Some("assign-to-undeclared")),
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        match expected {
            Some(code) => assert!(diagnostics.iter().any(|d| matches!(d.code,
                hir::diagnostics::DiagnosticCode::SailError(c) if c == code)), "{text}{diagnostics:?}"),
            None => assert!(!diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), "{text}{diagnostics:?}"),
        }
    }
}

#[test]
fn ordered_local_var_annotations_and_repeated_immutable_writes_are_checked() {
    for text in [
        "function run() -> int = var x : int = true in x\n",
        "function run(x : int) -> int = var x : int = 1 in x\n",
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
    for text in [
        "let x : int = 1\nfunction run() -> unit = { x = 2; x = 3; () }\n",
        "function run() -> unit = { let x = 1; let y = 2; x = 2; x = 3; () }\n",
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        let writes: Vec<_> = diagnostics
            .iter()
            .filter(|d| {
                matches!(d.code, hir::diagnostics::DiagnosticCode::SailError("assign-to-immutable"))
            })
            .collect();
        assert_eq!(writes.len(), 2, "{text}{diagnostics:?}");
    }
}

#[test]
fn ordered_test_attributes_use_only_the_preceding_prototype() {
    for (decl, body, error) in [
        ("", "$[test]\nfunction run() -> unit = ()\nval run : unit -> unit\n", false),
        ("\n\nval run : unit -> unit\n", "$[test]\nfunction run() = ()\n", true),
        ("$[test]\nval run : unit -> unit\n", "function run() = ()\n", false),
    ] {
        let (host, map, _) = setup(&[("defs.sail", decl), ("main.sail", body)], &[0, 1], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        let found = diagnostics.iter().find(|d| {
            matches!(d.code, hir::diagnostics::DiagnosticCode::SailError("invalid-test-signature"))
        });
        assert_eq!(found.is_some(), error, "{body}{diagnostics:?}");
        if let Some(d) = found {
            assert_eq!(d.related.len(), 1, "{d:?}");
            assert_eq!(d.related[0].0.file_id, FileId::from_raw(0));
            assert_eq!(usize::from(d.related[0].0.range.start()), decl.find("run").unwrap());
        }
    }
}

#[test]
fn ordered_outcome_checks_use_cross_file_preceding_implementations() {
    let outcome = "outcome O : 'a -> unit with 'a : Type = { val convert : 'a -> string }\n";
    for (implementation, replacement, error) in [
        ("unit -> string", "val convert_impl : unit -> unit\n", false),
        ("unit -> unit", "val convert_impl : unit -> string\n", true),
    ] {
        let defs = format!("{outcome}val convert_impl : {implementation}\n");
        let body = format!("instantiation O with 'a = unit, convert = convert_impl\n{replacement}");
        let (host, map, _) = setup(&[("defs.sail", &defs), ("main.sail", &body)], &[0, 1], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailError("instantiation-mismatch")
            )),
            error,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_outcome_constraints_use_the_preceding_implementation() {
    let defs = "default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\noutcome O : forall 'n, 'n > 0. ('a, bits('n)) -> unit with 'a : Type = { val convert : forall 'n, 'n == 64. 'a -> bits('n) }\nval convert_impl : forall 'n, 'n in {32, 48}. bits('n) -> bits(64)\n";
    for width in [32, 33] {
        let body = format!("instantiation O with 'a = bits({width}), convert = convert_impl\nval convert_impl : forall 'n, 'n in {{33, 48}}. bits('n) -> bits(64)\n");
        let (host, map, _) = setup(&[("defs.sail", defs), ("main.sail", &body)], &[0, 1], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailError("instantiation-constraint")
            )),
            width == 33,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_metadata_checks_do_not_use_future_declarations() {
    for (text, code) in [
        ("val one : int -> int\noverload g = {one}\nfunction use(x : int) -> int = g(x)\nval one : (int, int) -> int\n", "no-possible-overload"),
        ("function use(x : int, y : int) -> bool = m(x, y)\nval m : int <-> bool\n", "mapping-arity-mismatch"),
        ("function use() -> unit = { x = 1; () }\nlet x : int = 2\n", "assign-to-immutable"),
        ("val use : vector(size, int) -> unit\ntype size : Bool = true\n", "kind-mismatch"),
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        assert!(!diagnostics.iter().any(|d| matches!(&d.code, hir::diagnostics::DiagnosticCode::SailError(c) if *c == code)), "{text}\n{diagnostics:?}");
    }
}

#[test]
fn ordered_metadata_checks_resolve_cross_file_declarations() {
    for (definition, use_site, code) in [
        ("let x : int = 2\n", "function use() -> unit = { x = 1; () }\n", "assign-to-immutable"),
        (
            "type Box('n : Int) = vector('n, dec, int)\n",
            "val use : Box(bool) -> unit\n",
            "kind-mismatch",
        ),
        (
            "val m : int <-> bool\n",
            "function use(x : int, y : int) -> bool = m(x, y)\n",
            "mapping-arity-mismatch",
        ),
        (
            "val one : int -> int\noverload g = {one}\n",
            "function use(x : int, y : int) -> int = g(x, y)\n",
            "no-possible-overload",
        ),
    ] {
        let (host, map, _) =
            setup(&[("defs.sail", definition), ("main.sail", use_site)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            diagnostics.iter().any(
                |d| matches!(&d.code, hir::diagnostics::DiagnosticCode::SailError(c) if *c == code)
            ),
            "{definition}{use_site}\n{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_overload_arity_keeps_preceding_extensions() {
    let text = "val one : int -> int\nval two : (int, int) -> int\noverload g = {one}\noverload g = {two}\nfunction use(x : int) -> int = g(x)\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| matches!(
            d.code,
            hir::diagnostics::DiagnosticCode::SailError("no-possible-overload")
        )),
        "{diagnostics:?}"
    );
    let file = a.file(&urls[0]).unwrap();
    let help = a
        .signature_help(FileId::from_raw(0), file.position_at(text.rfind("g(x)").unwrap() + 2))
        .unwrap()
        .unwrap();
    assert_eq!(help.signatures.len(), 2, "{help:?}");
}

#[test]
fn ordered_overload_groups_are_global_but_members_keep_their_scope() {
    use base_db::module_graph::ModuleData;
    for (first, extension, argument, accepted) in [
        (
            "val one : int -> int\noverload g = {one}\n",
            "val two : (int, int) -> int\noverload g = {two}\n",
            "x",
            true,
        ),
        ("val one : int -> int\n", "overload g = {one}\n", "x", true),
        (
            "val one : int -> int\noverload g = {one}\n",
            "val two : (int, int) -> int\noverload g = {two}\n",
            "x, x",
            false,
        ),
    ] {
        let use_site = format!("function use(x : int) -> int = g({argument})\n");
        let modules = vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
        ];
        let (host, map, urls) = setup(
            &[("a.sail", first), ("b.sail", extension), ("c.sail", &use_site)],
            &[0, 1, 2],
            modules,
        );
        let a = snapshot(&host, &map);
        let extension_diagnostics =
            a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            !extension_diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{extension_diagnostics:?}"
        );
        let diagnostics = a.file_diagnostics(FileId::from_raw(2), &Default::default()).unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            accepted,
            "{use_site}{diagnostics:?}"
        );
        let file = a.file(&urls[2]).unwrap();
        let help = a
            .signature_help(FileId::from_raw(2), file.position_at(use_site.find("g(").unwrap() + 2))
            .unwrap()
            .unwrap();
        assert_eq!(help.signatures.len(), 1, "{help:?}");
        assert!(help.signatures[0].label.contains("one"), "{help:?}");
        let offset = use_site.find("g(").unwrap();
        let targets = a.goto_definition(&urls[2], file.position_at(offset)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file_id, Some(FileId::from_raw(1)));
        let completions = a
            .completions(FileId::from_raw(2), file.position_at(offset + 1), &Default::default())
            .unwrap();
        assert!(completions.iter().any(|item| item.label == "g"), "{completions:?}");
    }
}

#[test]
fn ordered_private_overload_groups_cannot_leak_or_be_extended_in_modules() {
    use base_db::module_graph::ModuleData;
    for (first, extension, use_site, error_file) in [
        ("val one : int -> int\nprivate overload g = {one}\nfunction use(x : int) -> int = g(x)\n", "", "", 0),
        ("val one : int -> int\noverload g = {one}\n", "private overload g = {one}\n", "function use(x : int) -> int = g(x)\n", 2),
        ("val one : int -> int\nprivate overload g = {one}\n", "overload g = {one}\n", "", 1),
    ] {
        let modules = vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
        ];
        let (host, map, urls) = setup(&[("a.sail", first), ("b.sail", extension), ("c.sail", use_site)], &[0, 1, 2], modules);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(error_file), &Default::default()).unwrap();
        assert!(diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), "{first}{extension}{use_site}{diagnostics:?}");
        if error_file != 1 {
            let text = if error_file == 0 { first } else { use_site };
            let file = a.file(&urls[error_file as usize]).unwrap();
            let help = a.signature_help(FileId::from_raw(error_file), file.position_at(text.rfind("g(x)").unwrap()+2)).unwrap();
            assert!(help.is_none(), "{help:?}");
        }
    }
}

#[test]
fn ordered_arity_preserves_implicit_and_mapping_tuple_conventions() {
    for (defs, use_site, error) in [
        (
            "val one : forall 'n, 'n >= 0. (implicit('n), int('n)) -> int\noverload g = {one}\n",
            "function use(x : nat) -> int = g(x)\n",
            false,
        ),
        (
            "val two : (int, int) -> int\noverload g = {two}\n",
            "function use(x : int) -> int = g((x,x))\n",
            true,
        ),
        ("val m : (int, int) <-> bool\n", "function use(x : int) -> bool = m((x,x))\n", false),
        (
            "type Pair = (int, int)\nval m : Pair <-> bool\n",
            "function use(x : int) -> bool = m(x,x)\n",
            false,
        ),
    ] {
        let (host, map, _) =
            setup(&[("defs.sail", defs), ("main.sail", use_site)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailError(
                    "no-possible-overload" | "mapping-arity-mismatch"
                )
            )),
            error,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_pattern_width_checks_use_the_selected_signature() {
    for body in [
        "function f(v[3..0] @ v[7..4]) = ()\n",
        "function f(x) = match x { _ : bits(7) @ _ : bits(1) => () }\n",
    ] {
        for width in [8, 4] {
            let defs = format!("default Order dec\ntype bits('n : Int), 'n >= 0 = bitvector('n, dec)\nval f : bits({width}) -> unit\n");
            let (host, map, _) =
                setup(&[("defs.sail", &defs), ("main.sail", body)], &[0, 1], vec![]);
            let a = snapshot(&host, &map);
            let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                width != 8,
                "{defs}{body}\n{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_dependent_call_checks_do_not_use_later_signatures() {
    let text = "type width : Int = 8\nval consume : int(8) -> unit\nfunction use() -> unit = consume(sizeof(width))\nval consume : int(16) -> unit\n";
    let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_inference_checks_field_widths_and_sizeof_arguments_across_files() {
    for width in [8, 16] {
        for (defs, use_site) in [
            (format!("default Order dec\ntype W = bitvector(8, dec)\nstruct R = {{ field : W }}\nval produce : unit -> bitvector({width}, dec)\n"), "function use() -> R = struct R { field = produce() }\n"),
            (format!("type width : Int = 8\nval consume : int({width}) -> unit\n"), "function use() -> unit = consume(sizeof(width))\n"),
        ] {
            let (host, map, _) = setup(&[("defs.sail", &defs), ("main.sail", use_site)], &[0, 1], vec![]);
            let a = snapshot(&host, &map);
            let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
            assert_eq!(diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), width != 8, "{defs}{use_site}\n{diagnostics:?}");
        }
    }
}

#[test]
fn ordered_declaration_diagnostics_reject_future_bindings() {
    for (before, after, message) in [
        ("overload group = {target}\n", "val target : int -> int\n", "target is not bound"),
        (
            "instantiation audit with 'n = 32\n",
            "outcome audit : int('n) -> unit with 'n\n",
            "Unknown outcome audit",
        ),
        (
            "type early : Int = late\n",
            "type late : Int = 8\n",
            "Well-formedness check failed for numeric type expression",
        ),
        (
            "val target : vector(late, dec, int) -> unit\n",
            "type late : Int = 8\n",
            "Well-formedness check failed for type",
        ),
        (
            "termination_measure target(n) = n\n",
            "val target : int -> int\nfunction target(n) = n\n",
            "Function target not found",
        ),
        (
            "function target(n) = n\n",
            "val target : int -> int\n",
            "Cannot infer function type for unannotated function",
        ),
    ] {
        for forward in [true, false] {
            let text =
                if forward { format!("{before}{after}") } else { format!("{after}{before}") };
            let (host, map, _) = setup(&[("main.sail", &text)], &[0], vec![]);
            let a = snapshot(&host, &map);
            let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.message.contains(message)),
                forward,
                "{text}\n{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_overload_cannot_introduce_itself_as_a_member() {
    let (host, map, _) = setup(&[("main.sail", "overload group = {group}\n")], &[0], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.message.contains("group is not bound")),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_declaration_checks_interleave_includes() {
    let part =
        "type early : Int = late\noverload group = {target}\ntermination_measure target(n) = n\n";
    for forward in [true, false] {
        let definitions = "type late : Int = 8\nval target : int -> int\nfunction target(n) = n\n";
        let main = if forward {
            format!("$include \"part.sail\"\n{definitions}")
        } else {
            format!("{definitions}$include \"part.sail\"\n")
        };
        let (host, map, _) = setup(&[("main.sail", &main), ("part.sail", part)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        for message in [
            "Well-formedness check failed for numeric type expression",
            "target is not bound",
            "Function target not found",
        ] {
            assert_eq!(
                diagnostics.iter().any(|d| d.message.contains(message)),
                forward,
                "{main}\n{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_declaration_type_checks_follow_cross_file_aliases() {
    for (definition, use_site, expected) in [
        (
            "union U = { Mk : unit }\ntype A = U\n",
            "register R : A\n",
            Some("Must provide a default register value"),
        ),
        ("enum U = { Mk }\ntype A = U\n", "register R : A\n", None),
        (
            "type A = int\n",
            "bitfield B : A = { x : 0 }\n",
            Some("must be a constant-width bitvector"),
        ),
        ("type N : Int = 8\n", "val f : int(N(1)) -> unit\n", Some("N expected 0 arguments")),
        ("type A = int\n", "val f : A <-> int\n", Some("Bidirectional types cannot be the same")),
    ] {
        let (host, map, _) =
            setup(&[("defs.sail", definition), ("main.sail", use_site)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        if let Some(expected) = expected {
            assert!(
                diagnostics.iter().any(|d| d.message.contains(expected)),
                "{definition}{use_site}\n{diagnostics:?}"
            );
        } else {
            assert!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                "{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_declaration_alias_checks_preserve_private_support() {
    use base_db::module_graph::ModuleData;
    let (host, map, _) = setup(
        &[
            ("defs.sail", "private union U = { Mk : unit }\ntype A = U\n"),
            ("main.sail", "register R : A\n"),
        ],
        &[0, 1],
        vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
        ],
    );
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.message.contains("Must provide a default register value")),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_signature_checks_do_not_read_replacements_after_the_body() {
    for text in [
        "val f : int -> int\nfunction f(x : int) = x\nval f : bool -> bool\n",
        "val f : unit -> int\nfunction f() = 1\nval f : unit <-> int\n",
        "val f : unit <-> int\nmapping f : unit <-> int = { () <-> 1 }\nval f : bool <-> int\n",
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        assert!(
            !diagnostics.iter().any(|d| matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailError(
                    "signature-mismatch" | "not-a-function-type" | "mapping-type-mismatch"
                )
            )),
            "{text}\n{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_signature_checks_keep_foreign_related_locations() {
    let declaration = "\n\nval f : unit <-> int\n";
    let body = "mapping f : bool <-> int = { true <-> 1 }\n";
    let (host, map, _) = setup(&[("defs.sail", declaration), ("main.sail", body)], &[0, 1], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
    let diagnostic = diagnostics
        .iter()
        .find(|d| {
            matches!(d.code, hir::diagnostics::DiagnosticCode::SailError("mapping-type-mismatch"))
        })
        .expect("mapping mismatch");
    assert_eq!(diagnostic.related.len(), 1);
    assert_eq!(diagnostic.related[0].0.file_id, FileId::from_raw(0));
    assert_eq!(
        usize::from(diagnostic.related[0].0.range.start()),
        declaration.find("f :").unwrap()
    );
}

#[test]
fn ordered_mapping_bindings_are_not_hidden_by_future_enum_members() {
    for (text, duplicate) in [
        ("mapping m : (int, int) <-> int = { (C, C) <-> 1 }\nenum E = { C }\n", true),
        ("enum E = { C }\nmapping m : (E, E) <-> int = { (C, C) <-> 1 }\n", false),
    ] {
        let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| d.message.contains("Duplicate binding for C")),
            duplicate,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_overload_is_not_rejected_by_a_future_same_name_signature() {
    let text = "val target : int -> int\noverload group = {target}\nval group : bool -> bool\n";
    let (host, map, _) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| matches!(
            d.code,
            hir::diagnostics::DiagnosticCode::SailError("invalid-overload")
        )),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_navigation_selects_each_repeated_signature() {
    let text = "val f : unit -> int\nval a : unit -> int\nfunction a() = f()\nval f : unit -> bool\nval b : unit -> bool\nfunction b() = f()\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (call, definition) in [
        (text.find("f()").unwrap(), text.find("f :").unwrap()),
        (text.rfind("f()").unwrap(), text.rfind("f :").unwrap()),
    ] {
        let targets = a.goto_definition(&urls[0], file.position_at(call)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(base_db::range_start(targets[0].focus_range), definition);
    }
}

#[test]
fn ordered_navigation_does_not_see_future_declarations_or_initializer_self() {
    let text = "val before : unit -> int\nfunction before() = later()\nval later : unit -> int\nlet self : int = self\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for offset in [text.find("later()").unwrap(), text.rfind("self").unwrap()] {
        assert!(a.goto_definition(&urls[0], file.position_at(offset)).unwrap().is_empty());
    }
    let declared = text.find("self :").unwrap();
    let targets = a.goto_definition(&urls[0], file.position_at(declared)).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(base_db::range_start(targets[0].focus_range), declared);
}

#[test]
fn ordered_navigation_interleaves_parent_and_include() {
    let main = "val f : unit -> int\n$include \"part.sail\"\nval f : unit -> bool\nval b : unit -> bool\nfunction b() = f()\n";
    let part = "val a : unit -> int\nfunction a() = f()\n";
    let (host, map, urls) = setup(&[("main.sail", main), ("part.sail", part)], &[0], vec![]);
    let a = snapshot(&host, &map);
    for (i, text, expected) in
        [(1, part, main.find("f :").unwrap()), (0, main, main.rfind("f :").unwrap())]
    {
        let file = a.file(&urls[i]).unwrap();
        let targets =
            a.goto_definition(&urls[i], file.position_at(text.find("f()").unwrap())).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].url, urls[0]);
        assert_eq!(base_db::range_start(targets[0].focus_range), expected);
    }
}

#[test]
fn ordered_rename_does_not_cross_signature_replacement() {
    let text = "val f : unit -> int\nfunction f() = 1\nval a : unit -> int\nfunction a() = f()\nval f : unit -> bool\nfunction f() = true\nval b : unit -> bool\nfunction b() = f()\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let split = text.rfind("val f").unwrap();
    for (needle, lower, upper) in
        [("f : unit -> int", 0, split), ("f : unit -> bool", split, text.len())]
    {
        let changes = a
            .rename(&urls[0], file.position_at(text.find(needle).unwrap()), "renamed")
            .unwrap()
            .unwrap()
            .unwrap();
        let edits: Vec<_> = changes.source_file_edits.values().flatten().collect();
        assert_eq!(edits.len(), 3, "{edits:?}");
        assert!(
            edits.iter().all(|e| (lower..upper).contains(&base_db::range_start(e.range))),
            "{edits:?}"
        );
    }
}

#[test]
fn ordered_hover_and_signature_help_use_the_selected_signature() {
    let text = "val f : int -> int\nval a : unit -> int\nfunction a() = f(1)\nval f : bool -> bool\nval b : unit -> bool\nfunction b() = f(true)\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (needle, expected, excluded) in
        [("f(1)", "int -> int", "bool -> bool"), ("f(true)", "bool -> bool", "int -> int")]
    {
        let offset = text.find(needle).unwrap();
        let hover = a
            .hover(FileId::from_raw(0), file.position_at(offset), &Default::default())
            .unwrap()
            .unwrap();
        assert!(
            hover.markup.contains(expected) && !hover.markup.contains(excluded),
            "{}",
            hover.markup
        );
        let help =
            a.signature_help(FileId::from_raw(0), file.position_at(offset + 2)).unwrap().unwrap();
        assert_eq!(help.signatures.len(), 1);
        assert!(help.signatures[0].label.contains(expected), "{help:?}");
    }
}

#[test]
fn ordered_members_and_locals_do_not_use_future_enum_declarations() {
    let text = "val before : int -> int\nfunction before(Later) = Later\nenum E = { Later, Now }\nval after : unit -> E\nfunction after() = Now\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (offset, expected) in [
        (text.find("= Later").unwrap() + 2, text.find("(Later").unwrap() + 1),
        (text.rfind("Now").unwrap(), text.find("Now").unwrap()),
    ] {
        let targets = a.goto_definition(&urls[0], file.position_at(offset)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(base_db::range_start(targets[0].focus_range), expected);
    }
}

#[test]
fn ordered_private_replacement_blocks_navigation_and_references() {
    use base_db::module_graph::ModuleData;
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
    ];
    let first = "val f : unit -> int\nval before : unit -> int\nfunction before() = f()\n";
    let last = "val after : unit -> int\nfunction after() = f()\n";
    let (host, map, urls) = setup(
        &[("a.sail", first), ("b.sail", "private val f : unit -> int\n"), ("c.sail", last)],
        &[0, 1, 2],
        modules,
    );
    let a = snapshot(&host, &map);
    let last_file = a.file(&urls[2]).unwrap();
    assert!(a
        .goto_definition(&urls[2], last_file.position_at(last.find("f()").unwrap()))
        .unwrap()
        .is_empty());
    let first_file = a.file(&urls[0]).unwrap();
    let refs = a
        .find_all_refs(&urls[0], first_file.position_at(first.find("f :").unwrap()), None)
        .unwrap()
        .unwrap();
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].references.values().map(Vec::len).sum::<usize>(), 1);
}

#[test]
fn ordered_completion_excludes_future_declarations() {
    let text = "val before : unit -> int\nfunction before() = lat\nval later : unit -> int\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let items = a
        .completions(
            FileId::from_raw(0),
            file.position_at(text.find("= lat").unwrap() + 5),
            &Default::default(),
        )
        .unwrap();
    assert!(!items.iter().any(|i| i.label == "later"), "{items:?}");
}

#[test]
fn ordered_navigation_keeps_signature_and_implementation_requests_distinct() {
    let text = "val f : unit -> int\nfunction f() = 1\nval a : unit -> int\nfunction a() = f()\nval f : unit -> bool\nfunction f() = true\nval b : unit -> bool\nfunction b() = f()\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (call, signature, implementation) in [
        (
            text.find("= f()").unwrap() + 2,
            text.find("f :").unwrap(),
            text.find("function f").unwrap() + 9,
        ),
        (
            text.rfind("= f()").unwrap() + 2,
            text.rfind("f :").unwrap(),
            text.rfind("function f").unwrap() + 9,
        ),
    ] {
        let at = file.position_at(call);
        let defs = a.goto_definition(&urls[0], at).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(base_db::range_start(defs[0].focus_range), implementation);
        let decls = a.goto_declaration(&urls[0], at).unwrap();
        assert_eq!(decls.len(), 1);
        assert_eq!(base_db::range_start(decls[0].range), signature);
        let implementations = a.goto_implementation(&urls[0], at).unwrap();
        assert_eq!(implementations.len(), 1);
        assert_eq!(base_db::range_start(implementations[0].range), implementation);
    }
}

#[test]
fn ordered_record_field_navigation_uses_the_receiver_type() {
    let text = "struct A = { field : int }\nstruct B = { field : bool }\nval read : A -> int\nfunction read(x) = x.field\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let targets =
        a.goto_definition(&urls[0], file.position_at(text.rfind("field").unwrap())).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(base_db::range_start(targets[0].focus_range), text.find("field").unwrap());
}

#[test]
fn ordered_completion_keeps_the_current_signature_and_local_binding() {
    let text = "val pick : int -> int\nval before : int -> int\nfunction before(local) = { pick(local); local }\nval pick : (bool, bool) -> bool\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let items = a
        .completions(
            FileId::from_raw(0),
            file.position_at(text.find("pick(local)").unwrap() + 4),
            &Default::default(),
        )
        .unwrap();
    let picks: Vec<_> = items.iter().filter(|i| i.label == "pick").collect();
    assert_eq!(picks.len(), 1, "{items:?}");
    assert!(picks[0].detail.as_ref().unwrap().contains("int -> int"), "{picks:?}");
    let mut resolved = (*picks[0]).clone();
    a.resolve_completion_item(&mut resolved).unwrap();
    let docs = resolved.documentation.as_deref().unwrap();
    assert!(docs.contains("int -> int") && !docs.contains("bool"), "{docs}");
    let locals = a
        .completions(
            FileId::from_raw(0),
            file.position_at(text.find("; local").unwrap() + 7),
            &Default::default(),
        )
        .unwrap();
    assert!(locals.iter().any(|i| i.label == "local"), "{locals:?}");
}

#[test]
fn ordered_type_names_do_not_resolve_to_same_spelling_constructors() {
    let text = "enum E = { A }\ntype A = int\nval use : A -> int\nfunction use(x) = x\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let targets =
        a.goto_definition(&urls[0], file.position_at(text.find("A ->").unwrap())).unwrap();
    assert_eq!(targets.len(), 1);
    assert_eq!(base_db::range_start(targets[0].focus_range), text.find("A = int").unwrap());
}

#[test]
fn ordered_generated_bitfield_constructor_retains_its_origin() {
    let text = "default Order dec\nbitfield B : bits(8) = { part : 7 .. 0 }\nval make : bits(8) -> B\nfunction make(x) = Mk_B(x)\n";
    let (mut host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let targets =
        a.goto_definition(&urls[0], file.position_at(text.find("Mk_B").unwrap())).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(base_db::range_start(targets[0].focus_range), text.find("B :").unwrap());
    let rename =
        a.rename(&urls[0], file.position_at(text.find("Mk_B").unwrap()), "renamed").unwrap();
    assert!(
        rename.is_err(),
        "renaming a generated name must not edit the bitfield name: {rename:?}"
    );
    drop(a);
    let updated = format!("// moving declarations without changing their surface\n{text}");
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), &updated);
    host.apply_change(change);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let targets =
        a.goto_definition(&urls[0], file.position_at(updated.find("Mk_B").unwrap())).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(base_db::range_start(targets[0].focus_range), updated.find("B :").unwrap());
}

#[test]
fn ordered_bitfield_overloads_collect_only_visible_members() {
    use base_db::module_graph::ModuleData;
    let main = "function use(x : A) -> A = update_part(x, 0x0)\n";
    for requires in [vec![0], vec![0, 1]] {
        let (host, map, urls) = setup(
            &[
                ("a.sail", "default Order dec\nbitfield A : bits(8) = { part : 3 .. 0 }\n"),
                ("b.sail", "bitfield B : bits(8) = { part : 1 .. 0 }\n"),
                ("main.sail", main),
            ],
            &[0, 1, 2],
            vec![
                ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(2)], requires: requires.clone() },
            ],
        );
        let a = snapshot(&host, &map);
        let file = a.file(&urls[2]).unwrap();
        let help = a
            .signature_help(
                FileId::from_raw(2),
                file.position_at(main.find("update_part(").unwrap() + 12),
            )
            .unwrap()
            .unwrap();
        assert_eq!(help.signatures.len(), requires.len(), "{help:?}");
        assert!(
            help.signatures
                .iter()
                .any(|s| s.label.contains("_update_A_part") && s.label.contains("bits(4)")),
            "{help:?}"
        );
        assert_eq!(
            help.signatures.iter().any(|s| s.label.contains("_update_B_part")),
            requires.len() == 2
        );
        let diagnostics = a.file_diagnostics(FileId::from_raw(2), &Default::default()).unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_bitfield_owner_rename_keeps_derived_and_shared_names_consistent() {
    let defs = "default Order dec\n/// bitfield docs\nbitfield B_Type : bits(8) = { field_part : (7 .. 6 @ 1 .. 0) }\n";
    let body = "function use(r : register(B_Type)) -> bits(4) = { let b = Mk_B_Type(0x00); let c = _update_B_Type_field_part(b, 0x1); _set_B_Type_field_part(r, 0x2); let d = update_field_part(c, 0x3); _get_B_Type_field_part(d) }\n";
    let (mut host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let change = a
        .rename(&urls[0], file.position_at(defs.find("B_Type").unwrap()), "Renamed_Type")
        .unwrap()
        .unwrap()
        .unwrap();
    let edits = &change.source_file_edits[&FileId::from_raw(1)];
    for name in [
        "Mk_Renamed_Type",
        "_update_Renamed_Type_field_part",
        "_set_Renamed_Type_field_part",
        "_get_Renamed_Type_field_part",
    ] {
        assert!(edits.iter().any(|e| e.new_text == name), "{name}: {edits:?}");
    }
    assert!(!edits
        .iter()
        .any(|e| usize::from(e.range.start()) == body.find("update_field_part").unwrap()));
    drop(a);
    let mut update = ide_db::apply_change::Change::new();
    for (id, text) in [(0, defs), (1, body)] {
        let mut text = text.to_owned();
        let mut edits = change.source_file_edits[&FileId::from_raw(id)].clone();
        edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
        for edit in edits {
            text.replace_range(
                usize::from(edit.range.start())..usize::from(edit.range.end()),
                &edit.new_text,
            );
        }
        update.change_file(FileId::from_raw(id), text);
    }
    host.apply_change(update);
    let a = snapshot(&host, &map);
    for id in [0, 1] {
        let diagnostics = a.file_diagnostics(FileId::from_raw(id), &Default::default()).unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_bitfield_field_rename_tracks_selected_shared_alternatives() {
    let defs = "default Order dec\nbitfield A_Type : bits(8) = { field_part : 3 .. 0 }\nbitfield B_Type : bits(8) = { field_part : 3 .. 0, renamed_part : 7 .. 4 }\n";
    let main = "function use_a(x : A_Type, r : register(A_Type)) -> bits(4) = { let a = _update_A_Type_field_part(x, 0x0); _set_A_Type_field_part(r, 0x1); let b = update_field_part(a, 0x2); _mod_field_part(r, 0x3); let c = _mod_field_part(b); let d = b[field_part]; _get_A_Type_field_part(b) }\nfunction use_b(x : B_Type) -> bits(4) = { let b = update_field_part(x, 0x4); _mod_field_part(b) }\n";
    for from_use in [false, true] {
        let (mut host, map, urls) =
            setup(&[("defs.sail", defs), ("main.sail", main)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let (id, offset) = if from_use {
            (1, main.find("b[field_part]").unwrap() + 2)
        } else {
            (0, defs.find("field_part").unwrap())
        };
        let file = a.file(&urls[id]).unwrap();
        let change = a
            .rename(&urls[id], file.position_at(offset), "renamed_part")
            .unwrap()
            .unwrap()
            .unwrap();
        let expected_main = main
            .replace("_A_Type_field_part", "_A_Type_renamed_part")
            .replace("update_field_part(a", "_update_A_Type_renamed_part(a")
            .replace("_mod_field_part(r", "_set_A_Type_renamed_part(r")
            .replacen("_mod_field_part(b)", "_get_A_Type_renamed_part(b)", 1)
            .replace("b[field_part]", "b[renamed_part]");
        let expected_defs = defs.replacen("field_part", "renamed_part", 1);
        drop(a);
        let mut update = ide_db::apply_change::Change::new();
        for (id, text, expected) in [(0, defs, &expected_defs), (1, main, &expected_main)] {
            let mut text = text.to_owned();
            let mut edits = change.source_file_edits[&FileId::from_raw(id)].clone();
            edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
            for edit in edits {
                text.replace_range(
                    usize::from(edit.range.start())..usize::from(edit.range.end()),
                    &edit.new_text,
                );
            }
            assert_eq!(&text, expected);
            update.change_file(FileId::from_raw(id), text);
        }
        host.apply_change(update);
        let a = snapshot(&host, &map);
        for id in [0, 1] {
            let diagnostics =
                a.file_diagnostics(FileId::from_raw(id), &Default::default()).unwrap();
            assert!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                "{diagnostics:?}"
            );
        }
        // A second rename consumes the new explicit helper identities too.
        let file = a.file(&urls[0]).unwrap();
        let second = a
            .rename(
                &urls[0],
                file.position_at(expected_defs.find("renamed_part").unwrap()),
                "again",
            )
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(second.source_file_edits[&FileId::from_raw(1)].len(), 7);
    }
}

#[test]
fn ordered_bitfield_field_rename_expands_unshared_alias_members() {
    let source = "default Order dec\nbitfield A : bits(8) = { part : 3 .. 0 }\noverload modify = {_mod_part}\noverload update = {update_part}\nfunction use(x : A, r : register(A)) -> bits(4) = { modify(r, 0x0); modify(update(x, 0x1)) }\n";
    let (mut host, map, urls) = setup(&[("main.sail", source)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let change = a
        .rename(&urls[0], file.position_at(source.find("part :").unwrap()), "other")
        .unwrap()
        .unwrap()
        .unwrap();
    let mut text = source.to_owned();
    let mut edits = change.source_file_edits[&FileId::from_raw(0)].clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
    for edit in edits {
        text.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    assert!(text.contains("overload modify = {_get_A_other, _set_A_other}"), "{text}");
    assert!(text.contains("overload update = {_update_A_other}"), "{text}");
    drop(a);
    let mut update = ide_db::apply_change::Change::new();
    update.change_file(FileId::from_raw(0), text);
    host.apply_change(update);
    let diagnostics =
        snapshot(&host, &map).file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_bitfield_field_rename_covers_index_update_and_legacy_access() {
    let defs = "default Order dec\nbitfield A : bits(8) = { part : 3 .. 0 }\ntype Alias = A\n";
    let main = "$include \"defs.sail\"\nfunction use(x : A, part : int) -> bits(4) = { var y = x; y[part] = 0x0; let z = [y with part = 0x1]; z[part] }\nfunction old(x : Alias, r : register(A)) -> bits(4) = { r.part(0x0); x.part() }\nfunction vec(v : bits(8), part : int) -> bit = v[part]\n";
    let (mut host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", main)], &[1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[1]).unwrap();
    let change = a
        .rename(&urls[1], file.position_at(main.find("z[part]").unwrap() + 2), "renamed")
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(change.source_file_edits[&FileId::from_raw(0)].len(), 1);
    assert_eq!(change.source_file_edits[&FileId::from_raw(1)].len(), 5, "{change:?}");
    let mut updated = main.to_owned();
    let mut edits = change.source_file_edits[&FileId::from_raw(1)].clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
    for edit in edits {
        updated.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    assert_eq!(
        updated,
        main.replace("y[part]", "y[renamed]")
            .replace("with part", "with renamed")
            .replace("z[part]", "z[renamed]")
            .replace("x.part()", "x.renamed()")
            .replace("r.part(", "r.renamed(")
    );
    drop(a);
    let mut update = ide_db::apply_change::Change::new();
    update.change_file(FileId::from_raw(0), defs.replace("part", "renamed"));
    update.change_file(FileId::from_raw(1), updated);
    host.apply_change(update);
    let diagnostics =
        snapshot(&host, &map).file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn inline_bitfield_field_rename_refreshes_selected_overloads_after_edits() {
    let id = FileId::from_raw(0);
    let url = url::Url::parse("file:///project/inline.sail").unwrap();
    let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
    let mut host = AnalysisHost::new();
    for receiver in ["A", "B", "A"] {
        let text = format!("$project# A {{}} B {{}} C {{ requires A, B }}\n$start_module# A\ndefault Order dec\nbitfield A : bits(8) = {{ part : 3 .. 0 }}\n$end_module#\n$start_module# B\nbitfield B : bits(8) = {{ part : 3 .. 0 }}\n$end_module#\n$start_module# C\nfunction use(x : {receiver}) -> {receiver} = update_part(x, 0x0)\n$end_module#\n");
        let mut update = ide_db::apply_change::Change::new();
        update.change_file(id, &text);
        update.set_workspace_files(vec![id]);
        host.apply_change(update);
        let a = snapshot(&host, &map);
        let file = a.file(&url).unwrap();
        let change = a
            .rename(&url, file.position_at(text.find("part :").unwrap()), "other")
            .unwrap()
            .unwrap()
            .unwrap();
        let edits = &change.source_file_edits[&id];
        assert_eq!(edits.len(), if receiver == "A" { 2 } else { 1 }, "{receiver}{edits:?}");
        assert_eq!(edits.iter().any(|e| e.new_text == "_update_A_other"), receiver == "A");
    }
}

#[test]
fn ordered_bitfield_rename_keeps_callable_and_local_namespaces_separate() {
    let defs = "default Order dec\nbitfield A : bits(8) = { part : 3 .. 0 }\n";
    let main = "function use(x : A, update_part : int, part : int) -> bits(4) = { let y = update_part(x, 0x0); let value = update_part; let z = y[part]; _get_A_part(y) }\n";
    let (mut host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", main)], &[0, 1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[1]).unwrap();
    for name in ["update_part", "part"] {
        let at = main.find(&format!(", {name} : int")).unwrap() + 2;
        let change =
            a.rename(&urls[1], file.position_at(at), "local_value").unwrap().unwrap().unwrap();
        let edits = &change.source_file_edits[&FileId::from_raw(1)];
        assert_eq!(edits.len(), if name == "part" { 1 } else { 2 }, "{name}{edits:?}");
        assert!(
            edits.iter().all(|e| base_db::range_start(e.range) == at
                || base_db::range_start(e.range) == main.find("= update_part;").unwrap() + 2),
            "{edits:?}"
        );
    }
    let def_file = a.file(&urls[0]).unwrap();
    let change = a
        .rename(&urls[0], def_file.position_at(defs.find("part :").unwrap()), "other")
        .unwrap()
        .unwrap()
        .unwrap();
    let mut updated = main.to_owned();
    let mut edits = change.source_file_edits[&FileId::from_raw(1)].clone();
    edits.sort_by_key(|e| std::cmp::Reverse(e.range.start()));
    for edit in edits {
        updated.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    assert_eq!(
        updated,
        main.replace("update_part(x", "_update_A_other(x")
            .replace("_get_A_part(y)", "_get_A_other(y)")
            .replace("y[part]", "y[other]")
    );
    drop(a);
    let mut update = ide_db::apply_change::Change::new();
    update.change_file(FileId::from_raw(0), defs.replace("part", "other"));
    update.change_file(FileId::from_raw(1), updated);
    host.apply_change(update);
    let diagnostics =
        snapshot(&host, &map).file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn ordered_bitfield_field_rename_rejects_multiple_physical_include_contexts() {
    let defs = "bitfield A : bits(8) = { part : 3 .. 0 }\n";
    let main = "default Order dec\n$include \"defs.sail\"\n$include \"defs.sail\"\n";
    let (host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", main)], &[1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let error = a
        .rename(&urls[0], file.position_at(defs.find("part").unwrap()), "other")
        .unwrap()
        .unwrap_err();
    assert!(error.contains("multiple include contexts"), "{error}");
}

#[test]
fn ordered_bitfield_field_rename_refuses_unsafe_edits_atomically() {
    let a = "bitfield A : bits(8) = { part : 3 .. 0, other : 7 .. 4 }\n";
    let b = "bitfield B : bits(8) = { part : 3 .. 0 }\n";
    let alias = "overload change = {update_part}\n";
    for (body, new_name, reason) in [
        (format!("{a}{alias}{b}"), "renamed", "shared group"),
        (format!("{b}{alias}{a}"), "renamed", "shared group"),
        (a.to_owned(), "other", "already has a member"),
        (a.to_owned(), "bits", "implicit bitfield member"),
        (format!("{a}val _get_A_renamed : A -> bits(4)\n"), "renamed", "already exists"),
        (format!("{a}let saved = update_part\n"), "renamed", "unresolved"),
    ] {
        let source = format!("default Order dec\n{body}");
        let (host, map, urls) = setup(&[("main.sail", &source)], &[0], vec![]);
        let analysis = snapshot(&host, &map);
        let file = analysis.file(&urls[0]).unwrap();
        let error = analysis
            .rename(
                &urls[0],
                file.position_at(source.find(a).unwrap() + a.find("part").unwrap()),
                new_name,
            )
            .unwrap()
            .unwrap_err();
        assert!(error.contains(reason), "{source}{error}");
    }
}

#[test]
fn ordered_bitfield_grouped_ranges_preserve_the_exact_width() {
    for range in ["(7 .. 6 @ 1 .. 0)", "(7 .. 6) @ (1 .. 0)", "top - 1 .. 4", "0 .. 3"] {
        let defs = format!(
            "default Order dec\ntype top : Int = 8\nbitfield B : bits(8) = {{ part : {range} }}\n"
        );
        for width in [4, 3] {
            let body = format!("function use(x : B) -> bits({width}) = _get_B_part(x)\n");
            let (host, map, _) =
                setup(&[("defs.sail", &defs), ("main.sail", &body)], &[0, 1], vec![]);
            let diagnostics = snapshot(&host, &map)
                .file_diagnostics(FileId::from_raw(1), &Default::default())
                .unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                width == 3 || range == "0 .. 3",
                "{defs}{body}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_bitfield_generated_visibility_matches_sails_global_overload_scope() {
    use base_db::module_graph::ModuleData;
    for private in [false, true] {
        for local in [false, true] {
            for call in ["_get_B_part", "_mod_part", "update_part"] {
                let body = format!(
                    "function use(x : B) -> {} = {call}(x{})\n",
                    if call == "update_part" { "B" } else { "bits(4)" },
                    if call == "update_part" { ", 0x0" } else { "" }
                );
                let defs = format!(
                    "default Order dec\n{}bitfield B : bits(8) = {{ part : 3 .. 0 }}\n{}",
                    if private { "private " } else { "" },
                    if local { &body } else { "" }
                );
                let (host, map, urls) = setup(
                    &[("defs.sail", &defs), ("main.sail", if local { "" } else { &body })],
                    &[0, 1],
                    vec![
                        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                    ],
                );
                let a = snapshot(&host, &map);
                let id = if local { 0 } else { 1 };
                let file = a.file(&urls[id]).unwrap();
                let position = file.position_at(file.text().find(call).unwrap());
                let visible = !private || (local && call == "_get_B_part");
                let targets = a.goto_definition(&urls[id], position).unwrap();
                assert_eq!(
                    !targets.is_empty(),
                    visible,
                    "private={private} local={local} {call}: {targets:?}"
                );
                let diagnostics =
                    a.file_diagnostics(FileId::from_raw(id as u32), &Default::default()).unwrap();
                assert_eq!(
                    diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                    !visible,
                    "private={private} local={local} {call}: {diagnostics:?}"
                );
            }
        }
    }
}

#[test]
fn ordered_generated_enum_and_mapping_names_keep_their_origins() {
    for (definition, name, body, owner) in [
        ("enum E = { A, B }\n", "num_of_E", "function use() -> int = num_of_E(A)\n", "E ="),
        ("enum E = { A, B }\n", "E_of_num", "function use() -> E = E_of_num(0)\n", "E ="),
        (
            "enum E with label -> string = { A => \"A\", B => \"B\" }\n",
            "E_label",
            "function use() -> string = E_label(A)\n",
            "E with",
        ),
        (
            "$[enum_number_conversions {to_enum = to_E, from_enum = from_E}]\nenum E = { A, B }\n",
            "to_E",
            "function use() -> E = to_E(0)\n",
            "E =",
        ),
        (
            "$[enum_vector \"all_E\"]\nenum E = { A, B }\n",
            "all_E",
            "function use() -> vector(2, dec, E) = all_E\n",
            "E =",
        ),
        (
            "val m : bool <-> int\n",
            "m_forwards",
            "function use() -> int = m_forwards(true)\n",
            "m :",
        ),
        (
            "val m : bool <-> int\n",
            "m_backwards_matches",
            "function use() -> bool = m_backwards_matches(1)\n",
            "m :",
        ),
    ] {
        let (host, map, urls) =
            setup(&[("defs.sail", definition), ("main.sail", body)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        let targets =
            a.goto_definition(&urls[1], file.position_at(body.find(name).unwrap())).unwrap();
        assert_eq!(targets.len(), 1, "{name}: {targets:?}");
        assert_eq!(targets[0].file_id, Some(FileId::from_raw(0)));
        assert_eq!(base_db::range_start(targets[0].focus_range), definition.find(owner).unwrap());
        assert!(
            a.rename(&urls[1], file.position_at(body.find(name).unwrap()), "renamed")
                .unwrap()
                .is_err(),
            "must not rename the generating declaration through {name}"
        );
    }
}

#[test]
fn ordered_generated_names_respect_suppression_customization_and_source_order() {
    for (defs, name, body) in [
        (
            "$[no_enum_number_conversions]\nenum E = { A, B }\n",
            "num_of_E",
            "function use() -> int = num_of_E(A)\n",
        ),
        (
            "$[enum_number_conversions {to_enum = to_E, from_enum = from_E}]\nenum E = { A, B }\n",
            "num_of_E",
            "function use() -> int = num_of_E(A)\n",
        ),
        (
            "enum E = { A, B }\n",
            "E_of_num",
            "function use() -> unit = { let x = E_of_num(0); () }\n",
        ),
    ] {
        let roots = if name == "E_of_num" { vec![1, 0] } else { vec![0, 1] };
        let (host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", body)], &roots, vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        assert!(a
            .goto_definition(&urls[1], file.position_at(body.find(name).unwrap()))
            .unwrap()
            .is_empty());
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{defs}{body}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_generated_conversion_overrides_are_per_file_and_position_sensitive() {
    let text = "enum E = { A, B }\nfunction use() -> int = num_of_E(A)\nval num_of_E : E -> int\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    assert!(a
        .goto_definition(&urls[0], file.position_at(text.find("num_of_E(A)").unwrap()))
        .unwrap()
        .is_empty());
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    assert!(
        diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );

    let defs = "enum E = { A, B }\n";
    let body = "function use() -> int = num_of_E(A)\n";
    let (host, map, urls) = setup(
        &[("defs.sail", defs), ("main.sail", body), ("later.sail", "val num_of_E : E -> int\n")],
        &[0, 1, 2],
        vec![],
    );
    let a = snapshot(&host, &map);
    let file = a.file(&urls[1]).unwrap();
    let targets =
        a.goto_definition(&urls[1], file.position_at(body.find("num_of_E").unwrap())).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].file_id, Some(FileId::from_raw(0)));
}

#[test]
fn ordered_generated_enum_conversion_checks_the_finite_domain() {
    for value in [0, 1, 2, -1] {
        let defs = "enum E = { A, B }\n";
        let body = format!("function use() -> E = E_of_num({value})\n");
        let (host, map, _) = setup(&[("defs.sail", defs), ("main.sail", &body)], &[0, 1], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            value < 0 || value > 1,
            "{body}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_generated_helpers_share_signature_completion_and_reference_identity() {
    let defs = "/// mapping docs\nval m : forall 'n, 'n > 0. bits('n) <-> bits('n)\n";
    let body = "function use(x : bits(8)) -> bits(8) = m_forwards(m_forwards(x))\n";
    let (host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[1]).unwrap();
    let offset = body.find("m_forwards").unwrap();
    let refs = a.find_all_refs(&urls[1], file.position_at(offset), None).unwrap().unwrap();
    assert_eq!(refs[0].references.values().map(Vec::len).sum::<usize>(), 2, "{refs:?}");
    let help =
        a.signature_help(FileId::from_raw(1), file.position_at(offset + 11)).unwrap().unwrap();
    assert_eq!(help.signatures.len(), 1, "{help:?}");
    assert!(help.signatures[0].label.contains("'n > 0"), "{help:?}");
    let mut item = a
        .completions(FileId::from_raw(1), file.position_at(offset + 10), &Default::default())
        .unwrap()
        .into_iter()
        .find(|i| i.label == "m_forwards" && i.import.is_none())
        .unwrap();
    a.resolve_completion_item(&mut item).unwrap();
    assert!(
        item.documentation
            .as_ref()
            .is_some_and(|d| d.contains("mapping docs") && d.contains("'n > 0")),
        "{item:?}"
    );
}

#[test]
fn ordered_generated_owner_rename_updates_derived_calls() {
    for (defs, body, owner, expected) in [
        (
            "enum E = { A, B }\n",
            "function use() -> int = num_of_E(E_of_num(0))\n",
            "E",
            vec!["num_of_Renamed", "Renamed_of_num"],
        ),
        (
            "val m : bool <-> int\n",
            "function use() -> int = m_forwards(true)\n",
            "m",
            vec!["Renamed_forwards"],
        ),
        (
            "$[enum_number_conversions {to_enum = to_E, from_enum = from_E}]\nenum E = { A, B }\n",
            "function use() -> int = from_E(to_E(0))\n",
            "E",
            vec![],
        ),
    ] {
        let (mut host, map, urls) =
            setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[0]).unwrap();
        let change = a
            .rename(&urls[0], file.position_at(defs.rfind(owner).unwrap()), "Renamed")
            .unwrap()
            .unwrap()
            .unwrap();
        let edits = change.source_file_edits.get(&FileId::from_raw(1)).cloned().unwrap_or_default();
        if expected.is_empty() {
            assert!(edits.is_empty(), "{edits:?}");
        }
        for name in expected {
            assert!(edits.iter().any(|edit| edit.new_text == name), "{edits:?}");
        }
        drop(a);
        let mut update = ide_db::apply_change::Change::new();
        for (id, text) in [(0, defs), (1, body)] {
            let mut updated = text.to_owned();
            let mut edits =
                change.source_file_edits.get(&FileId::from_raw(id)).cloned().unwrap_or_default();
            edits.sort_by_key(|edit| std::cmp::Reverse(base_db::range_start(edit.range)));
            for edit in edits {
                updated.replace_range(
                    usize::from(edit.range.start())..usize::from(edit.range.end()),
                    &edit.new_text,
                );
            }
            update.change_file(FileId::from_raw(id), updated);
        }
        host.apply_change(update);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_name_diagnostics_are_not_suppressed_by_future_workspace_names() {
    let text = "let before : int = later\nlet later : int = 1\nlet own : int = own\n";
    let (host, map, _urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
    for offset in [text.find("= later").unwrap() + 2, text.rfind("own").unwrap()] {
        assert!(
            diagnostics.iter().any(|d| d.range.range.contains((offset as u32).into())
                && d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_type_navigation_uses_the_local_binding_at_the_cursor() {
    let text = "struct A = { value : int }\nstruct B = { value : bool }\nval first : A -> A\nfunction first(x) = x\nval second : B -> B\nfunction second(x) = x\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    for (offset, expected) in [
        (text.find("= x").unwrap() + 2, text.find("A =").unwrap()),
        (text.rfind("= x").unwrap() + 2, text.find("B =").unwrap()),
    ] {
        let targets = a.goto_type_definition(&urls[0], file.position_at(offset)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(base_db::range_start(targets[0].range), expected);
    }
}

#[test]
fn ordered_signature_help_does_not_use_a_global_for_an_invalid_local_call() {
    let text = "val f : int -> int\nval apply : bool -> bool\nfunction apply(f) = f(true)\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    // Sail rejects this binding collision. While the user edits it, signature
    // help must not claim that the local boolean has the global int signature.
    let help = a
        .signature_help(FileId::from_raw(0), file.position_at(text.rfind("f(true)").unwrap() + 2))
        .unwrap();
    assert!(help.is_none(), "{help:?}");
}

#[test]
fn ordered_completion_resolve_rejects_removed_include_origins() {
    let text =
        "$include \"library.sail\"\nval caller : int -> int\nfunction caller(x) = helper(x)\n";
    let (mut host, map, urls) =
        setup(&[("main.sail", text), ("library.sail", "val helper : int -> int\n")], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let mut item = a
        .completions(
            FileId::from_raw(0),
            file.position_at(text.find("helper(x)").unwrap() + 6),
            &Default::default(),
        )
        .unwrap()
        .into_iter()
        .find(|i| i.label == "helper")
        .unwrap();
    a.resolve_completion_item(&mut item).unwrap();
    assert!(item.documentation.as_deref().unwrap().contains("int -> int"));
    drop(a);
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), text.replacen("$include \"library.sail\"\n", "", 1));
    host.apply_change(change);
    snapshot(&host, &map).resolve_completion_item(&mut item).unwrap();
    assert!(item.documentation.is_none());
}

#[test]
fn ordered_generated_mapping_tuple_is_one_argument() {
    let defs = "val m : (int, bool) <-> int\n";
    let body = "function use() -> int = m_forwards((1, true))\n";
    let (host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[1]).unwrap();
    let help = a
        .signature_help(FileId::from_raw(1), file.position_at(body.find("((1").unwrap() + 1))
        .unwrap()
        .unwrap();
    assert_eq!(help.signatures[0].parameters.len(), 1, "{help:?}");
}

#[test]
fn inline_program_ide_requests_share_declaration_identity() {
    let text = "$project# A {} B {} C { requires A }\n$start_module# A\nval chosen : unit -> int\n$end_module#\n$start_module# C\nfunction early() -> int = chosen()\n$end_module#\n$start_module# B\nval chosen : unit -> bool\n$end_module#\n$start_module# C\nfunction late() -> bool = chosen()\n$end_module#\n";
    let id = FileId::from_raw(0);
    let url = url::Url::parse("file:///project/inline.sail").unwrap();
    let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(id, text);
    change.set_workspace_files(vec![id]);
    host.apply_change(change);
    let a = snapshot(&host, &map);
    let file = a.file(&url).unwrap();
    let early = text.find("chosen()").unwrap();
    let late = text.rfind("chosen()").unwrap();
    let definition = text.find("chosen :").unwrap();
    let targets = a.goto_definition(&url, file.position_at(early)).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(base_db::range_start(targets[0].focus_range), definition);
    let declarations = a.goto_declaration(&url, file.position_at(early)).unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(base_db::range_start(declarations[0].range), definition);
    assert!(a.goto_definition(&url, file.position_at(late)).unwrap().is_empty());
    let help = a.signature_help(id, file.position_at(early + 7)).unwrap().unwrap();
    assert_eq!(help.signatures.len(), 1);
    assert!(help.signatures[0].label.contains("int"), "{help:?}");
    assert!(a.signature_help(id, file.position_at(late + 7)).unwrap().is_none());
    let hover = a.hover(id, file.position_at(early), &Default::default()).unwrap().unwrap();
    assert!(hover.markup.contains("int") && !hover.markup.contains("bool"), "{hover:?}");
    for (offset, visible) in [(early, true), (late, false)] {
        let items = a.completions(id, file.position_at(offset + 3), &Default::default()).unwrap();
        assert_eq!(items.iter().any(|i| i.label == "chosen"), visible, "{items:?}");
    }
    let edit = a.rename(&url, file.position_at(early), "renamed").unwrap().unwrap().unwrap();
    let edits = &edit.source_file_edits[&id];
    let mut starts: Vec<_> = edits.iter().map(|e| usize::from(e.range.start())).collect();
    starts.sort();
    assert_eq!(starts, [definition, early]);
}

#[test]
fn ordered_typed_overloads_exclude_hidden_same_arity_members() {
    use base_db::module_graph::ModuleData;
    for (argument, accepted) in [("int", true), ("bool", false)] {
        let body = format!("function use(x : {argument}) -> int = outer(x)\n");
        let modules = vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
        ];
        let (host, map, urls) = setup(
            &[
                ("a.sail", "val one : int -> int\noverload inner = {one}\n"),
                (
                    "b.sail",
                    "val two : bool -> int\noverload inner = {two}\noverload outer = {inner}\n",
                ),
                ("c.sail", &body),
            ],
            &[0, 1, 2],
            modules,
        );
        let a = snapshot(&host, &map);
        let file = a.file(&urls[2]).unwrap();
        let call = body.find("outer(x)").unwrap();
        let help =
            a.signature_help(FileId::from_raw(2), file.position_at(call + 6)).unwrap().unwrap();
        assert_eq!(help.signatures.len(), 1, "{help:?}");
        assert!(help.signatures[0].label.contains("one"), "{help:?}");
        assert!(!help.signatures[0].label.contains("bool"), "{help:?}");
        let nav = a.goto_definition(&urls[2], file.position_at(call)).unwrap();
        assert_eq!(nav.len(), 1, "{nav:?}");
        assert_eq!(nav[0].file_id, Some(FileId::from_raw(1)));
        let change =
            a.rename(&urls[2], file.position_at(call), "renamed").unwrap().unwrap().unwrap();
        assert!(!change.source_file_edits.contains_key(&FileId::from_raw(0)));
        assert_eq!(change.source_file_edits[&FileId::from_raw(1)].len(), 1);
        assert_eq!(change.source_file_edits[&FileId::from_raw(2)].len(), 1);
        let diagnostics = a.file_diagnostics(FileId::from_raw(2), &Default::default()).unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            accepted,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn inline_program_external_navigation_and_hierarchy_keep_workspace_support() {
    let text =
        "$project# A {}\n$start_module# A\nfunction run() -> int = external(true)\n$end_module#\n";
    let ids = [FileId::from_raw(0), FileId::from_raw(1)];
    let urls: Vec<_> = ["inline", "library"]
        .iter()
        .map(|name| url::Url::parse(&format!("file:///project/{name}.sail")).unwrap())
        .collect();
    let map = UrlMap::new(
        urls.iter().cloned().zip(ids).collect(),
        ids.into_iter().zip(urls.iter().cloned()).collect(),
    );
    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(ids[0], text);
    change.change_file(ids[1], "val external : bool -> int\n");
    change.set_workspace_files(ids.to_vec());
    host.apply_change(change);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let offset = text.find("external(true)").unwrap();
    let position = file.position_at(offset);
    let targets = a.goto_definition(&urls[0], position).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].url, urls[1]);
    let call = a.prepare_call_hierarchy(&urls[0], position).unwrap().unwrap();
    assert_eq!(call.url, urls[1]);
    let help = a.signature_help(ids[0], file.position_at(offset + 9)).unwrap().unwrap();
    assert_eq!(help.signatures.len(), 1);
    assert!(help.signatures[0].label.contains("bool"), "{help:?}");
    let items = a.completions(ids[0], file.position_at(offset + 3), &Default::default()).unwrap();
    assert!(items.iter().any(|i| i.label == "external"), "{items:?}");
}

#[test]
fn individual_constructor_privacy_is_shared_by_ide_requests() {
    use base_db::module_graph::ModuleData;
    let definitions =
        "union U = { private Hidden : unit, Public : unit }\nfunction owner() -> U = Hidden()\n";
    let body = "function allowed() -> U = Public()\nfunction denied() -> U = Hidden()\n";
    for inline in [false, true] {
        let combined = format!("$project# A {{}} B {{ requires A }}\n$start_module# A\n{definitions}$end_module#\n$start_module# B\n{body}$end_module#\n");
        let (host, map, urls) = if inline {
            let id = FileId::from_raw(0);
            let url = url::Url::parse("file:///project/inline.sail").unwrap();
            let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
            let mut host = AnalysisHost::new();
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(id, &combined);
            change.set_workspace_files(vec![id]);
            host.apply_change(change);
            (host, map, vec![url])
        } else {
            setup(
                &[("defs.sail", definitions), ("main.sail", body)],
                &[0, 1],
                vec![
                    ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                    ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                ],
            )
        };
        let a = snapshot(&host, &map);
        let id = FileId::from_raw(if inline { 0 } else { 1 });
        let url = urls.last().unwrap();
        let text = if inline { combined.as_str() } else { body };
        let file = a.file(url).unwrap();
        for (name, visible) in [("Public", true), ("Hidden", false)] {
            let offset = text.rfind(&format!("{name}()")).unwrap();
            let targets = a.goto_definition(url, file.position_at(offset)).unwrap();
            assert_eq!(!targets.is_empty(), visible, "inline={inline} {name}: {targets:?}");
            if visible {
                let declaration = if inline { combined.as_str() } else { definitions };
                assert_eq!(
                    base_db::range_start(targets[0].focus_range),
                    declaration.find("Public :").unwrap()
                );
            }
            let items =
                a.completions(id, file.position_at(offset + 3), &Default::default()).unwrap();
            assert_eq!(
                items.iter().any(|i| i.label == name && i.import.is_none()),
                visible,
                "inline={inline} {name}: {items:?}"
            );
        }
        let own_file = a.file(&urls[0]).unwrap();
        let own_text = if inline { combined.as_str() } else { definitions };
        assert!(!a
            .goto_definition(&urls[0], own_file.position_at(own_text.find("Hidden()").unwrap()))
            .unwrap()
            .is_empty());
        let diagnostics = a.file_diagnostics(id, &Default::default()).unwrap();
        assert!(
            diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn private_enum_helpers_keep_public_identity_and_private_type_checks() {
    use base_db::module_graph::ModuleData;
    let definitions = "default Order dec\n$[enum_vector \"all_E\"]\nprivate enum E with label -> string = { A => \"A\", B => \"B\" }\n";
    for inline in [false, true] {
        for same_module in [true, false] {
            let main = "function number() -> int = num_of_E(E_of_num(0))\nfunction label() -> string = E_label(E_of_num(0))\nfunction values() -> unit = { let x = all_E; () }\n";
            let modules = if same_module {
                vec![ModuleData {
                    files: vec![FileId::from_raw(0), FileId::from_raw(1)],
                    requires: vec![],
                }]
            } else {
                vec![
                    ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                    ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                ]
            };
            let combined = format!("$project# A {{}} B {{ requires A }}\n$start_module# A\n{definitions}$end_module#\n$start_module# {}\n{main}$end_module#\n", if same_module { "A" } else { "B" });
            let (host, map, urls) = if inline {
                let id = FileId::from_raw(0);
                let url = url::Url::parse("file:///project/inline.sail").unwrap();
                let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
                let mut host = AnalysisHost::new();
                let mut change = ide_db::apply_change::Change::new();
                change.change_file(id, &combined);
                change.set_workspace_files(vec![id]);
                host.apply_change(change);
                (host, map, vec![url])
            } else {
                setup(&[("defs.sail", definitions), ("main.sail", main)], &[0, 1], modules)
            };
            let use_id = FileId::from_raw(if inline { 0 } else { 1 });
            let use_url = urls.last().unwrap();
            let main = if inline { combined.as_str() } else { main };
            let a = snapshot(&host, &map);
            let file = a.file(use_url).unwrap();
            for name in ["E_of_num", "num_of_E", "E_label", "all_E"] {
                let at = main.rfind(name).unwrap();
                let visible = same_module || name != "all_E";
                let targets = a.goto_definition(use_url, file.position_at(at)).unwrap();
                assert_eq!(!targets.is_empty(), visible, "{name}: {targets:?}");
                let items =
                    a.completions(use_id, file.position_at(at + 3), &Default::default()).unwrap();
                assert_eq!(
                    items.iter().any(|i| i.label == name && i.import.is_none()),
                    visible,
                    "{name}: {items:?}"
                );
                if name != "all_E" {
                    let call =
                        a.prepare_call_hierarchy(use_url, file.position_at(at)).unwrap().unwrap();
                    assert_eq!(call.name, name);
                    assert_eq!(call.url, urls[0]);
                    let help =
                        a.signature_help(use_id, file.position_at(at + name.len() + 1)).unwrap();
                    assert!(help.is_some(), "{name}: no signature");
                }
            }
            let diagnostics = a.file_diagnostics(use_id, &Default::default()).unwrap();
            if same_module {
                assert!(
                    !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                    "{diagnostics:?}"
                );
            } else {
                assert!(
                    diagnostics.iter().any(|d| d.message.contains("Well-formedness check failed")
                        && d.message.contains("type E")),
                    "{diagnostics:?}"
                );
            }
        }
    }
}

#[test]
fn scattered_members_keep_clause_identity_and_kind_specific_scope() {
    use base_db::module_graph::ModuleData;
    for (kind, clause, expression) in [
        ("enum", "enum clause E = Member\n", "Member"),
        ("union", "union clause E = Member : unit\n", "Member()"),
    ] {
        for requires_clause in [false, true] {
            let head = format!("scattered {kind} E\n");
            let body = format!("function run() -> E = {expression}\n");
            let (host, map, urls) = setup(
                &[
                    ("head.sail", &head),
                    ("clause.sail", clause),
                    ("end.sail", "end E\n"),
                    ("main.sail", &body),
                ],
                &[0, 1, 2, 3],
                vec![
                    ModuleData {
                        files: vec![FileId::from_raw(0), FileId::from_raw(2)],
                        requires: vec![],
                    },
                    ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                    ModuleData {
                        files: vec![FileId::from_raw(3)],
                        requires: if requires_clause { vec![0, 1] } else { vec![0] },
                    },
                ],
            );
            let a = snapshot(&host, &map);
            let file = a.file(&urls[3]).unwrap();
            let use_at = body.find("Member").unwrap();
            let visible = kind == "enum" || requires_clause;
            let targets = a.goto_definition(&urls[3], file.position_at(use_at)).unwrap();
            assert_eq!(
                !targets.is_empty(),
                visible,
                "{kind} requires_clause={requires_clause}: {targets:?}"
            );
            if visible {
                assert_eq!(targets[0].url, urls[1]);
                assert_eq!(
                    base_db::range_start(targets[0].focus_range),
                    clause.find("Member").unwrap()
                );
            }
            let items = a
                .completions(FileId::from_raw(3), file.position_at(use_at + 3), &Default::default())
                .unwrap();
            assert_eq!(
                items.iter().any(|i| i.label == "Member" && i.import.is_none()),
                visible,
                "{kind}: {items:?}"
            );
            let diagnostics = a.file_diagnostics(FileId::from_raw(3), &Default::default()).unwrap();
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                visible,
                "{kind}: {diagnostics:?}"
            );
            let type_at = body.find("E =").unwrap();
            let type_items = a
                .completions(
                    FileId::from_raw(3),
                    file.position_at(type_at + 1),
                    &Default::default(),
                )
                .unwrap();
            assert!(type_items.iter().any(|i| i.label == "E"), "{type_items:?}");
            let renamed =
                a.rename(&urls[3], file.position_at(type_at), "Renamed").unwrap().unwrap().unwrap();
            assert_eq!(renamed.source_file_edits.len(), 4, "{renamed:?}");
            for index in 0..4 {
                assert_eq!(
                    renamed.source_file_edits[&FileId::from_raw(index)].len(),
                    1,
                    "{renamed:?}"
                );
            }
            for (index, text) in [(1, clause), (2, "end E\n")] {
                let f = a.file(&urls[index]).unwrap();
                let targets = a
                    .goto_definition(&urls[index], f.position_at(text.find("E").unwrap()))
                    .unwrap();
                assert_eq!(targets.len(), 1, "{targets:?}");
                assert_eq!(targets[0].url, urls[0], "{targets:?}");
            }
        }
    }
}

#[test]
fn inline_scattered_enum_members_inherit_head_scope_and_privacy() {
    for private in [false, true] {
        let text = if private {
            "$project# A {} B { requires A } C { requires A }\n$start_module# A\nprivate scattered enum E\nenum clause E = Member\n$end_module#\n$start_module# C\nfunction run() -> unit = { let _x = Member; () }\n$end_module#\n"
        } else {
            "$project# A {} B { requires A } C { requires A }\n$start_module# A\nscattered enum E\n$end_module#\n$start_module# B\nenum clause E = Member\n$end_module#\n$start_module# C\nfunction run() -> unit = { let _x = Member; () }\n$end_module#\n"
        };
        let id = FileId::from_raw(0);
        let url = url::Url::parse("file:///project/inline.sail").unwrap();
        let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
        let mut host = AnalysisHost::new();
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(id, text);
        change.set_workspace_files(vec![id]);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        let file = a.file(&url).unwrap();
        let at = text.rfind("Member").unwrap();
        let targets = a.goto_definition(&url, file.position_at(at)).unwrap();
        assert_eq!(!targets.is_empty(), !private, "{targets:?}");
        let items = a.completions(id, file.position_at(at + 3), &Default::default()).unwrap();
        assert_eq!(items.iter().any(|i| i.label == "Member"), !private, "{items:?}");
        let diagnostics = a.file_diagnostics(id, &Default::default()).unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            !private,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn scattered_type_rename_does_not_rename_a_same_named_callable() {
    let text = "scattered enum E\nenum clause E = EA\nval E : unit -> int\nend E\nfunction a() -> E = EA\nfunction b() -> int = E()\n";
    let (host, map, urls) = setup(&[("main.sail", text)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let edit = a
        .rename(&urls[0], file.position_at(text.find("E = EA").unwrap()), "Renamed")
        .unwrap()
        .unwrap()
        .unwrap();
    let edits = &edit.source_file_edits[&FileId::from_raw(0)];
    assert_eq!(edits.len(), 4, "{edits:?}");
    assert!(
        edits.iter().all(|e| base_db::range_start(e.range) != text.find("E : unit").unwrap()
            && base_db::range_start(e.range) != text.rfind("E()").unwrap()),
        "{edits:?}"
    );
}

#[test]
fn scattered_enum_vectors_are_available_at_end_with_exact_length() {
    for ended in [false, true] {
        let head = "default Order dec\n$[enum_vector \"all_E\"]\nscattered enum E\n";
        let clause = "enum clause E = EA\nenum clause E = EB\n";
        let body = "function values() -> vector(2, E) = all_E\n";
        let order: &[usize] = if ended { &[0, 1, 2, 3] } else { &[0, 1, 3, 2] };
        let (host, map, urls) = setup(
            &[
                ("head.sail", head),
                ("clause.sail", clause),
                ("end.sail", "end E\n"),
                ("main.sail", body),
            ],
            order,
            vec![],
        );
        let a = snapshot(&host, &map);
        let file = a.file(&urls[3]).unwrap();
        let at = body.find("all_E").unwrap();
        let targets = a.goto_definition(&urls[3], file.position_at(at)).unwrap();
        assert_eq!(!targets.is_empty(), ended, "{targets:?}");
        if ended {
            assert_eq!(targets[0].url, urls[0]);
            let hover = a
                .hover(FileId::from_raw(3), file.position_at(at), &Default::default())
                .unwrap()
                .unwrap();
            assert!(format!("{hover:?}").contains("vector(2, E)"), "{hover:?}");
        }
        let items = a
            .completions(FileId::from_raw(3), file.position_at(at + 3), &Default::default())
            .unwrap();
        assert_eq!(
            items.iter().any(|i| i.label == "all_E" && i.import.is_none()),
            ended,
            "{items:?}"
        );
        let diagnostics = a.file_diagnostics(FileId::from_raw(3), &Default::default()).unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            ended,
            "{diagnostics:?}"
        );
    }
}

#[test]
fn scattered_enum_vectors_keep_local_binding_scope_and_nominal_type_checks() {
    use base_db::module_graph::ModuleData;
    for private in [false, true] {
        for requires in [vec![0], vec![1], vec![0, 1]] {
            let head = format!("default Order dec\n$[enum_vector \"all_E\"]\n{}scattered enum E\nenum clause E = EA\n{}", if private {"private "} else {""}, if private {"end E\n"} else {""});
            let end = if private { "" } else { "end E\n" };
            let body = "function values() -> unit = { let _x = all_E; () }\n";
            let (host, map, urls) = setup(
                &[("head.sail", &head), ("end.sail", end), ("main.sail", body)],
                &[0, 1, 2],
                vec![
                    ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                    ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                    ModuleData { files: vec![FileId::from_raw(2)], requires: requires.clone() },
                ],
            );
            let a = snapshot(&host, &map);
            let file = a.file(&urls[2]).unwrap();
            let at = body.find("all_E").unwrap();
            let targets = a.goto_definition(&urls[2], file.position_at(at)).unwrap();
            assert_eq!(targets.len(), 1, "private={private} requires={requires:?}: {targets:?}");
            assert_eq!(targets[0].url, urls[0]);
            let items = a
                .completions(FileId::from_raw(2), file.position_at(at + 3), &Default::default())
                .unwrap();
            assert!(
                items.iter().any(|i| i.label == "all_E" && i.import.is_none()),
                "private={private} requires={requires:?}: {items:?}"
            );
            let diagnostics = a.file_diagnostics(FileId::from_raw(2), &Default::default()).unwrap();
            let valid = !private && requires.contains(&0);
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                valid,
                "private={private} requires={requires:?}: {diagnostics:?}"
            );
            if !valid {
                assert!(
                    diagnostics.iter().any(|d| d.message.contains("Well-formedness check failed")
                        && d.message.contains("type E")),
                    "{diagnostics:?}"
                );
            }
        }
    }
}

#[test]
fn scattered_member_availability_and_private_union_clauses() {
    use base_db::module_graph::ModuleData;
    for (kind, clause, expression) in [
        ("enum", "enum clause E = Member\n", "Member"),
        ("union", "union clause E = Member : unit\n", "Member()"),
    ] {
        for after in [false, true] {
            let body = format!("function run() -> E = {expression}\n");
            let text = format!(
                "scattered {kind} E\n{}end E\n",
                if after { format!("{clause}{body}") } else { format!("{body}{clause}") }
            );
            let (host, map, urls) = setup(&[("main.sail", &text)], &[0], vec![]);
            let a = snapshot(&host, &map);
            let file = a.file(&urls[0]).unwrap();
            let at = text.find(&body).unwrap() + body.find("Member").unwrap();
            let targets = a.goto_definition(&urls[0], file.position_at(at)).unwrap();
            assert_eq!(!targets.is_empty(), after, "{text}{targets:?}");
            let items = a
                .completions(FileId::from_raw(0), file.position_at(at + 3), &Default::default())
                .unwrap();
            assert_eq!(
                items.iter().any(|i| i.label == "Member" && i.import.is_none()),
                after,
                "{text}{items:?}"
            );
            let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                after,
                "{text}{diagnostics:?}"
            );
        }
    }
    for clause in
        ["union clause E = private Member : unit\n", "private union clause E = Member : unit\n"]
    {
        for owner in [false, true] {
            let body = "function run() -> E = Member()\n";
            let modules = if owner {
                vec![ModuleData { files: (0..3).map(FileId::from_raw).collect(), requires: vec![] }]
            } else {
                vec![
                    ModuleData {
                        files: vec![FileId::from_raw(0), FileId::from_raw(1)],
                        requires: vec![],
                    },
                    ModuleData { files: vec![FileId::from_raw(2)], requires: vec![0] },
                ]
            };
            let (host, map, urls) = setup(
                &[
                    ("head.sail", "scattered union E\n"),
                    ("clause.sail", clause),
                    ("main.sail", body),
                ],
                &[0, 1, 2],
                modules,
            );
            let a = snapshot(&host, &map);
            let file = a.file(&urls[2]).unwrap();
            let targets = a
                .goto_definition(&urls[2], file.position_at(body.find("Member").unwrap()))
                .unwrap();
            assert_eq!(!targets.is_empty(), owner, "{clause}{targets:?}");
            let diagnostics = a.file_diagnostics(FileId::from_raw(2), &Default::default()).unwrap();
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                owner,
                "{clause}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn inline_scattered_vectors_resolve_after_end_and_check_element_scope() {
    for requires in ["A", "B"] {
        let text = format!("$project# A {{}} B {{ requires A }} C {{ requires {requires} }}\n$start_module# A\ndefault Order dec\n$[enum_vector \"all_E\"]\nscattered enum E\nenum clause E = EA\n$end_module#\n$start_module# B\nend E\n$end_module#\n$start_module# C\nfunction run() -> unit = {{ let _x = all_E; () }}\n$end_module#\n");
        let id = FileId::from_raw(0);
        let url = url::Url::parse("file:///project/inline.sail").unwrap();
        let map = UrlMap::new([(url.clone(), id)].into(), [(id, url.clone())].into());
        let mut host = AnalysisHost::new();
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(id, &text);
        change.set_workspace_files(vec![id]);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        let file = a.file(&url).unwrap();
        let at = text.rfind("all_E").unwrap();
        assert_eq!(a.goto_definition(&url, file.position_at(at)).unwrap().len(), 1);
        assert!(a
            .completions(id, file.position_at(at + 3), &Default::default())
            .unwrap()
            .iter()
            .any(|i| i.label == "all_E"));
        let diagnostics = a.file_diagnostics(id, &Default::default()).unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            requires == "A",
            "{diagnostics:?}"
        );
    }
}

#[test]
fn scattered_callable_heads_do_not_make_future_signatures_available() {
    for (kind, signature, clause, expression, result) in [
        ("function", "val f : int -> int\n", "function clause f(x) = x\n", "f(1)", "int"),
        (
            "mapping",
            "val f : int <-> bool\n",
            "mapping clause f = 1 <-> true\n",
            "f_forwards(1)",
            "bool",
        ),
    ] {
        for before in [false, true] {
            let body = format!("function run() -> {result} = {expression}\n");
            let text = format!(
                "scattered {kind} f\n{}end f\n",
                if before {
                    format!("{body}{signature}{clause}")
                } else {
                    format!("{signature}{clause}{body}")
                }
            );
            let (host, map, urls) = setup(&[("main.sail", &text)], &[0], vec![]);
            let a = snapshot(&host, &map);
            let file = a.file(&urls[0]).unwrap();
            let at = text.find(&body).unwrap() + body.find(expression).unwrap();
            let targets = a.goto_definition(&urls[0], file.position_at(at)).unwrap();
            assert_eq!(!targets.is_empty(), !before, "{text}{targets:?}");
            if !before {
                let rename_at =
                    if kind == "function" { at } else { text.find(" f\n").unwrap() + 1 };
                let edit = a
                    .rename(&urls[0], file.position_at(rename_at), "renamed")
                    .unwrap()
                    .unwrap()
                    .unwrap();
                assert_eq!(edit.source_file_edits[&FileId::from_raw(0)].len(), 5, "{text}{edit:?}");
            }
            let items = a
                .completions(FileId::from_raw(0), file.position_at(at + 1), &Default::default())
                .unwrap();
            assert_eq!(
                items.iter().any(|i| i.label == "f" && i.import.is_none()),
                !before,
                "{text}{items:?}"
            );
            let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
            assert_eq!(
                !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                !before,
                "{text}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn ordered_generated_names_keep_type_and_value_identities_separate() {
    for (defs, body, signature, callable) in [
        ("$[enum_number_conversions {to_enum = E, from_enum = as_int}]\nenum E = { EA, EB }\n", "function run() -> E = E(0)\n", "val E :", true),
        ("default Order dec\n$[enum_vector \"E\"]\nenum E = { EA, EB }\n", "function run() -> vector(2, E) = E\n", "let E : vector(2", false),
        ("default Order dec\n$[enum_vector \"E\"]\nscattered enum E\nenum clause E = EA\nend E\n", "function run() -> vector(1, E) = E\n", "let E : vector(1", false),
    ] {
        let (mut host, map, urls) = setup(&[("defs.sail", defs), ("main.sail", body)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        let type_at = file.position_at(body.find('E').unwrap());
        let value_at = file.position_at(body.rfind('E').unwrap());
        let type_def = ide_db::defs::Definition::classify_at(&file, type_at).unwrap();
        let value_def = ide_db::defs::Definition::classify_at(&file, value_at).unwrap();
        assert!(matches!(type_def, ide_db::defs::Definition::TypeDef(_)), "{type_def:?}");
        assert_ne!(type_def, value_def, "{defs}{body}");
        assert_eq!(matches!(value_def, ide_db::defs::Definition::ValSpec(_)), callable, "{value_def:?}");
        assert!(value_def.def_location().unwrap().signature_text(&a.db).unwrap().contains(signature));
        assert!(a.rename(&urls[1], value_at, "Renamed").unwrap().is_err());
        let navigation = a.goto_definition(&urls[1], value_at).unwrap();
        assert_eq!(navigation.len(), 1, "{navigation:?}");
        assert!(navigation[0].detail.as_deref().unwrap().contains(signature), "{navigation:?}");
        if callable {
            let help = a.signature_help(FileId::from_raw(1), file.position_at(body.rfind('E').unwrap() + 2)).unwrap().unwrap();
            assert_eq!(help.signatures.len(), 1, "{help:?}");
            assert!(help.signatures[0].label.contains("E"), "{help:?}");
        }
        let completions = a.completions(FileId::from_raw(1), file.position_at(body.rfind('E').unwrap() + 1), &Default::default()).unwrap();
        let mut value = completions.into_iter().find(|item| item.label == "E" && matches!(item.kind, ide_db::defs::CompletionItemKind::Function | ide_db::defs::CompletionItemKind::Variable | ide_db::defs::CompletionItemKind::Constant)).expect("generated value completion");
        a.resolve_completion_item(&mut value).unwrap();
        assert!(value.documentation.as_deref().unwrap().contains(signature), "{value:?}");
        let change = a.rename(&urls[1], type_at, "Renamed").unwrap().unwrap().unwrap();
        let mut updated = vec![];
        for (id, text) in [(0, defs), (1, body)] {
            let mut text = text.to_owned();
            let mut edits = change.source_file_edits.get(&FileId::from_raw(id)).cloned().unwrap_or_default();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
            for edit in edits { text.replace_range(usize::from(edit.range.start())..usize::from(edit.range.end()), &edit.new_text); }
            updated.push(text);
        }
        assert!(updated[1].ends_with(if callable { "= E(0)\n" } else { "= E\n" }), "{updated:?}");
        assert!(updated[1].contains("Renamed"), "{updated:?}");
        assert!(updated[0].contains(if callable { "to_enum = E" } else { "enum_vector \"E\"" }), "{updated:?}");
        drop(a);
        let mut update = ide_db::apply_change::Change::new();
        for (id, text) in updated.into_iter().enumerate() { update.change_file(FileId::from_raw(id as u32), text); }
        host.apply_change(update);
        let a = snapshot(&host, &map);
        for id in [0, 1] {
            let diagnostics = a.file_diagnostics(FileId::from_raw(id), &Default::default()).unwrap();
            assert!(!diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), "{defs}{body}{diagnostics:?}");
        }
    }
}

#[test]
fn bitfield_declarations_check_each_complete_range_expression() {
    for (field, code) in [
        ("0 .. 3", Some("bitfield-reversed-range")),
        ("0 .. 1", Some("bitfield-reversed-range")),
        ("7 .. 4 @ 0 .. 3", Some("bitfield-reversed-range")),
        ("(0 .. 3) @ (7 .. 0)", Some("bitfield-reversed-range")),
        ("top - 1 .. 4", None),
        ("top - 1 .. top", Some("bitfield-reversed-range")),
        ("top .. 0", Some("bitfield-index-out-of-range")),
        ("-1", Some("bitfield-index-out-of-range")),
        ("16 - 9 .. 0", None),
        ("7 .. 9 - 9", None),
        ("0 .. 0", None),
    ] {
        for underlying in ["bits(8)", "Word(8)", "Byte"] {
            let defs = "default Order dec\ntype top : Int = 8\ntype Word('n : Int) = bitvector('n, dec)\ntype Byte = Word(top)\n";
            let main = format!("bitfield B : {underlying} = {{ part : {field} }}\n");
            let (host, map, _) =
                setup(&[("defs.sail", defs), ("main.sail", &main)], &[0, 1], vec![]);
            let diagnostics = snapshot(&host, &map)
                .file_diagnostics(FileId::from_raw(1), &Default::default())
                .unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                code.is_some(),
                "{main}{diagnostics:?}"
            );
            if let Some(code) = code {
                assert!(
                    diagnostics.iter().any(|d| d.code.as_str() == code),
                    "{main}{diagnostics:?}"
                );
            }
        }
        for inline in [false, true] {
            let prefix = if inline { "$project# Main {}\n$start_module# Main\n" } else { "" };
            let text = format!("{prefix}default Order dec\ntype top : Int = 8\nbitfield B : bits(8) = {{ part : {field} }}\n");
            let mut host = AnalysisHost::default();
            let id = FileId::from_raw(0);
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(id, text.clone());
            change.set_workspace_files(vec![id]);
            host.apply_change(change);
            let url = url::Url::parse("file:///project/main.sail").unwrap();
            let map = UrlMap::new(
                [(url.clone(), id)].into_iter().collect(),
                [(id, url)].into_iter().collect(),
            );
            let diagnostics =
                snapshot(&host, &map).file_diagnostics(id, &Default::default()).unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                code.is_some(),
                "{text}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn inline_generated_collision_completion_resolves_its_own_namespace() {
    for (defs, body, signature) in [
        (
            "$[enum_number_conversions {to_enum = E, from_enum = as_int}]\nenum E = { EA, EB }\n",
            "function run() -> E = E(0)\n",
            "val E :",
        ),
        (
            "$[enum_vector \"E\"]\nenum E = { EA, EB }\n",
            "function run() -> vector(2, E) = E\n",
            "let E : vector(2",
        ),
        (
            "$[enum_vector \"E\"]\nscattered enum E\nenum clause E = EA\nend E\n",
            "function run() -> vector(1, E) = E\n",
            "let E : vector(1",
        ),
    ] {
        let text =
            format!("$project# Main {{}}\n$start_module# Main\ndefault Order dec\n{defs}{body}");
        let mut host = AnalysisHost::new();
        let id = FileId::from_raw(0);
        let url = url::Url::parse("file:///project/main.sail").unwrap();
        let map = UrlMap::new(
            [(url.clone(), id)].into_iter().collect(),
            [(id, url.clone())].into_iter().collect(),
        );
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(id, text.clone());
        change.set_workspace_files(vec![id]);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        let file = a.file(&url).unwrap();
        let offset = text.rfind('E').unwrap();
        let def = ide_db::defs::Definition::classify_at(&file, file.position_at(offset)).unwrap();
        assert!(!matches!(def, ide_db::defs::Definition::TypeDef(_)), "{def:?}");
        let items = a.completions(id, file.position_at(offset + 1), &Default::default()).unwrap();
        let mut item = items
            .into_iter()
            .find(|item| {
                item.label == "E"
                    && item.origin.is_some_and(|origin| {
                        origin.namespace == hir::source_order::bindings::Namespace::Value
                    })
            })
            .unwrap();
        a.resolve_completion_item(&mut item).unwrap();
        assert!(item.documentation.as_deref().unwrap().contains(signature), "{item:?}");
        assert!(a.rename(&url, file.position_at(offset), "Renamed").unwrap().is_err());
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(id, "enum E = { EA, EB }\n".to_owned());
        host.apply_change(change);
        snapshot(&host, &map).resolve_completion_item(&mut item).unwrap();
        assert!(item.documentation.is_none(), "stale generated origin: {item:?}");
    }
}

#[test]
fn ordered_instantiation_rhs_keeps_type_callable_and_key_identities_separate() {
    let defs = "outcome O : 'a -> unit with 'a : Type = { val convert : 'a -> string }\ntype T = unit\nval T : T -> string\nval convert : unit -> unit\nlet T_value = 0\n";
    let main = "instantiation O with 'a = T, convert = T\n";
    for (target, expected, other) in [
        (main.find("= T").unwrap() + 2, "type T", main.rfind('T').unwrap()),
        (main.rfind('T').unwrap(), "val T", main.find("= T").unwrap() + 2),
    ] {
        let (mut host, map, urls) =
            setup(&[("defs.sail", defs), ("main.sail", main)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        let targets = a.goto_definition(&urls[1], file.position_at(target)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(
            base_db::range_start(targets[0].focus_range),
            defs.find(expected).unwrap() + if expected.starts_with("type") { 5 } else { 4 }
        );
        assert!(ide_db::defs::Definition::classify_at(
            &file,
            file.position_at(main.find("convert").unwrap())
        )
        .is_none());
        let items = a
            .completions(FileId::from_raw(1), file.position_at(target + 1), &Default::default())
            .unwrap();
        let candidates: Vec<_> = items.iter().filter(|item| item.label == "T").collect();
        assert_eq!(candidates.len(), 1, "{items:?}");
        assert!(!items.iter().any(|item| item.label == "T_value"), "{items:?}");
        let mut item = (*candidates[0]).clone();
        a.resolve_completion_item(&mut item).unwrap();
        assert!(item.documentation.as_deref().unwrap().contains(expected), "{item:?}");
        let change =
            a.rename(&urls[1], file.position_at(target), "Renamed").unwrap().unwrap().unwrap();
        let edits = &change.source_file_edits[&FileId::from_raw(1)];
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(base_db::range_start(edits[0].range), target);
        assert_ne!(base_db::range_start(edits[0].range), other);
        drop(a);
        let mut update = ide_db::apply_change::Change::new();
        for (id, source) in [(0, defs), (1, main)] {
            let mut text = source.to_owned();
            let mut edits =
                change.source_file_edits.get(&FileId::from_raw(id)).cloned().unwrap_or_default();
            edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
            for edit in edits {
                text.replace_range(
                    usize::from(edit.range.start())..usize::from(edit.range.end()),
                    &edit.new_text,
                );
            }
            update.change_file(FileId::from_raw(id), text);
        }
        host.apply_change(update);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{diagnostics:?}"
        );
    }
}

#[test]
fn instantiation_references_follow_namespace_privacy_and_order() {
    use base_db::module_graph::ModuleData;
    let outcome = "outcome O : 'a -> unit with 'a : Type = { val convert : 'a -> string }\n";
    for (type_private, function_private) in [(false, false), (true, false), (false, true)] {
        for type_rhs in [false, true] {
            let definitions = format!("{outcome}{}type Thing = unit\n{}val Thing : unit -> string\nval convert_impl : unit -> string\n", if type_private {"private "} else {""}, if function_private {"private "} else {""});
            let main = if type_rhs {
                "instantiation O with 'a = Thing, convert = convert_impl\n"
            } else {
                "instantiation O with 'a = unit, convert = Thing\n"
            };
            let unavailable = if type_rhs { type_private } else { function_private };
            for inline in [false, true] {
                let combined = format!("$project# A {{}} B {{ requires A }}\n$start_module# A\n{definitions}$end_module#\n$start_module# B\n{main}$end_module#\n");
                let (host, map, urls) = if inline {
                    let id = FileId::from_raw(0);
                    let url = url::Url::parse("file:///project/main.sail").unwrap();
                    let map = UrlMap::new(
                        [(url.clone(), id)].into_iter().collect(),
                        [(id, url.clone())].into_iter().collect(),
                    );
                    let mut host = AnalysisHost::new();
                    let mut change = ide_db::apply_change::Change::new();
                    change.change_file(id, combined.clone());
                    change.set_workspace_files(vec![id]);
                    host.apply_change(change);
                    (host, map, vec![url])
                } else {
                    setup(
                        &[("defs.sail", &definitions), ("main.sail", main)],
                        &[0, 1],
                        vec![
                            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
                        ],
                    )
                };
                let id = FileId::from_raw(if inline { 0 } else { 1 });
                let text = if inline { combined.as_str() } else { main };
                let url = urls.last().unwrap();
                let a = snapshot(&host, &map);
                let file = a.file(url).unwrap();
                let offset = text.rfind("Thing").unwrap();
                let targets = a.goto_definition(url, file.position_at(offset)).unwrap();
                assert_eq!(targets.is_empty(), unavailable, "{combined}{targets:?}");
                let items =
                    a.completions(id, file.position_at(offset + 5), &Default::default()).unwrap();
                assert_eq!(
                    items.iter().any(|item| item.label == "Thing"),
                    !unavailable,
                    "{combined}{items:?}"
                );
                let diagnostics = a.file_diagnostics(id, &Default::default()).unwrap();
                assert_eq!(
                    diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                    unavailable,
                    "{combined}{diagnostics:?}"
                );
                if unavailable {
                    assert!(
                        diagnostics
                            .iter()
                            .any(|d| d.code.as_str() == "unknown-instantiation-reference"
                                && !d.related.is_empty()),
                        "{diagnostics:?}"
                    );
                }
            }
        }
    }
    for (replacement, declaration) in [
        ("'a = Thing, convert = convert_impl", "type Thing = unit\n"),
        ("'a = unit, convert = Thing", "val Thing : unit -> string\n"),
    ] {
        for before in [false, true] {
            let text = format!("{outcome}val convert_impl : unit -> string\n{}instantiation O with {replacement}\n{}", if before {declaration} else {""}, if before {""} else {declaration});
            let (host, map, urls) = setup(&[("main.sail", &text)], &[0], vec![]);
            let a = snapshot(&host, &map);
            let file = a.file(&urls[0]).unwrap();
            let offset = text.find(replacement).unwrap() + replacement.find("Thing").unwrap();
            assert_eq!(
                a.goto_definition(&urls[0], file.position_at(offset)).unwrap().is_empty(),
                !before
            );
            let diagnostics = a.file_diagnostics(FileId::from_raw(0), &Default::default()).unwrap();
            assert_eq!(
                diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
                !before,
                "{text}{diagnostics:?}"
            );
        }
    }
}

#[test]
fn instantiation_signature_substitutes_whole_type_variables() {
    for (actual, accepted) in [("Xa", false), ("string", true)] {
        let main = format!("type X = unit\ntype Xa = unit\noutcome O : ('a, 'aa) -> unit with 'a : Type, 'aa : Type = {{ val convert : 'aa -> string }}\nval convert_impl : {actual} -> string\ninstantiation O with 'a = X, 'aa = string, convert = convert_impl\n");
        let (host, map, _) = setup(&[("main.sail", &main)], &[0], vec![]);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            accepted,
            "{main}{diagnostics:?}"
        );
        if !accepted {
            assert!(
                diagnostics.iter().any(|d| d.message.contains("convert : string -> string")),
                "{diagnostics:?}"
            );
        }
    }
}

#[test]
fn instantiation_signature_keeps_outcome_supporting_alias_context() {
    use base_db::module_graph::ModuleData;
    let defs = "private type Hidden = string\noutcome O : 'a -> unit with 'a : Type = { val convert : 'a -> Hidden }\n";
    for (result, accepted) in [("string", true), ("int", false)] {
        let main = format!("val convert_impl : unit -> {result}\ninstantiation O with 'a = unit, convert = convert_impl\n");
        let (host, map, _) = setup(
            &[("defs.sail", defs), ("main.sail", &main)],
            &[0, 1],
            vec![
                ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
            ],
        );
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        assert_eq!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            accepted,
            "{main}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_nested_overload_diamond_has_one_signature_per_visible_leaf() {
    let definitions = "val one : int -> int\nval two : bool -> bool\noverload left = {one, two}\noverload right = {one, two}\noverload outer = {left, right}\n";
    for (kind, result) in [("int", "int"), ("bool", "bool")] {
        let main = format!("function use(x : {kind}) -> {result} = outer(x)\n");
        let (host, map, urls) =
            setup(&[("defs.sail", definitions), ("main.sail", &main)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        let help = a
            .signature_help(
                FileId::from_raw(1),
                file.position_at(main.find("outer(x)").unwrap() + 6),
            )
            .unwrap()
            .unwrap();
        assert_eq!(help.signatures.len(), 2, "{help:?}");
        assert!(help.signatures[0].label.contains("one"), "{help:?}");
        assert!(help.signatures[1].label.contains("two"), "{help:?}");
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(
            !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
            "{main}{diagnostics:?}"
        );
    }
}

#[test]
fn ordered_overloads_reject_more_indirection_than_current_sail() {
    for groups in [
        "overload inner = {one, two}\noverload left = {inner, one}\noverload right = {inner, two}\noverload outer = {left, right}\n",
        "overload inner = {one}\noverload left = {inner}\noverload outer = {one, left}\n",
    ] {
        let definitions = format!("val one : int -> int\nval two : bool -> bool\n{groups}");
        let main = "function use(x : int) -> int = outer(x)\n";
        let (host, map, urls) = setup(&[("defs.sail", &definitions), ("main.sail", main)], &[0, 1], vec![]);
        let a = snapshot(&host, &map);
        let file = a.file(&urls[1]).unwrap();
        assert!(a.signature_help(FileId::from_raw(1), file.position_at(main.find("outer(x)").unwrap() + 6)).unwrap().is_none());
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert!(diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error), "{definitions}{main}{diagnostics:?}");
    }
}

#[test]
fn strict_bitvector_configuration_invalidates_types_and_preprocessing() {
    let (mut host, map, _) = setup(
        &[("main.sail", "default Order dec\n$ifdef STRICT_BITVECTOR\nfunction branch() -> int = true\n$else\nfunction branch() -> int = 0\n$endif\nfunction empty() -> unit = { let _ : list(bitvector(-1)) = [||]; () }\n")],
        &[0], vec![],
    );
    for enabled in [false, true, false] {
        let mut change = ide_db::apply_change::Change::new();
        change.set_strict_bitvector(enabled);
        host.apply_change(change);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(0), &Default::default())
            .unwrap();
        let errors: Vec<_> =
            diagnostics.iter().filter(|d| d.severity == ide_diagnostics::Severity::Error).collect();
        if enabled {
            assert!(errors.iter().any(|d| d.message.contains("Well-formedness")), "{errors:?}");
            assert!(errors.iter().any(|d| d.message.contains("mismatch")), "{errors:?}");
        } else {
            assert!(errors.is_empty(), "{errors:?}");
        }
    }
}

#[test]
fn record_undefined_eligibility_tracks_provider_constraints() {
    let provider = "default Order dec\nstruct R('n : Int) = { x : bitvector('n) }\n";
    let (mut host, map, _) = setup(
        &[
            ("types.sail", provider),
            (
                "main.sail",
                "$include \"types.sail\"\nregister V : R(1)\nfunction f() -> R(1) = undefined\n",
            ),
        ],
        &[1],
        vec![],
    );
    for constrained in [false, true, false] {
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(
            FileId::from_raw(0),
            if constrained {
                provider.replace("Int)", "Int), 'n >= 0")
            } else {
                provider.to_owned()
            },
        );
        host.apply_change(change);
        let diagnostics = snapshot(&host, &map)
            .file_diagnostics(FileId::from_raw(1), &Default::default())
            .unwrap();
        let errors: Vec<_> =
            diagnostics.iter().filter(|d| d.severity == ide_diagnostics::Severity::Error).collect();
        if constrained {
            assert!(errors.is_empty(), "{errors:?}");
        } else {
            assert!(errors.iter().any(|d| d.message.contains("cannot be undefined")), "{errors:?}");
            assert!(
                errors.iter().any(|d| d.message.contains("default register value")),
                "{errors:?}"
            );
        }
    }
}
