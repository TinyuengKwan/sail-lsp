//! Integrated benchmarks for sail-lsp.
//! Run with: RUN_SLOW_BENCHES=1 cargo test -p sail-lsp --lib -- benchmarks

#![allow(clippy::print_stderr)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use base_db::{Durability, FileId, FileText, Files};
use ide::analysis::{AnalysisHost, UrlMap};
use ide_db::test_utils::TestFile;
use ide_diagnostics::DiagnosticsConfig;
use url::Url;


#[salsa::db]
#[derive(Default, Clone)]
struct BenchDb {
    storage: salsa::Storage<Self>,
}

#[salsa::db]
impl salsa::Database for BenchDb {}


fn bench_sail_dir() -> Option<PathBuf> {
    // Allow override via SAIL_RISCV_DIR; otherwise fall back to a well-known
    // relative path that CI / local dev typically have.
    if let Ok(dir) = std::env::var("SAIL_RISCV_DIR") {
        let p = PathBuf::from(dir);
        if p.is_dir() {
            return Some(p);
        }
    }
    let candidates = ["../../../sail-riscv", "../../sail-riscv", "../sail-riscv"];
    for c in candidates {
        let p = PathBuf::from(c);
        if p.is_dir() {
            return Some(p);
        }
    }
    None
}

fn discover_sail_files(root: &Path) -> Vec<(Url, PathBuf, String)> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<(Url, PathBuf, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "sail") {
                if let Ok(text) = std::fs::read_to_string(&path) {
                    if let Ok(url) = Url::from_file_path(&path) {
                        out.push((url, path, text));
                    }
                }
            }
        }
    }
    walk(root, &mut out);
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

fn collect_sail_files_flat(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "sail") {
                out.push(path);
            }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

/// Find `from` in `what`, replace with `to`, return byte offset.
fn patch(what: &mut String, from: &str, to: &str) -> usize {
    let idx = what.find(from).unwrap();
    *what = what.replacen(from, to, 1);
    idx
}

fn load_workspace(root: &Path) -> (AnalysisHost, Files, Vec<(Url, FileId)>) {
    let sail_files = discover_sail_files(root);
    let mut host = AnalysisHost::new(None);
    let mut files = Files::default();
    let mut vfs = vfs::Vfs::default();

    for (url, _path, text) in &sail_files {
        let fid = vfs.file_id_for_url(url);
        files.set_file_contents(fid, text, Durability::HIGH);
    }
    files.apply_pending(host.raw_database_mut());

    let mut url_fids = Vec::new();
    for (url, _path, _text) in &sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_fids.push((url.clone(), fid));
        }
    }
    (host, files, url_fids)
}

/// Like `load_workspace` but also returns a UrlMap for Analysis methods that
/// need URL<->FileId resolution (semantic_tokens, completions, etc.).
fn load_workspace_with_url_map(root: &Path) -> (AnalysisHost, Files, Vec<(Url, FileId)>, UrlMap) {
    let sail_files = discover_sail_files(root);
    let mut host = AnalysisHost::new(None);
    let mut files = Files::default();
    let mut vfs = vfs::Vfs::default();

    for (url, _path, text) in &sail_files {
        let fid = vfs.file_id_for_url(url);
        files.set_file_contents(fid, text, Durability::HIGH);
    }
    files.apply_pending(host.raw_database_mut());

    let mut url_fids = Vec::new();
    for (url, _path, _text) in &sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_fids.push((url.clone(), fid));
        }
    }

    let (u2f, f2u) = vfs.url_map_snapshot();
    let url_map = UrlMap::new(u2f, f2u);
    (host, files, url_fids, url_map)
}


fn run_diagnostics_single(source: &str) -> (f64, usize, f64, usize) {
    let db = BenchDb::default();
    let ft = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
    base_db::WorkspaceFiles::new(&db, vec![ft]);
    let config = DiagnosticsConfig::new();
    let file = TestFile::new(source);

    let t0 = Instant::now();
    let syntax_diags = ide_diagnostics::syntax_diagnostics(&db, &config, &file, ft);
    let parse_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let parse_count = syntax_diags.len();

    let t1 = Instant::now();
    let resolve = ide_db::assists::AssistResolveStrategy::All;
    let sema_diags = ide_diagnostics::semantic_diagnostics(&db, &config, &resolve, &file, ft, None);
    let sema_ms = t1.elapsed().as_secs_f64() * 1000.0;
    let sema_count = sema_diags.len();

    (parse_ms, parse_count, sema_ms, sema_count)
}

