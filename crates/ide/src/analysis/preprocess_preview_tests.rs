use super::hierarchy::tests::{setup, snapshot};
use base_db::FileId;

#[test]
fn preview_preserves_order_conditions_incoming_symbols_and_repeated_visits() {
    let main = "let before = 1\n$include \"shared.sail\"\n$define LATER\n$include \"shared.sail\"\nlet after = 3\n";
    let shared = "$ifdef LATER\nlet second = 2\n$else\nlet first = 1\n$endif\n";
    let (host, map, urls) = setup(&[("main.sail", main), ("shared.sail", shared)], &[0], vec![]);
    let analysis = snapshot(&host, &map);
    let preview = analysis.expand_includes(&urls[0]).unwrap().unwrap();
    let text = &preview.expansion;
    let before = text.find("let before").unwrap();
    let first = text.find("let first").unwrap();
    let second = text.find("let second").unwrap();
    let after = text.find("let after").unwrap();
    assert!(before < first && first < second && second < after, "{text}");
    assert_eq!(text.matches("let first").count(), 1);
    assert_eq!(text.matches("let second").count(), 1);
    assert!(text.contains("Inactive source omitted"));
    assert!(text.contains("Symbols added after directive: LATER"));
    for mapping in &preview.mappings {
        assert!(base_db::range_end(mapping.generated) <= text.len());
        let source = if mapping.original.file_id == FileId::from_raw(0) { main } else { shared };
        let original = &source[base_db::range_start(mapping.original.range)
            ..base_db::range_end(mapping.original.range)];
        if mapping.kind == "source" {
            assert_eq!(
                &text[base_db::range_start(mapping.generated)
                    ..base_db::range_end(mapping.generated)],
                original
            );
        }
    }
    let contexts = analysis.include_contexts(&urls[1]).unwrap();
    assert_eq!(contexts.len(), 2);
    let selected = analysis.with_preferred_views([(FileId::from_raw(1), contexts[1].0)].into());
    let preview = selected.expand_includes(&urls[1]).unwrap().unwrap();
    assert!(preview.expansion.contains("let second"), "{}", preview.expansion);
    assert!(!preview.expansion.contains("let first"));
    assert!(!preview.expansion.contains("let before"));
    assert!(preview
        .expansion
        .lines()
        .filter_map(|line| line.split_once("; symbols: "))
        .any(|(_, symbols)| symbols.split(", ").any(|symbol| symbol == "LATER")));
    assert_eq!(preview.context.unwrap(), contexts[1].1);
}

#[test]
fn preview_uses_current_buffers_and_source_positions_after_trivia_changes() {
    let main = "$include \"lib.sail\"\nlet after = 1\n";
    let child = "/*😀*/ let original = 2\n";
    let (mut host, map, urls) = setup(&[("main.sail", main), ("lib.sail", child)], &[0], vec![]);
    let old = snapshot(&host, &map).expand_includes(&urls[0]).unwrap().unwrap();
    assert!(old.expansion.contains("original"));
    let updated = format!("// new leading trivia\n{}", child.replace("original", "updated"));
    let mut change = ide_db::apply_change::Change::new();
    change.change_file(FileId::from_raw(1), &updated);
    host.apply_change(change);
    let preview = snapshot(&host, &map).expand_includes(&urls[0]).unwrap().unwrap();
    assert!(preview.expansion.contains("updated"));
    assert!(!preview.expansion.contains("original"));
    let mapping = preview
        .mappings
        .iter()
        .find(|mapping| mapping.original.file_id == FileId::from_raw(1) && mapping.kind == "source")
        .unwrap();
    assert_eq!(
        &preview.expansion
            [base_db::range_start(mapping.generated)..base_db::range_end(mapping.generated)],
        &updated
    );
}

#[test]
fn preview_tracks_symbols_from_prior_roots_and_resets_distinct_project_configurations() {
    use base_db::module_graph::ModuleData;
    use base_db::preprocess::{ModuleOptions, WorkspaceSourceProject};
    let sources = [
        ("config.sail","$define FROM_PREVIOUS\n$target_set native c\n"),
        ("main.sail","$include \"shared.sail\"\n"),
        ("shared.sail","$ifdef FROM_PREVIOUS\nlet previous = 1\n$else\nlet reset = 2\n$endif\n$iftarget native\nlet native = 3\n$endif\n"),
    ];
    let (mut host, map, urls) = setup(
        &sources,
        &[0, 1],
        vec![
            ModuleData { files: vec![FileId::from_raw(0)], requires: vec![] },
            ModuleData { files: vec![FileId::from_raw(1)], requires: vec![] },
        ],
    );
    let input = WorkspaceSourceProject::try_get(host.raw_database()).unwrap();
    let mut project = input.project(host.raw_database()).clone();
    let options = ModuleOptions {
        project: Some("/project/a.sail_project".into()),
        target: Some("c".into()),
        ..Default::default()
    };
    project.module_options = [(0, options.clone()), (1, options)].into();
    let mut change = ide_db::apply_change::Change::new();
    change.set_source_project(project.clone());
    host.apply_change(change);
    let preview = snapshot(&host, &map).expand_includes(&urls[2]).unwrap().unwrap();
    assert!(
        preview.expansion.contains("let previous") && preview.expansion.contains("let native"),
        "{}",
        preview.expansion
    );
    assert!(!preview.expansion.contains("let reset"));
    let options = project.module_options.get_mut(&1).unwrap();
    options.project = Some("/project/b.sail_project".into());
    options.target = Some("lem".into());
    let mut change = ide_db::apply_change::Change::new();
    change.set_source_project(project);
    host.apply_change(change);
    let preview = snapshot(&host, &map).expand_includes(&urls[2]).unwrap().unwrap();
    assert!(preview.expansion.contains("let reset"), "{}", preview.expansion);
    assert!(
        !preview.expansion.contains("let previous") && !preview.expansion.contains("let native")
    );
    assert!(preview.expansion.contains("Target: lem"));
}

#[test]
fn preview_exposes_active_missing_includes_and_cycles_without_expanding_inactive_paths() {
    let main = "$ifdef NEVER\n$include \"inactive.sail\"\n$endif\n$include \"missing.sail\"\n$include \"cycle.sail\"\n";
    let (host, map, urls) =
        setup(&[("main.sail", main), ("cycle.sail", "$include \"main.sail\"\n")], &[0], vec![]);
    let preview = snapshot(&host, &map).expand_includes(&urls[0]).unwrap().unwrap();
    assert!(
        preview.expansion.contains("Include missing: /project/missing.sail"),
        "{}",
        preview.expansion
    );
    assert!(!preview.expansion.contains("Include missing: /project/inactive.sail"));
    assert!(preview.expansion.contains("Include cycle/depth limit:"));
}
