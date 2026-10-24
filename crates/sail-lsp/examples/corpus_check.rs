//! Corpus check: validate sail-lsp diagnostics against a directory of .sail files.
//!
//! Runs the **same `Analysis::file_diagnostics` pipeline** that the LSP server uses,
//! but does not drive the LSP workspace loader or incremental document path.
//! Loader equivalence is covered separately by
//! `reload::tests::server_workspace_order_matches_the_probe`.
//!
//! This distinction matters: the probe has an explicit, stable
//! standard-library-first vector. Reconstructing `WorkspaceFiles` from a
//! hash map in the real server once made editor diagnostics disagree while
//! this probe remained green.
//!
//! ## Architecture alignment (RA)
//!
//! RA's `analysis-stats` CLI (`crates/rust-analyzer/src/cli/analysis_stats.rs:1310-1331`)
//! calls `analysis.full_diagnostics()` — the same function the LSP server invokes via
//! `fetch_native_diagnostics` (`crates/rust-analyzer/src/diagnostics.rs:298-305`).
//! There is no separate "corpus-check-only" diagnostic logic in RA.
//!
//! This example follows the same pattern: load workspace → `Analysis` → `file_diagnostics`.
//! No direct calls to `parse_text`, `ItemTree::build_from_cst`, `check_file`, or
//! `TopLevelEnv::from_cst`. All diagnostics come from `ide-diagnostics` handlers.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --example corpus_check -- /path/to/sail-riscv
//! cargo run --example corpus_check -- -v /path/to/sail-riscv        # verbose: per-file detail
//! cargo run --example corpus_check -- --json /path/to/sail-riscv    # JSON output for CI
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base_db::{FileId, Files};
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use url::Url;

// ---------------------------------------------------------------------------
// Workspace loading — shared pattern with analysis_stats.rs
// ---------------------------------------------------------------------------

/// Recursively discover .sail files under `root`, returning (url, path, text).
fn discover_sail_files(root: &Path) -> Vec<(Url, PathBuf, String)> {
    let mut files = Vec::new();
    fn walk(dir: &Path, files: &mut Vec<(Url, PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, files);
            } else if path.extension().is_some_and(|e| e == "sail") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(url) = Url::from_file_path(&path) {
                        files.push((url, path, text));
                    }
                }
            }
        }
    }
    walk(root, &mut files);
    files.sort_by(|a, b| a.1.cmp(&b.1));
    files
}

/// Find the Sail standard library directory.
fn find_sail_lib() -> Option<PathBuf> {
    if let Ok(lib) = std::env::var("SAIL_LIB") {
        let p = PathBuf::from(lib);
        if p.is_dir() {
            return Some(p);
        }
    }
    // Anchored on this crate's source directory, not the cwd: a relative
    // lookup silently finds nothing when the binary is run from anywhere
    // else, and a run without the prelude reports zero errors no matter
    // what is broken.
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/sail-lib"),
        PathBuf::from("crates/sail-lsp/data/sail-lib"),
        PathBuf::from("data/sail-lib"),
    ];
    candidates.into_iter().find(|c| c.is_dir()).and_then(|p| p.canonicalize().ok())
}

/// Load workspace: project files + sail-lib prelude into salsa database.
///
/// Returns `Analysis` with a populated `UrlMap` so that
/// `build_workspace_names()` can produce cross-file name sets for
/// false-positive suppression (mirrors RA's `CrateDefMap`).
/// A loaded database plus the project files registered in it.
type LoadedWorkspace = (Analysis, Files, Vec<(Url, FileId)>);

