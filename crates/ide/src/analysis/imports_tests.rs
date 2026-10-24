//! Apply the actual assist/completion edits through the analysis host.
use super::hierarchy::tests::{setup, snapshot};
use super::*;
use ide_db::text_edit::TextEdit;

fn fixes(a: &Analysis, url: &url::Url, text: &str, needle: &str) -> Vec<TextEdit> {
    let offset = text.find(needle).unwrap();
    a.assists(url, base_db::text_range(offset, offset), &Default::default())
        .unwrap()
        .into_iter()
        .filter(|a| a.id.0 == "auto_include")
        .flat_map(|a| a.source_change.unwrap().source_file_edits.into_values().flatten())
        .collect()
}
fn apply(text: &str, edit: &TextEdit) -> String {
    let mut text = text.to_owned();
    text.replace_range(
        usize::from(edit.range.start())..usize::from(edit.range.end()),
        &edit.new_text,
    );
    text
}
fn assert_clean(a: &Analysis, id: u32) {
    let diagnostics = a.file_diagnostics(FileId::from_raw(id), &Default::default()).unwrap();
    assert!(
        !diagnostics.iter().any(|d| d.severity == ide_diagnostics::Severity::Error),
        "{diagnostics:?}"
    );
}

#[test]
fn include_fix_uses_relative_path_and_resolves_after_application() {
    let main = "// 项目说明\r\ndefault Order dec\r\nfunction use(x : int) -> int = helper(x)\r\n";
    let (mut host, map, urls) = setup(
        &[("src/main.sail", main), ("lib/工具.sail", "val helper : int -> int\n")],
        &[0],
        vec![],
    );
    let a = snapshot(&host, &map);
    let edits = fixes(&a, &urls[0], main, "helper");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert_eq!(edits[0].new_text, "$include \"../lib/工具.sail\"\r\n");
    let updated = apply(main, &edits[0]);
    assert!(updated.find("default Order").unwrap() < updated.find("$include").unwrap());
    drop(a);
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), &updated);
    host.apply_change(change);
    let a = snapshot(&host, &map);
    assert_clean(&a, 0);
    let f = a.file(&urls[0]).unwrap();
    let targets =
        a.goto_definition(&urls[0], f.position_at(updated.find("helper(x)").unwrap())).unwrap();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert_eq!(targets[0].file_id, Some(FileId::from_raw(1)));
    assert!(fixes(&a, &urls[0], &updated, "helper(x)").is_empty());
}

#[test]
fn completion_include_is_lazy_and_keeps_ambiguous_origins_separate() {
    let main = "function use(x : int) -> int = hel(x)\n";
    let (host, map, urls) = setup(
        &[
            ("main.sail", main),
            ("a/common.sail", "val helper : int -> int\n"),
            ("b/common.sail", "val helper : int -> int\n"),
        ],
        &[0],
        vec![],
    );
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let items = a
        .completions(FileId::from_raw(0), file.position_at(main.find("hel(").unwrap() + 3))
        .unwrap();
    let mut imports: Vec<_> = items.into_iter().filter(|i| i.import.is_some()).collect();
    assert_eq!(imports.len(), 2, "{imports:?}");
    for item in &mut imports {
        assert!(item.additional_text_edits.is_empty());
        a.resolve_completion_item(item).unwrap();
        assert_eq!(item.additional_text_edits.len(), 1, "{item:?}");
    }
    assert_ne!(imports[0].additional_text_edits, imports[1].additional_text_edits);
    assert!(imports[0].additional_text_edits[0].new_text.contains("a/common.sail"));
    assert!(imports[1].additional_text_edits[0].new_text.contains("b/common.sail"));
}

