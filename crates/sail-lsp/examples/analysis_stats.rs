//! Analysis statistics and benchmarks for sail-lsp.
//!
//! Fully loads a workspace via the salsa-backed Analysis pipeline,
//! then measures individual feature latencies. Reports metrics in
//! `METRIC:name:value:unit` format (RA convention).
//!
//! Usage:
//!   cargo run --release --example analysis_stats -- /path/to/sail-riscv
//!   cargo run --release --example analysis_stats -- -v /path/to/sail-riscv

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use base_db::{FileId, Files};
use hir_def::callgraph::SourceFileInfo as _;
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use ide_db::FileDb;
use url::Url;

fn report_metric(name: &str, value: u64, unit: &str) {
    // RA format: `METRIC:name:value:unit`
    println!("METRIC:{name}:{value}:{unit}");
}

// ---------------------------------------------------------------------------
// Workspace loading — shared pattern with corpus_check.rs
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
    let candidates =
        [PathBuf::from("crates/sail-lsp/data/sail-lib"), PathBuf::from("data/sail-lib")];
    candidates.into_iter().find(|c| c.is_dir()).and_then(|p| p.canonicalize().ok())
}

/// Load workspace: project files + sail-lib prelude into salsa database.
///
/// Returns `AnalysisHost` + `Files` + project file IDs.
/// Builds a `UrlMap` for cross-file workspace name resolution.
fn load_workspace(root: &Path) -> (AnalysisHost, Files, Vec<(Url, FileId)>) {
    let mut all_sail_files = Vec::new();

    // Load sail-lib first (prelude types)
    if let Some(lib_dir) = find_sail_lib() {
        let lib_files = discover_sail_files(&lib_dir);
        all_sail_files.extend(lib_files);
    }

    // Then project files
    let project_files = discover_sail_files(root);
    let project_start = all_sail_files.len();
    all_sail_files.extend(project_files);

    let mut host = AnalysisHost::new(None);
    let mut files = Files::default();
    let mut vfs = vfs::Vfs::default();

    for (url, _path, text) in &all_sail_files {
        let fid = vfs.file_id_for_url(url);
        files.set_file_contents(fid, text, base_db::Durability::HIGH);
    }
    files.apply_pending(host.raw_database_mut());

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
        base_db::WorkspaceFiles::new(db, file_texts);
    }

    let mut url_fids = Vec::new();
    for (url, _path, _text) in &all_sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_fids.push((url.clone(), fid));
        }
    }

    // Only return project file ids (skip sail-lib) for iteration
    let project_url_fids = url_fids.split_off(project_start);

    (host, files, project_url_fids)
}