fn collect_diagnostic_codes(source: &str) -> Vec<String> {
    let db = BenchDb::default();
    let ft = FileText::new(&db, Arc::from(source), FileId::from_raw(0));
    base_db::WorkspaceFiles::new(&db, vec![ft]);
    let config = DiagnosticsConfig::new();
    let file = TestFile::new(source);

    let resolve = ide_db::assists::AssistResolveStrategy::All;
    let all_diags = ide_diagnostics::full_diagnostics(&db, &config, &resolve, &file, ft, None);
    let mut codes: Vec<String> = all_diags.iter().map(|d| d.code.as_str().to_string()).collect();
    codes.sort();
    codes.dedup();
    codes
}

#[test]
fn benchmark_single_file_diagnostics() {
    if std::env::var("RUN_SLOW_BENCHES").is_err() {
        return;
    }

    let dir = bench_sail_dir().expect(
        "RUN_SLOW_BENCHES set but no sail-riscv directory found. \
         Set SAIL_RISCV_DIR=/path/to/sail-riscv",
    );

    let sail_files = collect_sail_files_flat(&dir);
    assert!(!sail_files.is_empty(), "No .sail files found in {}", dir.display());

    // Build workspace context for cross-file resolution
    eprintln!("[bench] Building workspace context from {} files...", sail_files.len());
    let all_test_files: Vec<TestFile> = sail_files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|t| TestFile::new(&t)))
        .collect();
    // Workspace context is now built by salsa tracked query
    // (workspace_context). No global static to set.
    let _ = &all_test_files; // used by test file selection below

    // Pick representative files (small, medium, large)
    let mut by_size: Vec<(usize, &Path)> = sail_files
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok().map(|s| (s.lines().count(), p.as_path())))
        .collect();
    by_size.sort_by_key(|(lines, _)| *lines);

    let small = by_size.iter().find(|(l, _)| *l >= 50 && *l <= 120);
    let medium = by_size.iter().find(|(l, _)| *l >= 200 && *l <= 400);
    let large = by_size.iter().find(|(l, _)| *l >= 500);

    let test_files: Vec<(&str, &Path, usize)> = [
        small.map(|(l, p)| ("small", *p, *l)),
        medium.map(|(l, p)| ("medium", *p, *l)),
        large.map(|(l, p)| ("large", *p, *l)),
    ]
    .into_iter()
    .flatten()
    .collect();

    assert!(!test_files.is_empty(), "No suitable test files found");

    let injections: Vec<(&str, &str, &[&str])> = vec![
        ("type error: int assigned to bool",
         "\nval __bench_err : unit -> bool\nfunction __bench_err() = 42\n",
         &["type-error"]),
        ("unresolved identifier",
         "\nfunction __bench_err2() = nonexistent_var + 1\n",
         &["unresolved-ident"]),
        ("duplicate definition",
         "\ntype __bench_dup = int\ntype __bench_dup = bool\n",
         &["duplicate-definition"]),
        ("parse error: missing =",
         "\nfunction __bench_parse_err() 42\n",
         &["syntax-error"]),
        ("parse error: unmatched paren",
         "\nfunction __bench_paren() = (1 + 2\n",
         &["syntax-error", "bracket-mismatch"]),
        ("unused variable",
         "\nfunction __bench_unused() = { let unused_x = 1; () }\n",
         &["unused-variable"]),
        ("unreachable code",
         "\nfunction __bench_unreach() = { return (); let dead = 1; dead }\n",
         &["unreachable-code"]),
        ("mismatched arg count",
         "\nval __bench_f : (int, int) -> int\nfunction __bench_f(x, y) = x + y\nfunction __bench_caller() = __bench_f(1)\n",
         &["mismatched-arg-count"]),
    ];

    eprintln!("=== Per-File Diagnostic Benchmark: Latency + Accuracy ===\n");

    // Baseline
    eprintln!("-- Baseline (no errors) --\n");
    eprintln!(
        "  {:16} {:>6}  {:>12}  {:>12}  {:>8}",
        "File", "Lines", "Parse(ms)", "Sema(ms)", "Diags"
    );
    for (label, path, lines) in &test_files {
        let source = std::fs::read_to_string(path).unwrap();
        let (parse_ms, parse_count, sema_ms, sema_count) = run_diagnostics_single(&source);
        eprintln!(
            "  {:16} {:>6}  {:>12.2}  {:>12.2}  {:>8}",
            label,
            lines,
            parse_ms,
            sema_ms,
            parse_count + sema_count
        );
    }

    // Error injection tests
    let mut total_tests = 0u32;
    let mut detected = 0u32;

    for (_label, path, lines) in &test_files {
        let base_source = std::fs::read_to_string(path).unwrap();
        for (inj_name, inj_code, expected_codes) in &injections {
            let modified = format!("{}{}", base_source, inj_code);
            let all_codes = collect_diagnostic_codes(&modified);
            let found =
                expected_codes.iter().any(|exp| all_codes.iter().any(|code| code.contains(exp)));
            total_tests += 1;
            if found {
                detected += 1;
            }
            if !found {
                eprintln!(
                    "  MISSED: {} in {} ({} lines): expected {:?}, got {:?}",
                    inj_name,
                    path.file_name().unwrap().to_string_lossy(),
                    lines,
                    expected_codes,
                    all_codes
                );
            }
        }
    }

    let accuracy = if total_tests > 0 { detected as f64 / total_tests as f64 * 100.0 } else { 0.0 };
    eprintln!("\n=== Summary: {}/{} detected ({:.1}%) ===", detected, total_tests, accuracy);
}