#[test]
fn include_preview_preserves_guards_and_rejects_duplicates_cycles_and_inactive_candidates() {
    for (main, helper, valid) in [
        ("function use() -> unit = helper()\n$include \"helper.sail\"\n", "$ifndef HELPER\n$define HELPER\nval helper : unit -> unit\nfunction helper() = ()\n$endif\n", true),
        ("function use() -> unit = helper()\n$include \"helper.sail\"\n", "val helper : unit -> unit\nfunction helper() = ()\n", false),
        ("function use() -> unit = helper()\n", "$include \"main.sail\"\nval helper : unit -> unit\n", false),
        ("function use() -> unit = helper()\n", "$ifdef OFF\nval helper : unit -> unit\n$endif\n", false),
        ("function use() -> unit = helper()\n$ifndef FLAG\nval preserve : unit -> unit\n$endif\n", "$define FLAG\nval helper : unit -> unit\n", false),
        ("function use() -> unit = helper()\n", "$include \"missing.sail\"\nval helper : unit -> unit\n", false),
    ] {
        let (mut host, map, urls) = setup(&[("main.sail", main), ("helper.sail", helper)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let edits = fixes(&a, &urls[0], main, "helper()");
        assert_eq!(!edits.is_empty(), valid, "{main}{helper}{edits:?}");
        if let Some(edit) = edits.first() {
            let updated = apply(main, edit);
            drop(a);
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(0), updated);
            host.apply_change(change);
            assert_clean(&snapshot(&host, &map), 0);
        }
    }
}

#[test]
fn include_search_respects_locals_namespaces_and_module_privacy() {
    let main = "function use(helper : int) -> int = helper\n";
    let (host, map, urls) =
        setup(&[("main.sail", main), ("helper.sail", "val helper : unit -> unit\n")], &[0], vec![]);
    let a = snapshot(&host, &map);
    let offset = main.rfind("helper").unwrap();
    assert!(a
        .assists(&urls[0], base_db::text_range(offset, offset), &Default::default())
        .unwrap()
        .iter()
        .all(|assist| assist.id.0 != "auto_include"));
    let main = "val use : Helper -> unit\n";
    let (host, map, urls) = setup(
        &[
            ("main.sail", main),
            ("types.sail", "type Helper = int\n"),
            ("values.sail", "val Helper : unit -> unit\n"),
        ],
        &[0],
        vec![],
    );
    let edits = fixes(&snapshot(&host, &map), &urls[0], main, "Helper");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert!(edits[0].new_text.contains("types.sail"));
    use base_db::module_graph::ModuleData;
    let main = "function use() -> unit = hidden()\n";
    let (host, map, urls) = setup(
        &[("defs.sail", "private val hidden : unit -> unit\n"), ("main.sail", main)],
        &[0, 1],
        vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
        ],
    );
    assert!(fixes(&snapshot(&host, &map), &urls[1], main, "hidden()").is_empty());
}

#[test]
fn include_completion_rejects_stale_source_and_target() {
    for changed in [0, 1] {
        let main = "function use() -> unit = hel()\n";
        let (mut host, map, urls) = setup(
            &[("main.sail", main), ("helper.sail", "val helper : unit -> unit\n")],
            &[0],
            vec![],
        );
        let a = snapshot(&host, &map);
        let f = a.file(&urls[0]).unwrap();
        let mut item = a
            .completions(FileId::from_raw(0), f.position_at(main.find("hel(").unwrap() + 3))
            .unwrap()
            .into_iter()
            .find(|i| i.import.is_some())
            .unwrap();
        a.resolve_completion_item(&mut item).unwrap();
        assert_eq!(item.additional_text_edits.len(), 1);
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(changed), "\n");
        host.apply_change(change);
        snapshot(&host, &map).resolve_completion_item(&mut item).unwrap();
        assert!(item.additional_text_edits.is_empty());
        assert!(item.documentation.is_none());
    }
}

#[test]
fn include_preview_activates_nested_dependencies_at_the_insertion_context() {
    let main = "$define READY\nfunction use() -> unit = helper()\n";
    let (mut host, map, urls) = setup(&[("main.sail", main), ("helper.sail", "$ifdef READY\n$include \"nested.sail\"\nval helper : unit -> unit\nfunction helper() = nested()\n$endif\n"), ("nested.sail", "val nested : unit -> unit\n")], &[0], vec![]);
    let a = snapshot(&host, &map);
    let edits = fixes(&a, &urls[0], main, "helper");
    assert_eq!(edits.len(), 1, "{edits:?}");
    drop(a);
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), apply(main, &edits[0]));
    host.apply_change(change);
    let a = snapshot(&host, &map);
    assert_clean(&a, 0);
    assert_clean(&a, 1);
}

#[test]
fn include_fix_keeps_doc_comments_and_attributes_attached() {
    let main = "/// retained docs\n$[test]\nfunction run() -> unit = helper()\n";
    let (host, map, urls) =
        setup(&[("main.sail", main), ("helper.sail", "val helper : unit -> unit\n")], &[0], vec![]);
    let edits = fixes(&snapshot(&host, &map), &urls[0], main, "helper");
    assert_eq!(edits.len(), 1, "{edits:?}");
    assert!(
        apply(main, &edits[0])
            .starts_with("$include \"helper.sail\"\n/// retained docs\n$[test]\nfunction"),
        "{}",
        apply(main, &edits[0])
    );
}

