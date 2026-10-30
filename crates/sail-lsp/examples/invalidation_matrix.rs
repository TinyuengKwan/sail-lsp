//! Actual Salsa execution counts for ordered-project edits. Run with --release.
//! The output is JSON; assertions check semantic freshness and unrelated reuse.
use base_db::{
    module_graph::{ModuleData, ModuleGraph},
    preprocess::SourceProject,
    FileId,
};
use ide_db::{apply_change::Change, root_database::RootDatabase};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

fn id(n: u32) -> FileId {
    FileId::from_raw(n)
}
fn project() -> SourceProject {
    let roots = vec![id(0), id(1), id(2)];
    SourceProject {
        roots: roots.clone(),
        paths: ["main", "caller", "unrelated", "types", "alternative"]
            .iter()
            .enumerate()
            .map(|(i, name)| (format!("/matrix/{name}.sail").into(), id(i as u32)))
            .collect(),
        modules: ModuleGraph {
            modules: vec![ModuleData { files: roots, requires: vec![] }],
            globals: vec![],
        },
        ..Default::default()
    }
}
fn infer(db: &RootDatabase) -> BTreeMap<String, Arc<hir::TypeCheckResult>> {
    let mut results = BTreeMap::new();
    for n in 0..3 {
        let file = db.files().file_text(id(n)).unwrap();
        if base_db::preprocess::project_membership(db, id(n)) == Some(false) {
            continue;
        }
        for &def in hir::def_query::file_def_with_body_ids(db, file) {
            results.insert(def.name(db).to_string(), hir::ty_query::infer(db, def).0.clone());
        }
    }
    results
}
fn main() {
    let mut report = BTreeMap::new();
    for mode in [
        "comment",
        "leading_comment",
        "body",
        "signature",
        "include",
        "include_content",
        "manifest",
    ] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut db = RootDatabase::with_event_callback(Some(Box::new(move |e| {
            if let salsa::EventKind::WillExecute { database_key } = e.kind {
                captured.lock().unwrap().push(database_key);
            }
        })));
        let original = "$include \"types.sail\"\nfunction f(x : int) -> Word = 1\n";
        let mut setup = Change::new();
        for (n, text) in [
            original,
            "function caller(x : int) -> int = f(x)\n",
            "function unrelated() -> int = 3\n",
            "type Word = int\n",
            "type Word = bool\n",
        ]
        .iter()
        .enumerate()
        {
            setup.change_file(id(n as u32), *text);
        }
        setup.set_source_project(project());
        db.apply_change(setup);
        let before = infer(&db);
        assert_eq!(before.len(), 3);
        assert!(before.values().all(|r| r.type_mismatches.is_empty()));
        events.lock().unwrap().clear();
        let warm = infer(&db);
        assert!(before.iter().all(|(k, v)| Arc::ptr_eq(v, &warm[k])));
        assert!(events.lock().unwrap().is_empty(), "warm queries executed");
        let mut edit = Change::new();
        match mode {
            "comment" => edit.change_file(id(0), original.replace("= 1", "= /* note */ 1")),
            "leading_comment" => edit.change_file(id(0), format!("// prefix\n{original}")),
            "include_content" => edit.change_file(id(3), "type Word = bool\n"),
            "body" => edit.change_file(id(0), original.replace("= 1", "= 2")),
            "signature" => {
                edit.change_file(id(0), original.replace("-> Word = 1", "-> bool = true"))
            }
            "include" => {
                edit.change_file(id(0), original.replace("types.sail", "alternative.sail"))
            }
            "manifest" => {
                let mut project = project();
                project.roots.remove(0);
                project.modules.modules[0].files.remove(0);
                edit.set_source_project(project);
            }
            _ => unreachable!(),
        }
        let start = Instant::now();
        db.apply_change(edit);
        let after = infer(&db);
        let elapsed = start.elapsed().as_micros();
        assert!(
            Arc::ptr_eq(&before["unrelated"], &after["unrelated"]),
            "{mode}: unrelated inference invalidated"
        );
        let changed: Vec<_> = after
            .iter()
            .filter(|(k, v)| !Arc::ptr_eq(&before[*k], v))
            .map(|(k, _)| k.clone())
            .collect();
        if matches!(mode, "leading_comment" | "comment") {
            assert!(changed.is_empty());
        }
        if mode == "body" {
            assert_eq!(changed, ["f"]);
        }
        if matches!(mode, "signature" | "include" | "include_content") {
            assert!(!after["caller"].type_mismatches.is_empty(), "{mode}: stale caller");
        }
        if mode == "manifest" {
            assert!(!after.contains_key("f"));
            assert!(!Arc::ptr_eq(&before["caller"], &after["caller"]));
            assert!(after["caller"].inference_diagnostics().iter().any(|diagnostic| matches!(
                diagnostic, hir::InferenceDiagnostic::NoOverloading { name, no_function: true, .. }
                    if name == "f"
            )), "manifest removal retained a stale function binding");
        }
        let keys = std::mem::take(&mut *events.lock().unwrap());
        let executions = salsa::attach(&db, || {
            let mut counts = BTreeMap::<String, usize>::new();
            for key in keys {
                let key = format!("{key:?}");
                let name = key.split('(').next().unwrap().to_owned();
                *counts.entry(name).or_default() += 1;
            }
            counts
        });
        assert_eq!(
            executions.get("infer").copied().unwrap_or(0),
            changed.len(),
            "{mode}: execution count disagrees with result replacement"
        );
        report.insert(mode, serde_json::json!({"elapsed_us":elapsed, "reinferred":changed, "executions":executions, "unrelated_reused":true, "warm_executions":0}));
    }
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}