#[test]
fn benchmark_workspace_diagnostics() {
    if std::env::var("RUN_SLOW_BENCHES").is_err() {
        return;
    }

    let dir = bench_sail_dir().expect(
        "RUN_SLOW_BENCHES set but no sail-riscv directory found. \
         Set SAIL_RISCV_DIR=/path/to/sail-riscv",
    );

    eprintln!("=== Workspace-Mode Diagnostic Benchmark ===\n");

    let _cpu = profile::cpu_span();
    eprintln!("[bench] Loading workspace...");
    let t_load = Instant::now();
    let (host, files, url_fids) = load_workspace(&dir);
    let analysis = host.analysis(&files);
    let ws_names = analysis.build_workspace_names();

    // Build WorkspaceContext for cross-file inference
    let all_test_files: Vec<TestFile> = url_fids
        .iter()
        .filter_map(|(url, _fid)| {
            let path = url.to_file_path().ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            Some(TestFile::new(&text))
        })
        .collect();
    // Workspace context is now a salsa tracked query — no global to set.
    let _ = &all_test_files;

    let load_ms = t_load.elapsed().as_secs_f64() * 1000.0;
    eprintln!(
        "[bench] Workspace: {} files, {:.0} ms, {} functions, {} constructors\n",
        url_fids.len(),
        load_ms,
        ws_names.function_names.len(),
        ws_names.constructor_names.len(),
    );

    // Pick representative files
    let mut file_info: Vec<(String, usize, String)> = url_fids
        .iter()
        .filter_map(|(url, _fid)| {
            let path = url.to_file_path().ok()?;
            let text = std::fs::read_to_string(&path).ok()?;
            let lines = text.lines().count();
            let name = path.file_name()?.to_string_lossy().to_string();
            Some((name, lines, text))
        })
        .collect();
    file_info.sort_by_key(|(_, lines, _)| *lines);

    let small = file_info.iter().find(|(_, l, _)| *l >= 50 && *l <= 120);
    let medium = file_info.iter().find(|(_, l, _)| *l >= 200 && *l <= 400);
    let large = file_info.iter().find(|(_, l, _)| *l >= 500);

    let targets: Vec<(&str, &str, usize, &str)> = [
        small.map(|(n, l, t)| ("small", n.as_str(), *l, t.as_str())),
        medium.map(|(n, l, t)| ("medium", n.as_str(), *l, t.as_str())),
        large.map(|(n, l, t)| ("large", n.as_str(), *l, t.as_str())),
    ]
    .into_iter()
    .flatten()
    .collect();

    // Baseline
    for (label, name, lines, text) in &targets {
        let db = BenchDb::default();
        let ft = FileText::new(&db, Arc::from(*text), FileId::from_raw(999));
        base_db::WorkspaceFiles::new(&db, vec![ft]);
        let file = TestFile::new(text);
        let config = DiagnosticsConfig::new();
        let t0 = Instant::now();
        let resolve = ide_db::assists::AssistResolveStrategy::All;
        let diags =
            ide_diagnostics::full_diagnostics(&db, &config, &resolve, &file, ft, Some(&ws_names));
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "  {:10} {:>22} {:>6} lines  {:>7.2} ms  {} diags",
            label,
            name,
            lines,
            ms,
            diags.len()
        );
    }

    // Error injections
    let injections: Vec<(&str, &str, &[&str])> = vec![
        ("type error: int->bool",
         "\nval __bench_err : unit -> bool\nfunction __bench_err() = 42\n",
         &["type-error"]),
        ("unresolved identifier",
         "\nfunction __bench_err2() = xyzzy_nonexistent + 1\n",
         &["unresolved-ident"]),
        ("duplicate definition",
         "\ntype __bench_dup = int\ntype __bench_dup = bool\n",
         &["duplicate-definition"]),
        ("parse error: missing =",
         "\nfunction __bench_parse() 42\n",
         &["syntax-error"]),
        ("parse error: unmatched paren",
         "\nfunction __bench_paren() = (1 + 2\n",
         &["syntax-error"]),
        ("unused variable",
         "\nfunction __bench_unused() = { let unused_xyz = 1; () }\n",
         &["unused-variable"]),
        ("unreachable code",
         "\nfunction __bench_unreach() = { return (); let dead = 1; dead }\n",
         &["unreachable-code"]),
        ("mismatched arg count",
         "\nval __bench_f : (int, int) -> int\nfunction __bench_f(x, y) = x + y\nfunction __bench_caller() = __bench_f(1)\n",
         &["mismatched-arg-count"]),
    ];

    let mut total_tests = 0u32;
    let mut detected = 0u32;

    for (_label, _name, _lines, original_text) in &targets {
        for (inj_name, inj_code, expected_codes) in &injections {
            let modified = format!("{}{}", original_text, inj_code);
            let db = BenchDb::default();
            let ft = FileText::new(&db, Arc::from(modified.as_str()), FileId::from_raw(999));
            base_db::WorkspaceFiles::new(&db, vec![ft]);
            let file = TestFile::new(&modified);
            let config = DiagnosticsConfig::new();

            let resolve = ide_db::assists::AssistResolveStrategy::All;
            let diags = ide_diagnostics::full_diagnostics(
                &db,
                &config,
                &resolve,
                &file,
                ft,
                Some(&ws_names),
            );

            let mut unique_codes: Vec<String> =
                diags.iter().map(|d| d.code.as_str().to_string()).collect();
            unique_codes.sort();
            unique_codes.dedup();

            let found =
                expected_codes.iter().any(|exp| unique_codes.iter().any(|code| code.contains(exp)));

            total_tests += 1;
            if found {
                detected += 1;
            }
            if !found {
                eprintln!(
                    "  MISSED: {}: expected {:?}, got {:?}",
                    inj_name, expected_codes, unique_codes
                );
            }
        }
    }

    let accuracy = if total_tests > 0 { detected as f64 / total_tests as f64 * 100.0 } else { 0.0 };
    eprintln!("\n=== Summary: {}/{} detected ({:.1}%) ===", detected, total_tests, accuracy);
}