#[test]
fn include_fix_checks_all_visits_of_a_shared_source() {
    // One physical edit runs in both modules. A global include guard would
    // define helper only in the first module, so it cannot repair both calls.
    use base_db::module_graph::ModuleData;
    let shared = "function use() -> unit = helper()\n";
    let modules = vec![
        ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
        ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
    ];
    let (host, map, urls) = setup(
        &[
            ("a.sail", "$include \"shared.sail\"\n"),
            ("b.sail", "$include \"shared.sail\"\n"),
            ("shared.sail", shared),
            ("helper.sail", "$ifndef HELPER\n$define HELPER\nval helper : unit -> unit\n$endif\n"),
        ],
        &[0, 1],
        modules,
    );
    assert!(fixes(&snapshot(&host, &map), &urls[2], shared, "helper").is_empty());
}

#[test]
fn include_completion_docs_follow_the_activated_branch() {
    let main = "$define READY\nfunction use(x : int) -> int = hel(x)\n";
    let helper = "$ifdef READY\n/// selected docs\nval helper : int -> int\n$else\n/// inactive docs\nval helper : unit -> unit\n$endif\n";
    let (host, map, urls) = setup(&[("main.sail", main), ("helper.sail", helper)], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let mut item = a
        .completions(FileId::from_raw(0), file.position_at(main.find("hel(").unwrap() + 3))
        .unwrap()
        .into_iter()
        .find(|i| i.import.is_some())
        .unwrap();
    a.resolve_completion_item(&mut item).unwrap();
    assert_eq!(item.additional_text_edits.len(), 1);
    let docs = item.documentation.unwrap();
    assert!(docs.contains("int -> int") && docs.contains("selected docs"), "{docs}");
    assert!(!docs.contains("inactive docs"), "{docs}");
}

#[test]
fn include_completion_revalidates_context_after_configuration_changes() {
    let main = "function use() -> unit = hel()\n";
    let (mut host, map, urls) = setup(
        &[("main.sail", main), ("helper.sail", "$iftarget c\nval helper : unit -> unit\n$endif\n")],
        &[0],
        vec![],
    );
    let mut project = base_db::preprocess::WorkspaceSourceProject::try_get(host.raw_database())
        .unwrap()
        .project(host.raw_database())
        .clone();
    project.target = Some("c".into());
    let mut change = ide_db::apply_change::Change::new();
    change.set_source_project(project.clone());
    host.apply_change(change);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let mut item = a
        .completions(FileId::from_raw(0), file.position_at(main.find("hel(").unwrap() + 3))
        .unwrap()
        .into_iter()
        .find(|i| i.import.is_some())
        .unwrap();
    a.resolve_completion_item(&mut item).unwrap();
    assert_eq!(item.additional_text_edits.len(), 1);
    drop(a);
    project.target = Some("ocaml".into());
    let mut change = ide_db::apply_change::Change::new();
    change.set_source_project(project);
    host.apply_change(change);
    snapshot(&host, &map).resolve_completion_item(&mut item).unwrap();
    assert!(item.additional_text_edits.is_empty());
}

#[test]
fn include_privacy_follows_the_selected_branch_not_the_catalog_representative() {
    use base_db::module_graph::ModuleData;
    for (first, last, allowed) in [("", "private ", true), ("private ", "", false)] {
        let main = "$define READY\nfunction use() -> unit = helper()\n";
        let helper = format!("$ifdef READY\n{first}val helper : unit -> unit\n$else\n{last}val helper : unit -> unit\n$endif\n");
        let (host, map, urls) = setup(
            &[("helper.sail", &helper), ("main.sail", main)],
            &[0, 1],
            vec![
                ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ],
        );
        let edits = fixes(&snapshot(&host, &map), &urls[1], main, "helper()");
        assert_eq!(!edits.is_empty(), allowed, "{helper}{edits:?}");
    }
}

#[test]
fn include_resolve_rejects_out_of_bounds_client_offsets() {
    let main = "function use() -> unit = hel()\n";
    let (host, map, urls) =
        setup(&[("main.sail", main), ("helper.sail", "val helper : unit -> unit\n")], &[0], vec![]);
    let a = snapshot(&host, &map);
    let file = a.file(&urls[0]).unwrap();
    let item = a
        .completions(FileId::from_raw(0), file.position_at(main.find("hel(").unwrap() + 3))
        .unwrap()
        .into_iter()
        .find(|i| i.import.is_some())
        .unwrap();
    for offset in [main.len() + 1, usize::MAX] {
        let mut request = item.import.clone().unwrap();
        request.offset = offset;
        assert!(a.resolve_auto_include(&request).unwrap().is_none());
    }
}

#[test]
fn include_generated_names_applies_the_owning_declaration() {
    for (helper, main, name) in [
        ("enum E = { A, B }\n", "function use() -> int = num_of_E(A)\n", "num_of_E"),
        (
            "$ifdef OFF\nval num_of_E : unit -> unit\n$endif\nenum E = { A, B }\n",
            "function use() -> int = num_of_E(A)\n",
            "num_of_E",
        ),
        (
            "enum E = { A, B }\n",
            "function use() -> unit = { let x = E_of_num(0); () }\n",
            "E_of_num",
        ),
        (
            "$[enum_vector \"all_E\"]\nenum E = { A, B }\n",
            "function use() -> unit = { let x = all_E; () }\n",
            "all_E",
        ),
        (
            "val m : bool <-> int\nmapping m = { true <-> 1, false <-> 0 }\n",
            "function use() -> int = m_forwards(true)\n",
            "m_forwards",
        ),
    ] {
        let (mut host, map, urls) =
            setup(&[("main.sail", main), ("helper.sail", helper)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let edits = fixes(&a, &urls[0], main, name);
        assert_eq!(edits.len(), 1, "{name}: {edits:?}");
        let updated = apply(main, &edits[0]);
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), updated);
        host.apply_change(change);
        assert_clean(&snapshot(&host, &map), 0);
    }
}

#[test]
fn include_bitfield_helpers_and_overloads_apply_the_actual_declaration() {
    let helper = "bitfield B : bits(8) = { part : (7 .. 6 @ 1 .. 0) }\n";
    for (body, name) in [
        ("function use() -> unit = { let b = Mk_B(0x00); () }\n", "Mk_B"),
        ("function use() -> bits(4) = _get_B_part(Mk_B(0x00))\n", "_get_B_part"),
        ("function use() -> bits(8) = _get_B_bits(Mk_B(0x00))\n", "_get_B_bits"),
        (
            "function use() -> unit = { let b = _update_B_part(Mk_B(0x00), 0x0); () }\n",
            "_update_B_part",
        ),
        ("function use(r : register(B)) -> unit = _set_B_part(r, 0x0)\n", "_set_B_part"),
        ("function use() -> bits(4) = _mod_part(Mk_B(0x00))\n", "_mod_part"),
        ("function use() -> unit = { let b = update_part(Mk_B(0x00), 0x0); () }\n", "update_part"),
    ] {
        let main = format!("default Order dec\n{body}");
        let (mut host, map, urls) =
            setup(&[("main.sail", &main), ("helper.sail", helper)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let edits = fixes(&a, &urls[0], &main, name);
        assert_eq!(edits.len(), 1, "{name}: {edits:?}");
        let updated = apply(&main, &edits[0]);
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), &updated);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        assert_clean(&a, 0);
        let file = a.file(&urls[0]).unwrap();
        let targets =
            a.goto_definition(&urls[0], file.position_at(updated.find(name).unwrap())).unwrap();
        assert_eq!(targets.len(), 1, "{name}: {targets:?}");
        assert_eq!(targets[0].file_id, Some(FileId::from_raw(1)));
    }
}

#[test]
fn include_bitfield_can_extend_existing_field_overloads() {
    let main = "default Order dec\nbitfield A : bits(8) = { part : 3 .. 0 }\nfunction first(x : A) -> bits(4) = _mod_part(x)\nfunction use() -> unit = { let b = Mk_B(0x00); () }\n";
    let (mut host, map, urls) = setup(
        &[("main.sail", main), ("helper.sail", "bitfield B : bits(8) = { part : 1 .. 0 }\n")],
        &[0],
        vec![],
    );
    let a = snapshot(&host, &map);
    let edits = fixes(&a, &urls[0], main, "Mk_B");
    assert_eq!(edits.len(), 1, "{edits:?}");
    drop(a);
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(0), apply(main, &edits[0]));
    host.apply_change(change);
    assert_clean(&snapshot(&host, &map), 0);
}