fn load_workspace(
    lib_files: &[(Url, PathBuf, String)],
    project_files: Vec<(Url, PathBuf, String)>,
) -> LoadedWorkspace {
    let mut all_sail_files: Vec<(Url, PathBuf, String)> = lib_files.to_vec();
    let project_start = all_sail_files.len();
    all_sail_files.extend(project_files);

    let mut host = AnalysisHost::new(None);
    let mut change = ide_db::apply_change::Change::new();
    let mut vfs = vfs::Vfs::default();

    // Register all files into VFS + salsa
    for (url, _path, text) in &all_sail_files {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    host.apply_change(std::mem::take(&mut change));
    let files = host.raw_database().files().clone();

    // Create WorkspaceFiles salsa singleton for cross-file type inference.
    {
        let db = host.raw_database_mut();
        let file_texts: Vec<base_db::FileText> = all_sail_files
            .iter()
            .filter_map(|(url, _, _)| {
                let fid = vfs.lookup_file_id_by_url(url)?;
                files.file_text(fid)
            })
            .collect();
        hir::def_query::set_workspace_files(db, file_texts);
    }

    // Build UrlMap (needed for cross-file workspace names)
    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    let mut url_fids = Vec::new();
    for (url, _path, _text) in &all_sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_to_fid.insert(url.clone(), fid);
            fid_to_url.insert(fid, url.clone());
            url_fids.push((url.clone(), fid));
        }
    }
    let url_map = UrlMap::new(url_to_fid.clone(), fid_to_url);
    // With the include graph the server builds, by the same function. A
    // probe that has none is a different program: an `$include` cycle is
    // reported only by a holder of the whole graph, so that diagnostic was
    // one no corpus run could ever produce.
    let paths: Vec<(Url, PathBuf)> =
        all_sail_files.iter().map(|(url, path, _)| (url.clone(), path.clone())).collect();
    let graph = ide::analysis::build_include_graph(
        host.raw_database(),
        &paths,
        &url_to_fid,
        find_sail_lib().as_deref(),
    );
    let analysis = Analysis::with_include_graph(
        host.raw_database().clone(),
        files.clone(),
        url_map,
        std::sync::Arc::new(graph),
    );

    // Only return project file ids (skip sail-lib) for diagnostics iteration
    let project_url_fids = url_fids.split_off(project_start);

    (analysis, files, project_url_fids)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// The file at `idx` plus every sibling it pulls in with `$include "..."`.