/// Build an Analysis with UrlMap from host + files + url_fids.
fn make_analysis(host: &AnalysisHost, files: &Files, url_fids: &[(Url, FileId)]) -> Analysis {
    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    for (url, fid) in url_fids {
        url_to_fid.insert(url.clone(), *fid);
        fid_to_url.insert(*fid, url.clone());
    }
    let url_map = UrlMap::new(url_to_fid, fid_to_url);
    Analysis::with_url_map(host.raw_database().clone(), files.clone(), url_map)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let run_ide = args.iter().any(|a| a == "--run-all-ide-things");
    let root =
        args.iter().filter(|a| !a.starts_with('-')).nth(1).expect(
            "Usage: analysis_stats [--verbose] [--run-all-ide-things] /path/to/sail-project",
        );
    let root = PathBuf::from(root);

    println!("=======================================================");
    println!("sail-lsp analysis-stats");
    println!("=======================================================");

    // 1. Workspace loading
    let t0 = Instant::now();
    let (host, files, url_fids) = load_workspace(&root);
    let t_load = t0.elapsed();
    let n_files = url_fids.len();
    println!("\nWorkspace: {} files", n_files);
    println!("Load + salsa inputs: {:>7} ms", t_load.as_millis());
    report_metric("workspace_load", t_load.as_millis() as u64, "ms");

    let analysis = make_analysis(&host, &files, &url_fids);

    // 2. Parse all files (prime parse cache)
    let t0 = Instant::now();
    let mut total_lines = 0usize;
    let mut parse_error_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(sf) = analysis.file_by_id(*fid) {
            total_lines += sf.text().lines().count();
            let ft = files.file_text(*fid).unwrap();
            let parsed = syntax::parse_query::parse_file(analysis.db(), ft);
            if let Some(errs) = &parsed.errors {
                parse_error_count += errs.len();
            }
        }
    }
    let t_parse = t0.elapsed();
    println!("Total lines:         {:>7}", total_lines);
    println!("Parse all:           {:>7} ms ({} errors)", t_parse.as_millis(), parse_error_count);
    report_metric("parse_all", t_parse.as_millis() as u64, "ms");
    report_metric("total_lines", total_lines as u64, "lines");

    // 3. ItemTree for all files
    let t0 = Instant::now();
    let mut item_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            if let Some(tree) = hir_def::def_query::file_item_tree(analysis.db(), ft).as_ref() {
                item_count += tree.top_level_items().len();
            }
        }
    }
    let t_items = t0.elapsed();
    println!("ItemTree all:        {:>7} ms ({} items)", t_items.as_millis(), item_count);
    report_metric("item_tree_all", t_items.as_millis() as u64, "ms");

    // 3b. Body lowering for all files (CST → HIR Body+SourceMap)
    let t0 = Instant::now();
    let mut body_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            if let Some(bodies) = hir_def::def_query::callable_bodies(analysis.db(), ft).as_ref() {
                body_count += bodies.0.entries().len();
            }
        }
    }
    let t_bodies = t0.elapsed();
    println!("Body lowering:       {:>7} ms ({} bodies)", t_bodies.as_millis(), body_count);
    report_metric("body_lowering", t_bodies.as_millis() as u64, "ms");
    report_metric("bodies", body_count as u64, "count");

    // 4. Type inference for all callables
    let t0 = Instant::now();
    let mut callable_count = 0usize;
    let mut type_error_count = 0usize;
    let mut slow_callables: Vec<(String, u128)> = Vec::new();
    for (url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let ids = hir_def::def_query::file_def_with_body_ids(analysis.db(), ft);
            for &id in ids {
                callable_count += 1;
                let tc = Instant::now();
                let tcr = hir_ty::query::infer(analysis.db(), id);
                let elapsed = tc.elapsed().as_millis();
                type_error_count += tcr.0.inference_diagnostics().len();
                type_error_count += tcr.0.type_mismatches.len();
                if elapsed > 50 {
                    let name = format!(
                        "{}#{}",
                        url.path().rsplit('/').next().unwrap_or("?"),
                        callable_count
                    );
                    slow_callables.push((name, elapsed));
                }
            }
        }
    }
    let t_infer = t0.elapsed();
    println!(
        "Infer all:           {:>7} ms ({} callables, {} type errors)",
        t_infer.as_millis(),
        callable_count,
        type_error_count
    );
    report_metric("infer_all", t_infer.as_millis() as u64, "ms");
    report_metric("callables", callable_count as u64, "count");
    // Distribution histogram
    let mut all_times: Vec<u128> = Vec::new();
    // Re-run to collect all times (cheap — salsa cached)
    for (_url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let ids = hir_def::def_query::file_def_with_body_ids(analysis.db(), ft);
            for &id in ids {
                let tc = Instant::now();
                let _ = hir_ty::query::infer(analysis.db(), id);
                all_times.push(tc.elapsed().as_micros());
            }
        }
    }
    all_times.sort();
    let p50 = all_times.get(all_times.len() / 2).copied().unwrap_or(0);
    let p90 = all_times.get(all_times.len() * 9 / 10).copied().unwrap_or(0);
    let p99 = all_times.get(all_times.len() * 99 / 100).copied().unwrap_or(0);
    let max = all_times.last().copied().unwrap_or(0);
    println!("\n  Infer latency distribution:");
    println!("    p50: {:>7} µs", p50);
    println!("    p90: {:>7} µs", p90);
    println!("    p99: {:>7} µs", p99);
    println!("    max: {:>7} µs", max);

    if !slow_callables.is_empty() {
        slow_callables.sort_by_key(|b| std::cmp::Reverse(b.1));
        println!("\n  Slowest callables (>50ms):");
        for (name, ms) in slow_callables.iter().take(20) {
            println!("    {:>7} ms  {}", ms, name);
        }
    }

    // 5. Diagnostics for all files (same pipeline as LSP + corpus_check)
    let t0 = Instant::now();
    let config = ide_diagnostics::DiagnosticsConfig::new();
    let mut diag_count = 0usize;
    for (_url, fid) in &url_fids {
        let diags = analysis.file_diagnostics(*fid, &config);
        diag_count += diags.len();
    }
    let t_diag = t0.elapsed();
    println!("Diagnostics all:     {:>7} ms ({} diagnostics)", t_diag.as_millis(), diag_count);
    report_metric("diagnostics_all", t_diag.as_millis() as u64, "ms");

    // 5b. Inlay hints for all files
    let t0 = Instant::now();
    let mut hint_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(sf) = analysis.file_by_id(*fid) {
            let text_len = sf.text().len();
            let range = base_db::text_range(0, text_len);
            if let Ok(hints) = analysis.inlay_hints(*fid, range) {
                hint_count += hints.len();
            }
        }
    }
    let t_hints = t0.elapsed();
    println!("Inlay hints all:     {:>7} ms ({} hints)", t_hints.as_millis(), hint_count);
    report_metric("inlay_hints_all", t_hints.as_millis() as u64, "ms");
    report_metric("inlay_hints", hint_count as u64, "count");

    // 5c. Semantic tokens (sample first 10 files)
    let sample_hl = url_fids.iter().take(10).collect::<Vec<_>>();
    let t0 = Instant::now();
    let mut token_count = 0usize;
    for (_url, fid) in &sample_hl {
        if let Ok(tokens) = analysis.semantic_tokens(*fid) {
            token_count += tokens.data.len();
        }
    }
    let t_tokens = t0.elapsed();
    let avg_tokens =
        if !sample_hl.is_empty() { t_tokens.as_micros() / sample_hl.len() as u128 } else { 0 };
    println!(
        "Semantic tokens (avg of {}): {:>5} µs ({} tokens total)",
        sample_hl.len(),
        avg_tokens,
        token_count
    );
    report_metric("semantic_tokens_avg", avg_tokens as u64, "µs");

    // 5d. Goto-definition (sample first 10 files)
    let t0 = Instant::now();
    let mut goto_count = 0usize;
    for (_url, fid) in &sample_hl {
        // Goto-def at line 3, col 5 (arbitrary — same as hover sample)
        let pos = ide_db::LineCol { line: 3, col: 5 };
        if let Some(sf) = analysis.file_by_id(*fid) {
            if let Some(url) = analysis.url_for_file_id(*fid) {
                let _ = ide::goto_definition::goto_definition_semantic(&sf, pos, url);
                goto_count += 1;
            }
        }
    }
    let t_goto = t0.elapsed();
    let avg_goto = if goto_count > 0 { t_goto.as_micros() / goto_count as u128 } else { 0 };
    println!("Goto-def (avg of {}): {:>7} µs", goto_count, avg_goto);
    report_metric("goto_def_avg", avg_goto as u64, "µs");

    // 6. Workspace symbol index
    let t0 = Instant::now();
    let all_salsa = analysis.all_salsa_files();
    let mut index = ide_db::workspace_index::SymbolIndex::new();
    for (url, sf) in &all_salsa {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            index.add_file(url, sf as &dyn FileDb);
        }));
    }
    let symbol_count = index.all_entries().count();
    let t_index = t0.elapsed();
    println!("Symbol index:        {:>7} ms ({} symbols)", t_index.as_millis(), symbol_count);
    report_metric("symbol_index", t_index.as_millis() as u64, "ms");
    report_metric("symbols", symbol_count as u64, "count");

    // 7. Per-file hover (sample first 10 files)
    let sample = url_fids.iter().take(10).collect::<Vec<_>>();
    let t0 = Instant::now();
    let mut hover_count = 0usize;
    for (_url, fid) in &sample {
        // Hover at line 3, column 5 (arbitrary)
        let pos = ide_db::LineCol { line: 3, col: 5 };
        let _ = analysis.hover(*fid, pos);
        hover_count += 1;
    }
    let t_hover = t0.elapsed();
    let avg_hover = if hover_count > 0 { t_hover.as_micros() / hover_count as u128 } else { 0 };
    println!("Hover (avg of {}):  {:>7} µs", hover_count, avg_hover);
    report_metric("hover_avg", avg_hover as u64, "µs");

    // 8. Run all IDE things (optional, `--run-all-ide-things` flag).
    if run_ide {
        run_ide_things(&analysis, &url_fids, &files);
    }

    // 9. Parallel prime_caches benchmark (fresh db)
    {
        let (host2, files2, _) = load_workspace(&root);
        let ncpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let cancel = hir_ty::CancellationToken::new();

        let t0 = Instant::now();
        ide_db::prime_caches::parallel_prime_caches(
            host2.raw_database(),
            &files2,
            ncpus,
            &|_| {},
            &cancel,
        );
        let t_prime = t0.elapsed();
        println!("prime_caches ({} threads): {:>5} ms", ncpus, t_prime.as_millis());
        report_metric("prime_caches_parallel", t_prime.as_millis() as u64, "ms");

        // Sequential comparison
        let (host3, files3, _) = load_workspace(&root);
        let t0 = Instant::now();
        ide_db::prime_caches::parallel_prime_caches(
            host3.raw_database(),
            &files3,
            1,
            &|_| {},
            &cancel,
        );
        let t_seq = t0.elapsed();
        println!("prime_caches (1 thread):  {:>5} ms", t_seq.as_millis());
        report_metric("prime_caches_sequential", t_seq.as_millis() as u64, "ms");

        if t_seq.as_millis() > 0 {
            let speedup = t_seq.as_millis() as f64 / t_prime.as_millis().max(1) as f64;
            println!("Speedup:                  {:.1}x", speedup);
        }
    }

    // 9. Summary
    let t_total = t_load + t_parse + t_items + t_bodies + t_infer + t_diag + t_hints + t_index;
    println!("\n-------------------------------------------------------");
    println!("Total analysis:      {:>7} ms", t_total.as_millis());
    report_metric("total_analysis", t_total.as_millis() as u64, "ms");

    if verbose {
        println!("\n--- Per-file details ---");
        for (url, fid) in &url_fids {
            let path: &str = url.path();
            let short = path
                .rsplit('/')
                .take(3)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/");
            if let Some(sf) = analysis.file_by_id(*fid) {
                let lines = sf.text().lines().count();
                let ft = files.file_text(*fid).unwrap();
                let ids = hir_def::def_query::file_def_with_body_ids(analysis.db(), ft);
                println!("  {:<50} {:>5} lines  {:>3} callables", short, lines, ids.len());
            }
        }
    }

    println!("\n=======================================================");
}