#[test]
fn include_preview_checks_duplicate_declarations_across_module_owners() {
    use base_db::module_graph::ModuleData;
    for (helper, main, name, allowed) in [
        (
            "val helper : unit -> unit\nfunction helper() = ()\n",
            "function use() -> unit = helper()\n",
            "helper",
            false,
        ),
        ("type Helper = int\n", "val use : Helper -> unit\n", "Helper", false),
        ("enum Helper = { A, B }\n", "val use : Helper -> unit\n", "Helper", false),
        ("val helper : unit -> unit\n", "function use() -> unit = helper()\n", "helper", true),
        (
            "outcome audit : int('n) -> unit with 'n\n",
            "instantiation audit with 'n = 32\n",
            "audit",
            true,
        ),
    ] {
        let (mut host, map, urls) = setup(
            &[("a.sail", "$include \"helper.sail\"\n"), ("b.sail", main), ("helper.sail", helper)],
            &[0, 1],
            vec![
                ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
            ],
        );
        let a = snapshot(&host, &map);
        let edits = fixes(&a, &urls[1], main, name);
        assert_eq!(!edits.is_empty(), allowed, "{helper}{edits:?}");
        // Lazy completions must enforce the same boundary at resolve time.
        let file = a.file(&urls[1]).unwrap();
        let mut item = a
            .completions(
                FileId::from_raw(1),
                file.position_at(main.find(name).unwrap() + name.len()),
            )
            .unwrap()
            .into_iter()
            .find(|item| item.import.is_some())
            .unwrap();
        a.resolve_completion_item(&mut item).unwrap();
        assert_eq!(!item.additional_text_edits.is_empty(), allowed, "{helper}{item:?}");
        if let Some(edit) = edits.first() {
            assert_eq!(item.additional_text_edits, edits);
            let updated = apply(main, edit);
            drop(a);
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(1), updated);
            host.apply_change(change);
            let a = snapshot(&host, &map);
            assert_clean(&a, 0);
            assert_clean(&a, 1);
        }
    }
}