///
/// A quoted include is a path relative to the including file, so a program
/// that splits itself across files is still one program. Only angle-bracket
/// includes name the bundled library.
fn with_local_includes(
    files: &[(Url, PathBuf, String)],
    idx: usize,
) -> Vec<(Url, PathBuf, String)> {
    let mut out = vec![files[idx].clone()];
    let mut queue = vec![idx];
    let mut seen = vec![idx];
    while let Some(i) = queue.pop() {
        for line in files[i].2.lines() {
            let Some(rest) = line.trim_start().strip_prefix("$include") else { continue };
            let Some(path) = rest.trim().strip_prefix('"').and_then(|r| r.split('"').next()) else {
                continue;
            };
            let target = match files[i].1.parent() {
                Some(dir) => dir.join(path),
                None => PathBuf::from(path),
            };
            let Some(j) = files.iter().position(|(_, p, _)| *p == target) else { continue };
            if seen.contains(&j) {
                continue;
            }
            seen.push(j);
            queue.push(j);
            out.push(files[j].clone());
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let json = args.iter().any(|a| a == "--json");
    // Upstream's test directories hold unrelated standalone programs, several
    // of which reuse the same names. Loading a directory as one workspace
    // lets them resolve against each other, which is neither what upstream
    // checks nor what an editor would see.
    let isolated = args.iter().any(|a| a == "--isolated");
    let root = args.iter().filter(|a| !a.starts_with('-')).nth(1).unwrap_or_else(|| {
        eprintln!("Usage: corpus_check [--verbose|-v] [--json] /path/to/sail-project");
        std::process::exit(1);
    });
    let root = PathBuf::from(root).canonicalize().unwrap_or_else(|err| {
        eprintln!("Cannot open project directory {root}: {err}");
        std::process::exit(1);
    });

    // Use large stack for deep type inference
    let handle = std::thread::Builder::new()
        .name("corpus-check".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run_check(&root, verbose, json, isolated))
        .expect("failed to spawn check thread");
    let exit_code = handle.join().unwrap();
    std::process::exit(exit_code);
}

fn run_check(root: &Path, verbose: bool, json: bool, isolated: bool) -> i32 {
    let t0 = std::time::Instant::now();

    let lib_files = match find_sail_lib() {
        Some(lib_dir) => {
            let lib_files = discover_sail_files(&lib_dir);
            eprintln!("Loaded {} sail-lib files from {}", lib_files.len(), lib_dir.display());
            lib_files
        }
        None => {
            eprintln!(
                "Warning: sail-lib not found. Set SAIL_LIB or ensure crates/sail-lsp/data/sail-lib exists."
            );
            Vec::new()
        }
    };

    eprintln!("Loading workspace from {}...", root.display());
    let project_files = discover_sail_files(root);
    let n_files = project_files.len();
    // In isolated mode each file gets its own database, so the analysis is
    // rebuilt per file inside the loop below.
    let whole: Option<LoadedWorkspace> =
        (!isolated).then(|| load_workspace(&lib_files, project_files.clone()));
    eprintln!("Loaded {} project files in {} ms", n_files, t0.elapsed().as_millis());

    // Run diagnostics through the same pipeline as the LSP server:
    //   Analysis::file_diagnostics()
    //     → ide_diagnostics::full_diagnostics()
    //       → syntax_diagnostics() + semantic_diagnostics()
    //
    // This matches RA's approach:
    //   analysis_stats.rs:1331  → analysis.full_diagnostics(config, resolve, file_id)
    //   main_loop.rs:670,686   → fetch_native_diagnostics → analysis.syntax/semantic_diagnostics
    //   Both go through ide-diagnostics handlers.
    let config = ide_diagnostics::DiagnosticsConfig::new();

    let mut total_errors = 0usize;
    let mut total_warnings = 0usize;
    let mut total_info = 0usize;
    let mut by_code: HashMap<String, usize> = HashMap::new();
    let mut per_file: Vec<(String, Vec<ide_diagnostics::Diagnostic>)> = Vec::new();

    for idx in 0..n_files {
        // Isolated: prelude + this one file. Whole-workspace: everything.
        let owned = whole
            .is_none()
            .then(|| load_workspace(&lib_files, with_local_includes(&project_files, idx)));
        let (analysis, url, fid) = match (&whole, &owned) {
            (Some((analysis, _, ids)), _) => {
                let (url, fid) = &ids[idx];
                (analysis, url.clone(), *fid)
            }
            (None, Some((analysis, _, ids))) => {
                let (url, fid) = &ids[0];
                (analysis, url.clone(), *fid)
            }
            (None, None) => unreachable!(),
        };
        let url = &url;
        if idx % 20 == 0 {
            let rel = url_relative(url, root);
            eprintln!("  [{}/{}] {}", idx, n_files, rel);
        }

        let diags = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            analysis.file_diagnostics(fid, &config).expect("no other thread is here to cancel it")
        }));

        match diags {
            Ok(diags) => {
                for d in &diags {
                    match d.severity {
                        ide_diagnostics::Severity::Error => total_errors += 1,
                        ide_diagnostics::Severity::Warning
                        | ide_diagnostics::Severity::WeakWarning => total_warnings += 1,
                        ide_diagnostics::Severity::Information
                        | ide_diagnostics::Severity::Hint => total_info += 1,
                    }
                    *by_code.entry(d.code.as_str().to_string()).or_default() += 1;
                }
                if !diags.is_empty() {
                    let rel = url_relative(url, root);
                    per_file.push((rel, diags));
                }
            }
            Err(_) => {
                let rel = url_relative(url, root);
                eprintln!("  CRASH: {}", rel);
                total_errors += 1;
                *by_code.entry("CRASH".to_string()).or_default() += 1;
            }
        }
    }

    let t_total = t0.elapsed();

    if json {
        print_json(&per_file, n_files, total_errors, total_warnings, total_info, &by_code, t_total);
    } else {
        print_text(
            &per_file,
            n_files,
            total_errors,
            total_warnings,
            total_info,
            &by_code,
            t_total,
            verbose,
        );
    }

    if total_errors > 0 {
        1
    } else {
        0
    }
}

