//! Measure actual retained inference Arcs, query recomputation and process heap.
//! cargo run --release -p sail-lsp --example query_cache_stats -- [files=64] [bodies=8]
#![allow(clippy::print_stdout)]
use base_db::FileId;
use ide_db::{apply_change::Change, cache::QueryCacheCapacities, root_database::RootDatabase};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Instant,
};

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let files: usize = args.get(1).map_or(64, |n| n.parse().expect("files"));
    let bodies: usize = args.get(2).map_or(8, |n| n.parse().expect("bodies"));
    assert!((1..=4096).contains(&files) && (1..=256).contains(&bodies));
    let mut reports = Vec::new();
    for capacity in [0, 32] {
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let mut db = RootDatabase::with_event_callback(Some(Box::new(move |e| {
            if let salsa::EventKind::WillExecute { database_key } = e.kind {
                captured.lock().unwrap().push(database_key);
            }
        })));
        QueryCacheCapacities {
            parse: capacity,
            environment: capacity,
            inference: capacity,
            bodies: if capacity == 0 { 512 } else { capacity },
        }
        .apply(&mut db);
        let mut change = Change::new();
        for f in 0..files {
            let text = (0..bodies)
                .map(|b| format!("function f{f}_{b}(x : int) -> int = x + {b}\n"))
                .collect::<String>();
            change.change_file(FileId::from_raw(f as u32), text);
        }
        db.apply_change(change);
        let visit = |db: &RootDatabase| {
            let mut weak = Vec::new();
            for f in 0..files {
                let ft = db.files().file_text(FileId::from_raw(f as u32)).unwrap();
                for &id in hir::def_query::file_def_with_body_ids(db, ft) {
                    let result = hir::ty_query::infer(db, id);
                    assert!(result.0.type_mismatches.is_empty());
                    weak.push(Arc::downgrade(&result.0));
                }
            }
            weak
        };
        let start = Instant::now();
        let retained = visit(&db);
        let cold_us = start.elapsed().as_micros();
        events.lock().unwrap().clear();
        visit(&db);
        assert!(events.lock().unwrap().is_empty(), "warm workload recomputed before eviction");
        let heap_before_mb = profile::MemoryUsage::now().allocated.megabytes();
        use salsa::Database;
        db.trigger_lru_eviction();
        let alive = retained.iter().filter(|weak| weak.strong_count() != 0).count();
        if capacity == 0 {
            assert_eq!(alive, files * bodies);
        } else {
            assert!(
                alive <= capacity,
                "inference results retained beyond configured capacity: {alive}"
            );
        }
        let heap_after_mb = profile::MemoryUsage::now().allocated.megabytes();
        events.lock().unwrap().clear();
        let start = Instant::now();
        visit(&db);
        let revisit_us = start.elapsed().as_micros();
        let executions = salsa::attach(&db, || {
            let mut counts = BTreeMap::<String, usize>::new();
            for key in std::mem::take(&mut *events.lock().unwrap()) {
                *counts
                    .entry(format!("{key:?}").split('(').next().unwrap().to_owned())
                    .or_default() += 1;
            }
            counts
        });
        if capacity == 0 {
            assert!(executions.is_empty());
        } else {
            assert!(executions.get("infer").copied().unwrap_or(0) >= files * bodies - capacity);
        }
        reports.push(serde_json::json!({"capacity":capacity, "files":files, "bodiesPerFile":bodies,
            "inferenceResultsAliveAfterEviction":alive, "coldUs":cold_us, "revisitUs":revisit_us,
            "heapBeforeMiB":heap_before_mb,"heapAfterMiB":heap_after_mb,"revisitExecutions":executions}));
    }
    println!("{}", serde_json::to_string_pretty(&serde_json::json!({"schemaVersion":1,
        "scope":"synthetic workload; Weak counts track query-owned inference results, not total retained bytes; heap estimate excludes some native/mapped allocations",
        "runs":reports})).unwrap());
}