#[test]
fn include_outcome_target_uses_its_own_namespace() {
    for (prefix, suffix, helper_suffix) in [
        ("", "", ""),
        ("val audit : unit -> unit\n", "", ""),
        ("", "val audit : unit -> unit\n", ""),
        ("", "", "val audit : unit -> unit\n"),
    ] {
        let main = format!("{prefix}instantiation audit with 'n = 32\n{suffix}");
        let outcome = format!("outcome audit : int('n) -> unit with 'n\n{helper_suffix}");
        let (mut host, map, urls) = setup(
            &[
                ("main.sail", &main),
                ("outcome.sail", &outcome),
                ("function.sail", "val audit : unit -> unit\n"),
            ],
            &[0],
            vec![],
        );
        let a = snapshot(&host, &map);
        let offset = main.find("instantiation audit").unwrap() + "instantiation ".len();
        let edits = fixes(&a, &urls[0], &main, &main[offset..]);
        assert_eq!(edits.len(), 1, "{main}{edits:?}");
        assert_eq!(edits[0].new_text, "$include \"outcome.sail\"\n");
        let file = a.file(&urls[0]).unwrap();
        let mut imports: Vec<_> = a
            .completions(FileId::from_raw(0), file.position_at(offset + 5))
            .unwrap()
            .into_iter()
            .filter(|item| item.import.is_some())
            .collect();
        assert_eq!(imports.len(), 1, "{imports:?}");
        a.resolve_completion_item(&mut imports[0]).unwrap();
        assert_eq!(imports[0].additional_text_edits, edits);
        let updated = apply(&main, &edits[0]);
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), &updated);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        assert_clean(&a, 0);
        let offset = updated.find("instantiation audit").unwrap() + "instantiation ".len();
        assert!(fixes(&a, &urls[0], &updated, &updated[offset..]).is_empty());
        let file = a.file(&urls[0]).unwrap();
        let targets = a.goto_definition(&urls[0], file.position_at(offset)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file_id, Some(FileId::from_raw(1)), "{targets:?}");
        let references =
            a.find_all_refs(&urls[0], file.position_at(offset), None).unwrap().unwrap();
        assert_eq!(
            references[0].references.values().map(Vec::len).sum::<usize>(),
            1,
            "{references:?}"
        );
        let renamed =
            a.rename(&urls[0], file.position_at(offset), "renamed").unwrap().unwrap().unwrap();
        let edits = &renamed.source_file_edits[&FileId::from_raw(0)];
        assert_eq!(edits.len(), 1, "{edits:?}");
        let updated = apply(&updated, &edits[0]);
        assert_eq!(
            updated,
            format!(
                "{prefix}$include \"outcome.sail\"\ninstantiation renamed with 'n = 32\n{suffix}"
            )
        );
        let edits = &renamed.source_file_edits[&FileId::from_raw(1)];
        assert_eq!(edits.len(), 1, "{edits:?}");
        let outcome = apply(&outcome, &edits[0]);
        assert_eq!(outcome, format!("outcome renamed : int('n) -> unit with 'n\n{helper_suffix}"));
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), updated);
        change.change_file(FileId::from_raw(1), outcome);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        assert_clean(&a, 0);
        assert_clean(&a, 1);
    }
}