/// Make a body-only edit: find first number literal and change it.
fn make_body_edit(source: &str) -> String {
    if let Some(pos) = source.find("= 0x") {
        make_body_edit_at(source, pos + 10)
    } else if let Some(pos) = source.find("= 0") {
        let mut s = source.to_string();
        s.replace_range(pos + 2..pos + 3, "99");
        s
    } else if let Some(pos) = source.find("= 1") {
        let mut s = source.to_string();
        s.replace_range(pos + 2..pos + 3, "99");
        s
    } else {
        format!("{}\n// body-edit\n", source)
    }
}

fn make_body_edit_at(source: &str, start: usize) -> String {
    if let Some(pos) = source[start..].find("= 0").or_else(|| source[start..].find("= 1")) {
        let abs = start + pos;
        let mut s = source.to_string();
        s.replace_range(abs + 2..abs + 3, "99");
        s
    } else {
        format!("{}\n// body-edit\n", source)
    }
}

fn diag_one_file(
    host: &AnalysisHost,
    files: &Files,
    fid: FileId,
    config: &DiagnosticsConfig,
    ws_names: &ide_diagnostics::WorkspaceNames,
) -> Vec<ide_diagnostics::Diagnostic> {
    let Some(ft) = files.file_text(fid) else {
        return Vec::new();
    };
    let sf = ide_db::root_database::SalsaFile::new(host.raw_database(), ft);
    let resolve = ide_db::assists::AssistResolveStrategy::All;
    ide_diagnostics::full_diagnostics(
        host.raw_database(),
        config,
        &resolve,
        &sf,
        ft,
        Some(ws_names),
    )
}