// ---------------------------------------------------------------------------
// --run-all-ide-things: full IDE feature benchmark
// ---------------------------------------------------------------------------

/// Run all IDE features on every file, with per-feature timing.
///
/// Runs diagnostics + inlay_hints + annotations + semantic_tokens +
/// completions + hover in six loops.
fn run_ide_things(analysis: &Analysis, url_fids: &[(Url, FileId)], _files: &Files) {
    let n = url_fids.len();
    let t_all = Instant::now();

    println!("\n--- IDE features (all {} files) ---\n", n);

    // 1. Diagnostics
    let config = ide_diagnostics::DiagnosticsConfig::new();
    let t0 = Instant::now();
    let mut diag_count = 0usize;
    for (_url, fid) in url_fids {
        let diags = analysis.file_diagnostics(*fid, &config);
        diag_count += diags.len();
    }
    let t_diag = t0.elapsed();
    println!("  Diagnostics:       {:>7} ms ({} diagnostics)", t_diag.as_millis(), diag_count);

    // 2. Inlay hints
    let t0 = Instant::now();
    let mut hint_count = 0usize;
    for (_url, fid) in url_fids {
        if let Some(sf) = analysis.file_by_id(*fid) {
            let text_len = sf.text().len();
            let range = base_db::text_range(0, text_len);
            if let Ok(hints) = analysis.inlay_hints(*fid, range) {
                hint_count += hints.len();
            }
        }
    }
    let t_hints = t0.elapsed();
    println!("  Inlay hints:       {:>7} ms ({} hints)", t_hints.as_millis(), hint_count);

    // 3. Semantic tokens
    let t0 = Instant::now();
    let mut token_count = 0usize;
    for (_url, fid) in url_fids {
        if let Ok(tokens) = analysis.semantic_tokens(*fid) {
            token_count += tokens.data.len();
        }
    }
    let t_tokens = t0.elapsed();
    println!("  Semantic tokens:   {:>7} ms ({} tokens)", t_tokens.as_millis(), token_count);

    // 4. Completions (at line 3, col 5 — exercises full completion pipeline)
    let t0 = Instant::now();
    let mut completion_count = 0usize;
    let pos = ide_db::LineCol { line: 3, col: 5 };
    for (_url, fid) in url_fids {
        if let Ok(items) = analysis.completions(*fid, pos) {
            completion_count += items.len();
        }
    }
    let t_completions = t0.elapsed();
    println!(
        "  Completions:       {:>7} ms ({} items)",
        t_completions.as_millis(),
        completion_count
    );

    // 5. Code lenses (annotations)
    let t0 = Instant::now();
    let mut lens_count = 0usize;
    for (_url, fid) in url_fids {
        if let Ok(lenses) = analysis.code_lenses(*fid) {
            lens_count += lenses.len();
        }
    }
    let t_lenses = t0.elapsed();
    println!("  Code lenses:       {:>7} ms ({} annotations)", t_lenses.as_millis(), lens_count);

    // 6. Hover
    let t0 = Instant::now();
    for (_url, fid) in url_fids {
        let _ = analysis.hover(*fid, pos);
    }
    let t_hover = t0.elapsed();
    println!("  Hover:             {:>7} ms", t_hover.as_millis());

    let t_total = t_all.elapsed();
    println!("  -------");
    println!("  Total IDE:         {:>7} ms", t_total.as_millis());
    report_metric("ide_all", t_total.as_millis() as u64, "ms");
}