#[test]
fn include_outcome_target_rejects_values_and_other_modules_private_outcomes() {
    let main = "instantiation audit with 'n = 32\n";
    let (host, map, urls) = setup(
        &[("main.sail", main), ("function.sail", "val audit : unit -> unit\n")],
        &[0],
        vec![],
    );
    assert!(fixes(&snapshot(&host, &map), &urls[0], main, "audit").is_empty());

    use base_db::module_graph::ModuleData;
    for (privacy, known) in [("", true), ("private ", false)] {
        let defs =
            format!("{privacy}outcome audit : int('n) -> unit with 'n\nval audit : unit -> unit\n");
        let (host, map, urls) = setup(
            &[("defs.sail", &defs), ("main.sail", main)],
            &[0, 1],
            vec![
                ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
                ModuleData { files: vec![FileId::from_raw(1)], requires: vec![0] },
            ],
        );
        let a = snapshot(&host, &map);
        assert!(fixes(&a, &urls[1], main, "audit").is_empty());
        let diagnostics = a.file_diagnostics(FileId::from_raw(1), &Default::default()).unwrap();
        assert_eq!(
            diagnostics.iter().any(|d| matches!(
                d.code,
                hir::diagnostics::DiagnosticCode::SailError("unknown-outcome")
            )),
            !known,
            "{diagnostics:?}"
        );
        let file = a.file(&urls[1]).unwrap();
        let targets =
            a.goto_definition(&urls[1], file.position_at(main.find("audit").unwrap())).unwrap();
        assert_eq!(!targets.is_empty(), known, "{targets:?}");
        if known {
            assert_eq!(usize::from(targets[0].focus_range.start()), defs.find("audit").unwrap());
        }
    }
}

#[test]
fn include_preview_validates_scattered_state_in_expansion_order() {
    for (label, before, extension, after, accepted) in [
        ("complete union", "", "scattered union U\nunion clause U = C : int\nend U\n", "", true),
        ("implicit end", "", "scattered union U\nunion clause U = C : int\n", "", true),
        ("parent head", "scattered union U\n", "union clause U = C : int\n", "end U\n", true),
        ("future head", "", "union clause U = C : int\n", "scattered union U\nend U\n", false),
        ("empty union", "", "scattered union U\nend U\n", "", false),
        (
            "duplicate head",
            "scattered union U\nunion clause U = C : int\n",
            "scattered union U\nunion clause U = D : int\n",
            "end U\n",
            false,
        ),
        (
            "duplicate union member",
            "scattered union U\nunion clause U = C : int\n",
            "union clause U = C : int\n",
            "end U\n",
            false,
        ),
        (
            "duplicate enum member",
            "scattered enum E\nenum clause E = A\n",
            "enum clause E = A\n",
            "end E\n",
            false,
        ),
        (
            "clause after end",
            "scattered union U\nunion clause U = C : int\nend U\n",
            "union clause U = D : int\n",
            "",
            false,
        ),
        (
            "new end breaks later clause",
            "scattered union U\nunion clause U = C : int\n",
            "end U\n",
            "union clause U = D : int\nend U\n",
            false,
        ),
        (
            "payload after head",
            "scattered union U\ntype Late = int\n",
            "union clause U = C : Late\n",
            "end U\n",
            false,
        ),
        (
            "payload before head",
            "type Early = int\nscattered union U\n",
            "union clause U = C : Early\n",
            "end U\n",
            true,
        ),
        ("inactive invalid state", "", "$ifdef OFF\nscattered union Empty\n$endif\n", "", true),
    ] {
        let main = format!("{before}function run() -> unit = helper()\n{after}");
        let helper = format!("{extension}val helper : unit -> unit\n");
        let (mut host, map, urls) =
            setup(&[("main.sail", &main), ("helper.sail", &helper)], &[0], vec![]);
        let a = snapshot(&host, &map);
        let edits = fixes(&a, &urls[0], &main, "helper()");
        assert_eq!(!edits.is_empty(), accepted, "{label}: {edits:?}");
        if let Some(edit) = edits.first() {
            let updated = apply(&main, edit);
            drop(a);
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(0), updated);
            host.apply_change(change);
            let a = snapshot(&host, &map);
            assert_clean(&a, 0);
            assert_clean(&a, 1);
        }
    }
}