fn pick_small_target_file(
    url_fids: &[(Url, FileId)],
    files: &Files,
    db: &ide_db::root_database::RootDatabase,
) -> Option<(Url, FileId, String, usize, usize)> {
    for (url, fid) in url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let text_arc = ft.text(db);
            let text: &str = &text_arc;
            let lines = text.lines().count();
            if lines >= 30 && lines <= 120 {
                let ids = hir_def::def_query::file_def_with_body_ids(db, ft);
                if ids.len() >= 2 && ids.len() <= 8 {
                    return Some((url.clone(), *fid, text.to_string(), lines, ids.len()));
                }
            }
        }
    }
    for (url, fid) in url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let text_arc = ft.text(db);
            let text: &str = &text_arc;
            let lines = text.lines().count();
            let ids = hir_def::def_query::file_def_with_body_ids(db, ft);
            if ids.len() >= 2 && lines <= 200 {
                return Some((url.clone(), *fid, text.to_string(), lines, ids.len()));
            }
        }
    }
    None
}

#[test]
fn benchmark_incremental_diagnostics() {
    if std::env::var("RUN_SLOW_BENCHES").is_err() {
        return;
    }

    let dir = bench_sail_dir().expect(
        "RUN_SLOW_BENCHES set but no sail-riscv directory found. \
         Set SAIL_RISCV_DIR=/path/to/sail-riscv",
    );

    eprintln!("=== Incremental Diagnostic Benchmark ===\n");

    let _cpu = profile::cpu_span();

    // 1. Load workspace + warm caches
    eprintln!("[bench] Loading workspace...");
    let t0 = Instant::now();
    let (mut host, mut files, url_fids) = load_workspace(&dir);
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Workspace: {} files, {:.0} ms load", url_fids.len(), load_ms);

    eprintln!("[bench] Warming caches...");
    let t0 = Instant::now();
    let config = DiagnosticsConfig::new();
    let analysis = host.analysis(&files);
    let ws_names = analysis.build_workspace_names();

    {
        let _t = stdx::timeit("cache_warm");
        for (_url, fid) in &url_fids {
            if let Some(ft) = files.file_text(*fid) {
                let ids = hir_def::def_query::file_def_with_body_ids(host.raw_database(), ft);
                for &id in ids {
                    let _ = hir_ty::query::infer(host.raw_database(), id);
                }
            }
        }
    }
    let warm_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("Cache warm: {:.0} ms\n", warm_ms);

    // Pick a small file
    let target = pick_small_target_file(&url_fids, &files, host.raw_database());
    let (target_url, target_fid, target_text, target_lines, target_callables) = match target {
        Some(t) => t,
        None => {
            eprintln!("No suitable target file found");
            return;
        }
    };
    let target_name = target_url.path().rsplit('/').next().unwrap_or("?");
    eprintln!("Target: {} ({} lines, {} callables)\n", target_name, target_lines, target_callables);

    // Scenario 1: No-op edit (content-hash dedup)
    eprintln!("-- Scenario 1: No-op edit --");
    {
        let t0 = Instant::now();
        files.set_file_text(host.raw_database_mut(), target_fid, &target_text);
        let edit_us = t0.elapsed().as_micros();

        let t1 = Instant::now();
        let diags = diag_one_file(&host, &files, target_fid, &config, &ws_names);
        let diag_us = t1.elapsed().as_micros();

        eprintln!("  Edit:        {:>7} us", edit_us);
        eprintln!("  Diagnostics: {:>7} us ({} diags)\n", diag_us, diags.len());
    }

    // Scenario 2: Raw re-parse cost (no salsa)
    eprintln!("-- Scenario 2: Raw re-parse cost --");
    {
        let modified = make_body_edit(&target_text);
        let t0 = Instant::now();
        let (_, errs) = syntax::parse_text(&modified);
        let raw_parse_ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("  Raw re-parse:   {:>7.2} ms ({} errors)", raw_parse_ms, errs.len());

        let t1 = Instant::now();
        let (root, _) = syntax::parse_text(&modified);
        let tree = hir_def::ItemTree::build_from_cst(&root);
        let tree_ms = t1.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "  Parse+ItemTree: {:>7.2} ms ({} items)\n",
            tree_ms,
            tree.top_level_items().len()
        );
    }

    // Scenario 3: salsa set_file_text cost (known slow, skipped)
    eprintln!("-- Scenario 3: salsa set_file_text cost (skipped -- known slow) --");
    eprintln!("  salsa revision bump after full cache warm triggers eager re-verification.\n");

    // Scenario 4: Body-only edit consistency
    //
    // 1. Run full_diagnostics() → baseline
    // 2. Edit file (insert statement)
    // 3. Re-run full_diagnostics() → verify no crash, results reasonable
    //
    // A body-only edit (changing a number literal) should NOT introduce
    // or remove structural diagnostics (parse errors, duplicate-definition).
    // Type errors may change but must not crash.
    eprintln!("-- Scenario 4: Body-only edit consistency --");
    {
        // Baseline diagnostics
        let baseline = diag_one_file(&host, &files, target_fid, &config, &ws_names);
        let baseline_count = baseline.len();
        let baseline_codes: Vec<String> = baseline.iter().map(|d| d.code.as_str().to_string()).collect();
        eprintln!("  Baseline: {} diagnostics", baseline_count);

        // Apply body-only edit
        let modified = make_body_edit(&target_text);
        let t0 = Instant::now();
        files.set_file_text(host.raw_database_mut(), target_fid, &modified);
        let edit_us = t0.elapsed().as_micros();

        // Re-run diagnostics (must not crash)
        let t1 = Instant::now();
        let after = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            diag_one_file(&host, &files, target_fid, &config, &ws_names)
        }));
        let diag_us = t1.elapsed().as_micros();

        match after {
            Ok(after_diags) => {
                let after_count = after_diags.len();
                let after_codes: Vec<String> = after_diags.iter().map(|d| d.code.as_str().to_string()).collect();

                // Structural diagnostics should be stable across body-only edits
                let structural_before: Vec<&String> = baseline_codes.iter()
                    .filter(|c| c.contains("duplicate") || c.contains("syntax") || c.contains("parse"))
                    .collect();
                let structural_after: Vec<&String> = after_codes.iter()
                    .filter(|c| c.contains("duplicate") || c.contains("syntax") || c.contains("parse"))
                    .collect();

                let structural_stable = structural_before.len() == structural_after.len();
                eprintln!("  After edit: {} diagnostics (edit: {} us, diag: {} us)", after_count, edit_us, diag_us);
                eprintln!("  Structural stable: {} (before: {}, after: {})",
                    if structural_stable { "YES" } else { "NO" },
                    structural_before.len(), structural_after.len());

                if !structural_stable {
                    eprintln!("  WARNING: body-only edit changed structural diagnostics");
                }
            }
            Err(_) => {
                eprintln!("  CRASH: diagnostics panicked after body-only edit!");
                eprintln!("  This indicates a salsa incremental computation bug.");
            }
        }

        // Restore original
        files.set_file_text(host.raw_database_mut(), target_fid, &target_text);
        eprintln!();
    }

    // Scenario 5: Signature edit consistency
    //
    // Editing a function signature (e.g., adding a parameter) exercises
    // deeper salsa invalidation: ItemTree hash changes → re-infer dependents.
    // Must not crash; diagnostic count may legitimately change.
    eprintln!("-- Scenario 5: Signature edit consistency --");
    {
        let baseline = diag_one_file(&host, &files, target_fid, &config, &ws_names);
        let baseline_count = baseline.len();
        eprintln!("  Baseline: {} diagnostics", baseline_count);

        // Insert a new val spec + function (signature-level change)
        let modified = format!(
            "{}\nval __bench_sig_test : int -> int\nfunction __bench_sig_test(x) = x + 1\n",
            target_text
        );

        let t0 = Instant::now();
        files.set_file_text(host.raw_database_mut(), target_fid, &modified);
        let edit_us = t0.elapsed().as_micros();

        let t1 = Instant::now();
        let after = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            diag_one_file(&host, &files, target_fid, &config, &ws_names)
        }));
        let diag_us = t1.elapsed().as_micros();

        match after {
            Ok(after_diags) => {
                let after_count = after_diags.len();
                eprintln!("  After sig edit: {} diagnostics (edit: {} us, diag: {} us)", after_count, edit_us, diag_us);
                // Adding a well-typed function should not increase error count
                // (it may add unused-variable warnings at most)
                let errors_before = baseline.iter().filter(|d| d.severity == ide_diagnostics::Severity::Error).count();
                let errors_after = after_diags.iter().filter(|d| d.severity == ide_diagnostics::Severity::Error).count();
                eprintln!("  Error count: before={}, after={} (should be equal or fewer)", errors_before, errors_after);
            }
            Err(_) => {
                eprintln!("  CRASH: diagnostics panicked after signature edit!");
                eprintln!("  This indicates a salsa incremental computation bug.");
            }
        }

        // Restore original
        files.set_file_text(host.raw_database_mut(), target_fid, &target_text);
        eprintln!();
    }

    eprintln!("=== Done ===");
}