// ---------------------------------------------------------------------------
// Output formatting
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn print_text(
    per_file: &[(String, Vec<ide_diagnostics::Diagnostic>)],
    n_files: usize,
    total_errors: usize,
    total_warnings: usize,
    total_info: usize,
    by_code: &HashMap<String, usize>,
    elapsed: std::time::Duration,
    verbose: bool,
) {
    // Per-file diagnostics
    for (rel, diags) in per_file {
        for d in diags {
            let sev = match d.severity {
                ide_diagnostics::Severity::Error => "error",
                ide_diagnostics::Severity::Warning => "warning",
                ide_diagnostics::Severity::WeakWarning | ide_diagnostics::Severity::Hint => "hint",
                ide_diagnostics::Severity::Information => "info",
            };
            println!("{}:{}: [{}] {}", rel, u32::from(d.range.range.start()), sev, d.message);
        }
    }

    // Summary
    println!();
    println!("============================================================");
    println!("CORPUS CHECK: {} files, {} ms", n_files, elapsed.as_millis());
    println!("============================================================");
    println!("  Errors:   {}", total_errors);
    println!("  Warnings: {}", total_warnings);
    println!("  Info:     {}", total_info);

    if verbose || !by_code.is_empty() {
        println!();
        println!("  By diagnostic code:");
        let mut sorted: Vec<_> = by_code.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (code, count) in &sorted {
            println!("    {:<40} {}", code, count);
        }
    }
}

fn print_json(
    per_file: &[(String, Vec<ide_diagnostics::Diagnostic>)],
    n_files: usize,
    total_errors: usize,
    total_warnings: usize,
    total_info: usize,
    by_code: &HashMap<String, usize>,
    elapsed: std::time::Duration,
) {
    // Minimal JSON output for CI diffing
    println!("{{");
    println!("  \"files\": {},", n_files);
    println!("  \"errors\": {},", total_errors);
    println!("  \"warnings\": {},", total_warnings);
    println!("  \"info\": {},", total_info);
    println!("  \"elapsed_ms\": {},", elapsed.as_millis());
    println!("  \"by_code\": {{");
    let mut sorted: Vec<_> = by_code.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    for (i, (code, count)) in sorted.iter().enumerate() {
        let comma = if i + 1 < sorted.len() { "," } else { "" };
        println!("    \"{}\": {}{}", code, count, comma);
    }
    println!("  }},");
    println!("  \"diagnostics\": [");
    let total_diags: usize = per_file.iter().map(|(_, ds)| ds.len()).sum();
    let mut diag_idx = 0;
    for (rel, diags) in per_file {
        for d in diags {
            diag_idx += 1;
            let comma = if diag_idx < total_diags { "," } else { "" };
            let sev = match d.severity {
                ide_diagnostics::Severity::Error => "error",
                ide_diagnostics::Severity::Warning => "warning",
                ide_diagnostics::Severity::WeakWarning | ide_diagnostics::Severity::Hint => "hint",
                ide_diagnostics::Severity::Information => "info",
            };
            // Escape message for JSON
            let msg = d.message.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n");
            println!(
                "    {{\"file\": \"{}\", \"offset\": {}, \"severity\": \"{}\", \"code\": \"{}\", \"message\": \"{}\"}}{}",
                rel, u32::from(d.range.range.start()), sev, d.code.as_str(), msg, comma
            );
        }
    }
    println!("  ]");
    println!("}}");
}

fn url_relative(url: &Url, root: &Path) -> String {
    let path = url.path();
    let root_str = root.to_string_lossy();
    if let Some(rest) = path.strip_prefix(root_str.as_ref()) {
        rest.trim_start_matches('/').to_string()
    } else {
        path.rsplit('/').take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/")
    }
}