#[test]
fn include_scattered_validation_uses_module_owners_and_preserves_existing_findings() {
    use base_db::module_graph::ModuleData;
    for (private, required, accepted) in
        [(false, true, true), (false, false, false), (true, true, false)]
    {
        let head = format!(
            "{}scattered union U\nunion clause U = C : int\n",
            if private { "private " } else { "" }
        );
        let main = "function run() -> unit = helper()\n";
        let helper = "union clause U = D : int\nval helper : unit -> unit\n";
        let modules = vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData {
                files: vec![FileId::from_raw(1)],
                requires: if required { vec![0] } else { vec![] },
            },
        ];
        let (host, map, urls) = setup(
            &[("head.sail", &head), ("main.sail", main), ("helper.sail", helper)],
            &[0, 1],
            modules,
        );
        let edits = fixes(&snapshot(&host, &map), &urls[1], main, "helper()");
        assert_eq!(
            !edits.is_empty(),
            accepted,
            "private={private}, required={required}: {edits:?}"
        );
    }
    let main = "scattered union ExistingError\nfunction run() -> unit = helper()\n";
    let (host, map, urls) =
        setup(&[("main.sail", main), ("helper.sail", "val helper : unit -> unit\n")], &[0], vec![]);
    assert_eq!(fixes(&snapshot(&host, &map), &urls[0], main, "helper()").len(), 1);
}

#[test]
fn include_constructor_privacy_retains_the_member_owner() {
    use base_db::module_graph::ModuleData;
    for (same_owner, name, accepted) in
        [(true, "Hidden", true), (false, "Hidden", false), (false, "Public", true)]
    {
        let main = format!("function run() -> U = {name}()\n");
        let modules = if same_owner {
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
        let (mut host, map, urls) = setup(
            &[
                ("defs.sail", "union U = { private Hidden : unit, Public : unit }\n"),
                ("main.sail", &main),
            ],
            &[1],
            modules,
        );
        let edits = fixes(&snapshot(&host, &map), &urls[1], &main, &format!("{name}()"));
        assert_eq!(edits.len(), usize::from(accepted), "same_owner={same_owner} {name}: {edits:?}");
        if accepted {
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(1), apply(&main, &edits[0]));
            host.apply_change(change);
            assert_clean(&snapshot(&host, &map), 1);
        }
    }
}

#[test]
fn include_private_enum_helpers_use_generated_visibility() {
    use base_db::module_graph::ModuleData;
    let definitions = "default Order dec\n$[enum_vector \"all_E\"]\nprivate enum E with label -> string = { A => \"A\", B => \"B\" }\n";
    for same_owner in [false, true] {
        for (name, expression) in
            [("E_of_num", "E_of_num(0)"), ("E_label", "E_label(E_of_num(0))"), ("all_E", "all_E")]
        {
            let main = format!("function run() -> unit = {{ let _x = {expression}; () }}\n");
            let modules = if same_owner {
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
            let (mut host, map, urls) =
                setup(&[("defs.sail", definitions), ("main.sail", &main)], &[1], modules);
            let edits = fixes(&snapshot(&host, &map), &urls[1], &main, name);
            let accepted = same_owner || name != "all_E";
            assert_eq!(
                edits.len(),
                usize::from(accepted),
                "same_owner={same_owner} name={name}: {edits:?}"
            );
            if accepted {
                let mut change = ide_db::apply_change::Change::new();
                change.change_file(FileId::from_raw(1), apply(&main, &edits[0]));
                host.apply_change(change);
                let a = snapshot(&host, &map);
                assert_clean(&a, 1);
                assert_clean(&a, 0);
            }
        }
    }
}

#[test]
fn include_scattered_types_and_members_applies_actual_declarations() {
    for (kind, clause, expression) in [
        ("enum", "enum clause E = Member\n", "Member"),
        ("union", "union clause E = Member : unit\n", "Member()"),
    ] {
        for whole_type in [false, true] {
            let (main, library, name) = if whole_type {
                (
                    "val consume : E -> unit\n".to_owned(),
                    format!("scattered {kind} E\n{clause}end E\n"),
                    "E",
                )
            } else {
                (
                    format!("scattered {kind} E\nfunction run() -> E = {expression}\nend E\n"),
                    clause.to_owned(),
                    "Member",
                )
            };
            let (mut host, map, urls) =
                setup(&[("main.sail", &main), ("library.sail", &library)], &[0], vec![]);
            let edits = fixes(&snapshot(&host, &map), &urls[0], &main, name);
            assert_eq!(edits.len(), 1, "{kind} whole_type={whole_type}: {edits:?}");
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(0), apply(&main, &edits[0]));
            host.apply_change(change);
            let a = snapshot(&host, &map);
            assert_clean(&a, 0);
            assert_clean(&a, 1);
        }
    }
}

