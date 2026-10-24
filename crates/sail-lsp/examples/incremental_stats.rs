//! Reproducible query invalidation and latency probe, without an external corpus.
//! cargo run --release --example incremental_stats -- [files=160] [edits=32] [body|signature] [--profile] [--modules] [--preprocess]

use std::sync::Arc;
use std::time::Instant;

#[path = "support/incremental_profile.rs"]
mod incremental_profile;

use base_db::{FileId, Files};
use ide::analysis::AnalysisHost;

fn infer_all(
    host: &AnalysisHost,
    files: &Files,
    fids: &[FileId],
) -> Vec<Arc<hir::infer::TypeCheckResult>> {
    let db = host.raw_database();
    fids.iter()
        .flat_map(|&fid| {
            let ft = files.file_text(fid).unwrap();
            hir::def_query::file_def_with_body_ids(db, ft)
                .iter()
                .map(|&id| hir::ty_query::infer(db, id).0.clone())
        })
        .collect()
}

fn source(i: usize, value: usize, signature: bool, preprocess: bool) -> String {
    let callee = if signature && i == 1 { 0 } else { i };
    let ret = if signature && i == 0 && value % 2 == 1 { "bool" } else { "int" };
    let value = if signature { 0 } else { value };
    let text = format!("val f{i} : int -> {ret}\nfunction f{i}(x) = x + {value:08}\nfunction g{i}(x : int) -> int = f{callee}(x)\n");
    if preprocess {
        format!("$ifdef FEATURE_IMPLICITS\n{text}$endif\n")
    } else {
        text
    }
}

fn percentile(samples: &mut [u128], percent: usize) -> u128 {
    samples.sort_unstable();
    samples[(samples.len() - 1) * percent / 100]
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let profiler = incremental_profile::Profiler::install(args.iter().any(|a| a == "--profile"));
    let count: usize = args.get(1).map_or(160, |s| s.parse().expect("file count"));
    let edits: usize = args.get(2).map_or(32, |s| s.parse().expect("edit count"));
    let mode = args.get(3).map(String::as_str).unwrap_or("body");
    assert!(matches!(mode, "body" | "signature"));
    let signature = mode == "signature";
    let preprocess = args.iter().any(|a| a == "--preprocess");
    assert!(count > 0 && count <= 100_000 && edits > 0 && edits < 100_000_000);
    let mut host = AnalysisHost::default();
    let mut files = Files::default();
    let fids: Vec<_> = (0..count).map(|i| FileId::from_raw(i as u32)).collect();
    for (i, &fid) in fids.iter().enumerate() {
        let text = source(i, 0, signature, preprocess);
        if preprocess {
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(fid, text);
            host.apply_change(change);
        } else {
            files.set_file_text(host.raw_database_mut(), fid, &text);
        }
    }
    if preprocess {
        files = host.raw_database().files().clone();
    }
    hir::def_query::set_workspace_files(
        host.raw_database_mut(),
        fids.iter().map(|&fid| files.file_text(fid).unwrap()).collect(),
    );
    if args.iter().any(|a| a == "--modules") {
        use base_db::module_graph::{set_module_graph, ModuleData, ModuleGraph};
        set_module_graph(
            host.raw_database_mut(),
            ModuleGraph {
                modules: fids
                    .iter()
                    .enumerate()
                    .map(|(i, &file)| ModuleData {
                        files: vec![file],
                        requires: if signature && i == 1 { vec![0] } else { vec![] },
                    })
                    .collect(),
                globals: vec![],
            },
        );
    }
    if preprocess {
        let modules = base_db::module_graph::WorkspaceModules::try_get(host.raw_database())
            .map(|g| g.graph(host.raw_database()).clone())
            .unwrap_or_default();
        let mut change = ide_db::apply_change::Change::new();
        change.set_source_project(base_db::preprocess::SourceProject {
            roots: fids.clone(),
            paths: fids
                .iter()
                .map(|id| (format!("/bench/{}.sail", id.index()).into(), *id))
                .collect(),
            modules,
            ..Default::default()
        });
        host.apply_change(change);
    }
    if let Some(p) = &profiler {
        p.reset();
    }
    let start = Instant::now();
    let mut previous = infer_all(&host, &files, &fids);
    println!("METRIC:incremental_cold:{}:us", start.elapsed().as_micros());
    println!("METRIC:incremental_callables:{}:count", previous.len());
    if let Some(p) = &profiler {
        p.report("cold");
        p.reset();
    }
    let mut warm = Vec::new();
    let mut edited = Vec::new();
    let mut snapshots = Vec::new();
    let mut reinferred = Vec::new();
    for revision in 1..=edits {
        let start = Instant::now();
        let cached = infer_all(&host, &files, &fids);
        warm.push(start.elapsed().as_micros());
        assert!(cached.iter().zip(&previous).all(|(a, b)| Arc::ptr_eq(a, b)));
        let start = Instant::now();
        drop(host.db_snapshot());
        snapshots.push(start.elapsed().as_nanos());

        let start = Instant::now();
        if preprocess {
            let mut change = ide_db::apply_change::Change::new();
            change.change_file(fids[0], source(0, revision, signature, true));
            host.apply_change(change);
        } else {
            files.set_file_text(
                host.raw_database_mut(),
                fids[0],
                &source(0, revision, signature, false),
            );
        }
        let next = infer_all(&host, &files, &fids);
        edited.push(start.elapsed().as_micros());
        assert_eq!(previous.len(), next.len());
        let recomputed: Vec<_> = previous
            .iter()
            .zip(&next)
            .enumerate()
            .filter_map(|(i, (a, b))| (!Arc::ptr_eq(a, b)).then_some(i))
            .collect();
        if signature {
            let expected = if count > 1 { vec![0, 1, 3] } else { vec![0, 1] };
            assert_eq!(
                recomputed, expected,
                "signature edit invalidated unrelated callables or retained stale results"
            );
            if count > 1 {
                assert_eq!(
                    next[3].type_mismatches.is_empty(),
                    revision % 2 == 0,
                    "dependent caller diagnostics did not track the new return type"
                );
            }
        } else {
            // The body LRU can evict the other callable in this file.
            assert!(recomputed.contains(&0), "the edited function retained stale inference");
            assert!(
                recomputed.iter().all(|&i| i < 2),
                "body edit invalidated another file: {recomputed:?}"
            );
        }
        reinferred.push(recomputed.len());
        previous = next;
    }
    if let Some(p) = &profiler {
        p.report("edits");
    }
    for (name, samples, unit) in [
        ("incremental_warm", &mut warm, "us"),
        (
            if signature { "incremental_signature_edit" } else { "incremental_body_edit" },
            &mut edited,
            "us",
        ),
        ("incremental_snapshot", &mut snapshots, "ns"),
    ] {
        for p in [50, 95, 99] {
            println!("METRIC:{name}_p{p}:{}:{unit}", percentile(samples, p));
        }
    }
    println!("METRIC:incremental_reinferred_min:{}:count", reinferred.iter().min().unwrap());
    println!("METRIC:incremental_reinferred_max:{}:count", reinferred.iter().max().unwrap());
}