/// Pick a medium-sized file suitable for highlighting/completion benchmarks.
/// Prefers files with 100-400 lines that contain function definitions.
fn pick_medium_target_file(
    url_fids: &[(Url, FileId)],
    files: &Files,
    db: &ide_db::root_database::RootDatabase,
) -> Option<(Url, FileId, String, usize)> {
    // First pass: ideal size range with functions
    for (url, fid) in url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let text_arc = ft.text(db);
            let text: &str = &text_arc;
            let lines = text.lines().count();
            if lines >= 100 && lines <= 400 && text.contains("function ") {
                return Some((url.clone(), *fid, text.to_string(), lines));
            }
        }
    }
    // Fallback: any file with functions
    for (url, fid) in url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let text_arc = ft.text(db);
            let text: &str = &text_arc;
            let lines = text.lines().count();
            if lines >= 50 && text.contains("function ") {
                return Some((url.clone(), *fid, text.to_string(), lines));
            }
        }
    }
    None
}

#[test]
fn benchmark_highlighting() {
    if std::env::var("RUN_SLOW_BENCHES").is_err() {
        return;
    }

    let dir = bench_sail_dir().expect(
        "RUN_SLOW_BENCHES set but no sail-riscv directory found. \
         Set SAIL_RISCV_DIR=/path/to/sail-riscv",
    );

    eprintln!("=== Highlighting Benchmark (incremental) ===\n");

    // 1. Load workspace
    let _cpu = profile::cpu_span();
    eprintln!("[bench] Loading workspace...");
    let t0 = Instant::now();
    let (mut host, mut files, url_fids, url_map) = load_workspace_with_url_map(&dir);
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[bench] Workspace: {} files, {:.0} ms\n", url_fids.len(), load_ms);

    // Pick target file
    let (target_url, target_fid, target_text, target_lines) =
        match pick_medium_target_file(&url_fids, &files, host.raw_database()) {
            Some(t) => t,
            None => {
                eprintln!("No suitable target file found");
                return;
            }
        };
    let target_name = target_url.path().rsplit('/').next().unwrap_or("?");
    eprintln!("[bench] Target: {} ({} lines)\n", target_name, target_lines);

    // 2. Initial highlighting (cold)
    eprintln!("-- Initial highlighting (cold) --");
    let t0 = Instant::now();
    {
        let _t = stdx::timeit("initial_highlighting");
        let analysis = host.analysis_with_url_map(&files, url_map.clone());
        let tokens = analysis.semantic_tokens(target_fid).unwrap();
        let initial_ms = t0.elapsed().as_secs_f64() * 1000.0;
        eprintln!("  Semantic tokens: {} tokens, {:.2} ms\n", tokens.data.len(), initial_ms);
    }

    // 3. Edit file
    eprintln!("-- Incremental highlighting (after edit) --");
    let mut modified = target_text.clone();
    // Try to patch a function body; fall back to appending a comment
    let edit_desc = if modified.contains("= 0x") {
        patch(&mut modified, "= 0x", "= 0xFF /*patched*/ + 0x");
        "patched hex literal"
    } else if modified.contains("= 0") {
        patch(&mut modified, "= 0", "= 99 /*patched*/ + 0");
        "patched decimal literal"
    } else {
        modified.push_str("\n// benchmark-edit\n");
        "appended comment"
    };
    eprintln!("  Edit: {}", edit_desc);

    // Apply edit via salsa
    let t_edit = Instant::now();
    files.set_file_text(host.raw_database_mut(), target_fid, &modified);
    let edit_us = t_edit.elapsed().as_micros();
    eprintln!("  set_file_text: {} us", edit_us);

    // 4. Re-compute highlighting (should benefit from salsa caching)
    let t1 = Instant::now();
    {
        let _t = stdx::timeit("incremental_highlighting");
        let analysis = host.analysis_with_url_map(&files, url_map.clone());
        let tokens = analysis.semantic_tokens(target_fid).unwrap();
        let incr_ms = t1.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "  Semantic tokens (incremental): {} tokens, {:.2} ms\n",
            tokens.data.len(),
            incr_ms
        );
    }

    eprintln!("=== Highlighting Done ===");
}