#[test]
fn include_scattered_enum_vectors_requires_end_before_use() {
    for ended in [false, true] {
        let library = format!(
            "default Order dec\n$[enum_vector \"all_E\"]\nscattered enum E\nenum clause E = EA\n{}",
            if ended { "end E\n" } else { "" }
        );
        let main = "function values() -> unit = { let _x = all_E; () }\n";
        let (mut host, map, urls) =
            setup(&[("main.sail", main), ("library.sail", &library)], &[0], vec![]);
        let edits = fixes(&snapshot(&host, &map), &urls[0], main, "all_E");
        assert_eq!(edits.len(), usize::from(ended), "ended={ended}: {edits:?}");
        if ended {
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(FileId::from_raw(0), apply(main, &edits[0]));
            host.apply_change(change);
            let a = snapshot(&host, &map);
            assert_clean(&a, 0);
            assert_clean(&a, 1);
        }
    }
}

#[test]
fn instantiation_rhs_auto_include_preserves_the_replacement_namespace() {
    let outcome = "default Order dec\noutcome O : 'a -> unit with 'a : Type = { val convert : 'a -> string }\n";
    for (existing, instantiation, needle, type_rhs) in [
        (
            "val Thing : unit -> string\n",
            "instantiation O with 'a = Thing, convert = Thing\n",
            "Thing, convert",
            true,
        ),
        (
            "type Thing = unit\n",
            "instantiation O with 'a = Thing, convert = Thing\n",
            "Thing\n",
            false,
        ),
        (
            "val convert_impl : vector(2, unit) -> string\n",
            "instantiation O with 'a = vector(2, Thing), convert = convert_impl\n",
            "Thing)",
            true,
        ),
    ] {
        let main = format!("{outcome}{existing}{instantiation}");
        let (mut host, map, urls) = setup(
            &[
                ("main.sail", &main),
                ("types.sail", "type Thing = unit\n"),
                ("functions.sail", "val Thing : unit -> string\n"),
                ("values.sail", "let Thing = 0\n"),
            ],
            &[0],
            vec![],
        );
        let a = snapshot(&host, &map);
        let offset = main.rfind(needle).unwrap();
        let edits = fixes(&a, &urls[0], &main, &main[offset..]);
        assert_eq!(edits.len(), 1, "{main}{edits:?}");
        assert_eq!(
            edits[0].new_text,
            if type_rhs { "$include \"types.sail\"\n" } else { "$include \"functions.sail\"\n" }
        );
        let file = a.file(&urls[0]).unwrap();
        let mut items: Vec<_> = a
            .completions(FileId::from_raw(0), file.position_at(offset + 5))
            .unwrap()
            .into_iter()
            .filter(|item| item.import.is_some())
            .collect();
        assert_eq!(items.len(), 1, "{main}{items:?}");
        a.resolve_completion_item(&mut items[0]).unwrap();
        assert_eq!(items[0].additional_text_edits, edits);
        let updated = apply(&main, &edits[0]);
        drop(a);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(FileId::from_raw(0), &updated);
        host.apply_change(change);
        let a = snapshot(&host, &map);
        assert_clean(&a, 0);
        let file = a.file(&urls[0]).unwrap();
        let offset = updated.rfind(needle).unwrap();
        let targets = a.goto_definition(&urls[0], file.position_at(offset)).unwrap();
        assert_eq!(targets.len(), 1, "{targets:?}");
        assert_eq!(targets[0].file_id, Some(FileId::from_raw(if type_rhs { 1 } else { 2 })));
        assert!(fixes(&a, &urls[0], &updated, &updated[offset..]).is_empty());
    }
}