#[test]
fn benchmark_completion() {
    if std::env::var("RUN_SLOW_BENCHES").is_err() {
        return;
    }

    let dir = bench_sail_dir().expect(
        "RUN_SLOW_BENCHES set but no sail-riscv directory found. \
         Set SAIL_RISCV_DIR=/path/to/sail-riscv",
    );

    eprintln!("=== Completion Benchmark (incremental) ===\n");

    // 1. Load workspace
    let _cpu = profile::cpu_span();
    eprintln!("[bench] Loading workspace...");
    let t0 = Instant::now();
    let (mut host, mut files, url_fids, url_map) = load_workspace_with_url_map(&dir);
    let load_ms = t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("[bench] Workspace: {} files, {:.0} ms\n", url_fids.len(), load_ms);

    // Pick target file - need one with function bodies
    let (target_url, target_fid, target_text, target_lines) =
        match pick_medium_target_file(&url_fids, &files, host.raw_database()) {
            Some(t) => t,
            None => {
                eprintln!("No suitable target file found");
                return;
            }
        };
    let target_name = target_url.path().rsplit('/').next().unwrap_or("?");
    eprintln!("[bench] Target: {} ({} lines)\n", target_name, target_lines);

    // 2. Find a suitable completion position.
    // Insert a partial identifier and complete at that position.
    // We find the first `function <name>() = ` and insert `sel` after the `=`.
    let mut modified = target_text.clone();
    let completion_line;
    let completion_col;

    if let Some(fn_pos) = modified.find("function ") {
        // Find the `=` after the function signature
        if let Some(eq_offset) = modified[fn_pos..].find(" = ") {
            let insert_pos = fn_pos + eq_offset + 3; // after " = "
            modified.insert_str(insert_pos, "sel");
            // Calculate line/col for the position after "sel"
            let before = &modified[..insert_pos + 3];
            completion_line = before.lines().count().saturating_sub(1) as u32;
            let last_line = before.lines().last().unwrap_or("");
            completion_col = last_line.len() as u32;
        } else {
            // Fallback: insert at end of line containing "function"
            let line_end =
                modified[fn_pos..].find('\n').map(|p| fn_pos + p).unwrap_or(modified.len());
            modified.insert_str(line_end, " sel");
            let before = &modified[..line_end + 4];
            completion_line = before.lines().count().saturating_sub(1) as u32;
            let last_line = before.lines().last().unwrap_or("");
            completion_col = last_line.len() as u32;
        }
    } else {
        // No function found - append and complete at end
        modified.push_str("\nsel");
        completion_line = modified.lines().count().saturating_sub(1) as u32;
        completion_col = 3;
    }

    let position = ide_db::LineCol { line: completion_line, col: completion_col };
    eprintln!("[bench] Completion position: line {}, col {}\n", completion_line, completion_col);

    // 3. Initial completion (cold - file not yet in db with edit)
    eprintln!("-- Initial completion --");
    files.set_file_text(host.raw_database_mut(), target_fid, &modified);

    let t1 = Instant::now();
    let initial_items;
    {
        let _t = stdx::timeit("initial_completion");
        let analysis = host.analysis_with_url_map(&files, url_map.clone());
        let items = analysis.completions(target_fid, position).unwrap();
        initial_items = items.len();
    }
    let initial_ms = t1.elapsed().as_secs_f64() * 1000.0;
    eprintln!("  Completions: {} items, {:.2} ms\n", initial_items, initial_ms);

    // 4. Incremental completion (edit file again, re-complete)
    eprintln!("-- Incremental completion (after second edit) --");
    // Make a small additional edit: change "sel" to "se"
    let mut modified2 = modified.clone();
    if let Some(idx) = modified2.find("sel") {
        modified2.replace_range(idx..idx + 3, "se");
    }
    files.set_file_text(host.raw_database_mut(), target_fid, &modified2);

    // Adjust position for shorter prefix
    let position2 =
        ide_db::LineCol { line: completion_line, col: completion_col.saturating_sub(1) };

    let t2 = Instant::now();
    let incr_items;
    {
        let _t = stdx::timeit("incremental_completion");
        let analysis = host.analysis_with_url_map(&files, url_map.clone());
        let items = analysis.completions(target_fid, position2).unwrap();
        incr_items = items.len();
    }
    let incr_ms = t2.elapsed().as_secs_f64() * 1000.0;
    eprintln!("  Completions (incremental): {} items, {:.2} ms\n", incr_items, incr_ms);

    eprintln!("=== Completion Done ===");
}
