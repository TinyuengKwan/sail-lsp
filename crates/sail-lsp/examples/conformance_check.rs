//! Conformance check: differential-test sail-lsp against upstream Sail's own
//! typecheck test suite (`sail/test/typecheck/{pass,fail}`).
//!
//! Upstream contract (see `sail/test/suites/typecheck.py`):
//!   - every file in `pass/` must typecheck cleanly
//!     (`sail --just-check --strict-bitvector`)
//!   - every file in `fail/` must be rejected with the error text in the
//!     sibling `.expect` file
//!
//! This harness runs each file through the exact `Analysis::file_diagnostics`
//! pipeline the LSP server uses and reports:
//!   - FALSE POSITIVE — upstream accepts, sail-lsp reports an error
//!   - FALSE NEGATIVE — upstream rejects, sail-lsp reports nothing
//!
//! Each test file gets a fresh `AnalysisHost` preloaded with the bundled
//! sail-lib, so `$include <prelude.sail>` resolves and test files cannot
//! collide with one another.
//!
//! The typecheck suite enables strict-bitvector on both sides. Extended corpora
//! retain the default mode used to produce their oracle. `--default-bitvector`
//! is a diagnostic comparison only: the typecheck directory expectations were
//! generated in strict mode and do not change when this switch is used.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --release --example conformance_check -- /path/to/sail/test/typecheck
//! cargo run --release --example conformance_check -- --list /path/to/sail/test/typecheck
//! cargo run --release --example conformance_check -- --only bitfield /path/to/...
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use base_db::FileId;
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use url::Url;

/// Recursively discover .sail files under `root`.
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

/// Build an `Analysis` containing the prelude plus exactly one test file.
/// Returns the analysis and the `FileId` of the test file.
fn analysis_for(
    lib_files: &[(Url, PathBuf, String)],
    test: &(Url, PathBuf, String),
    strict_bitvector: bool,
) -> (Analysis, FileId, std::collections::HashSet<FileId>) {
    let mut all: Vec<&(Url, PathBuf, String)> = lib_files.iter().collect();
    all.push(test);

    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    change.set_strict_bitvector(strict_bitvector);
    let mut vfs = vfs::Vfs::default();

    for (url, _p, text) in &all {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    host.apply_change(std::mem::take(&mut change));
    let files = host.raw_database().files().clone();

    {
        let db = host.raw_database_mut();
        let file_texts: Vec<base_db::FileText> = all
            .iter()
            .filter_map(|(url, _, _)| {
                let fid = vfs.lookup_file_id_by_url(url)?;
                files.file_text(fid)
            })
            .collect();
        hir::def_query::set_workspace_files(db, file_texts);
    }

    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    for (url, _p, _t) in &all {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_to_fid.insert((*url).clone(), fid);
            fid_to_url.insert(fid, (*url).clone());
        }
    }
    let test_fid = *url_to_fid.get(&test.0).expect("test file registered");
    let url_map = UrlMap::new(url_to_fid.clone(), fid_to_url);
    // The include graph the server builds, by the same function: an
    // `Analysis` without one is a different program, and `$include` cycles
    // are reported only by a holder of the whole graph.
    let paths: Vec<(Url, std::path::PathBuf)> =
        all.iter().map(|(url, p, _)| ((*url).clone(), p.clone())).collect();
    let graph = ide::analysis::build_include_graph(
        host.raw_database(),
        &paths,
        &url_to_fid,
        find_sail_lib().as_deref(),
    );
    let included = graph.transitive_includes(test_fid);
    let analysis = Analysis::with_include_graph(
        host.raw_database().clone(),
        files.clone(),
        url_map,
        std::sync::Arc::new(graph),
    );

    (analysis, test_fid, included)
}

struct Outcome {
    name: String,
    errors: Vec<(String, String)>, // (code, message)
    /// Byte offset each error starts at, so it can be compared with the
    /// caret in upstream's `.expect`.
    starts: Vec<Option<usize>>,
    crashed: bool,
}

/// The sibling files a test `$include`s by relative path, transitively.
///
/// `$include "data.sail"` resolves against the including file's own
/// directory; `$include <..>` is the library and is already loaded. Without
/// them `test/float/add_test.sail` reported 753 unresolved identifiers — a
/// statement about what the probe was given, not about the checker. The
/// same rule the repo already wrote down: before believing a number, check
/// what the probe hands the feature.
fn relative_includes(start: &Path) -> Vec<(Url, PathBuf, String)> {
    let mut seen: std::collections::BTreeSet<PathBuf> = Default::default();
    let mut queue = vec![start.to_path_buf()];
    let mut out = Vec::new();
    while let Some(path) = queue.pop() {
        let Ok(text) = std::fs::read_to_string(&path) else { continue };
        let Some(dir) = path.parent() else { continue };
        for line in text.lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("$include") else { continue };
            let rest = rest.trim();
            // Only the quoted form is relative.
            let Some(name) = rest.strip_prefix('"').and_then(|r| r.split('"').next()) else {
                continue;
            };
            let included = dir.join(name);
            if !included.exists() || !seen.insert(included.clone()) {
                continue;
            }
            if let (Ok(text), Ok(url)) =
                (std::fs::read_to_string(&included), Url::from_file_path(&included))
            {
                out.push((url, included.clone(), text));
                queue.push(included);
            }
        }
    }
    out
}

/// What upstream says about a file it rejects, read off its `.expect`.
///
/// `sail/test/typecheck/fail/` ships one per case — `run_tests.py` diffs
/// `sail`'s whole rendering against it — so the 137 files this harness had
/// been scoring as a *yes/no* rejection carry upstream's exact message and
/// exact caret. Rejecting for the wrong reason at the wrong place scored
/// the same as agreeing, which is the shape `$ifdef` was found in: already
/// reported, as a count, at the end of the file.
///
/// The format is a header line, a `file:LINE.COL-..` line, the source line
/// with a caret under it, and the message indented under a `|`. Nested
/// hints repeat that with deeper `|`s, and a file may hold several
/// findings; only the first that names *this* file is read, because that
/// is the one a reader sees first.
struct Expectation {
    /// 1-based line, 0-based column, as upstream prints them.
    line: usize,
    col: usize,
    message: String,
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

fn read_expectation(path: &Path, stem: &str) -> Option<Expectation> {
    let text = std::fs::read_to_string(path.with_extension("expect")).ok()?;
    parse_expectation(&text, stem)
}

fn parse_expectation(text: &str, stem: &str) -> Option<Expectation> {
    let text = strip_ansi(text);
    let needle = format!("{stem}.sail:");

    let mut loc: Option<(usize, usize)> = None;
    let mut warning = false;
    for line in text.lines() {
        if line.starts_with("Warning:") {
            warning = true;
            loc = None;
        } else if line.starts_with("Type error:") || line.starts_with("Error:") {
            warning = false;
            loc = None;
        }
        if warning {
            continue;
        }
        if let Some(at) = line.find(&needle) {
            // A location line. It may be a secondary one — upstream prints
            // "Previous definition" and "Has type Foo" *before* the finding
            // they belong to — so keep the most recent and let the message
            // decide which one it was.
            let rest = &line[at + needle.len()..];
            if let Some((l, c)) = rest.split_once('.') {
                let c: String = c.chars().take_while(char::is_ascii_digit).collect();
                if let (Ok(l), Ok(c)) = (l.trim().parse(), c.parse()) {
                    loc = Some((l, c));
                }
            }
            continue;
        }
        let (depth, body) = indent_depth(line);
        // Depth 1 only: a deeper one is a nested "Caused by", which is a
        // different finding with its own location.
        if depth != 1 || body.is_empty() {
            continue;
        }
        // A caret line, possibly with an inline hint after it. Both belong
        // to the location above, and neither is the finding's message.
        if body.starts_with('^') || body.starts_with('-') {
            continue;
        }
        let (line, col) = loc?;
        return Some(Expectation { line, col, message: body.to_string() });
    }
    None
}

/// How many `|` indent markers a rendered line carries, and what follows.
///
/// `  | msg` is depth 1, `  |   | msg` is depth 2, and `5 |source` is depth
/// 0 — the digits come first. A message that itself contains a `|` stays at
/// depth 1, because the scan only continues while the *next* non-space
/// character is another `|`.
fn indent_depth(line: &str) -> (usize, &str) {
    let mut rest = line;
    let mut depth = 0;
    loop {
        let trimmed = rest.trim_start();
        let Some(next) = trimmed.strip_prefix('|') else { return (depth, trimmed.trim()) };
        depth += 1;
        rest = next;
    }
}

/// Byte offset of a 1-based line and 0-based column.
fn offset_of(text: &str, line: usize, col: usize) -> usize {
    text.split_inclusive('\n').take(line.saturating_sub(1)).map(str::len).sum::<usize>() + col
}

/// Whether a message names one of upstream's generated existential
/// witnesses, which is a counter local to that `sail` process.
fn has_gensym(message: &str) -> bool {
    let b = message.as_bytes();
    b.windows(3)
        .enumerate()
        .any(|(i, w)| w[0] == b'\'' && w[1] == b'e' && w[2] == b'x' && b[i + 3..].contains(&b'#'))
}

/// The first line of a message, for a table that has to stay readable.
fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("").trim()
}

fn run_one(
    lib_files: &[(Url, PathBuf, String)],
    test: &(Url, PathBuf, String),
    config: &ide_diagnostics::DiagnosticsConfig,
    strict_bitvector: bool,
) -> Outcome {
    let name = test.1.file_name().unwrap_or_default().to_string_lossy().to_string();
    let mut scope = lib_files.to_vec();
    let local_includes = relative_includes(&test.1);
    scope.extend(local_includes.iter().cloned());
    let lib_files = &scope[..];
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let (analysis, fid, included) = analysis_for(lib_files, test, strict_bitvector);
        let mut diagnostics: Vec<_> = analysis
            .file_diagnostics(fid, config)
            .expect("no other thread is here to cancel it")
            .into_iter()
            .map(|d| (None, d))
            .collect();
        // Upstream checks the complete included program. A clean entry file
        // does not imply acceptance when a relative dependency has errors.
        // Use active graph edges, not just the disk discovery superset.
        for (url, path, _) in &local_includes {
            let Some(id) = analysis.file_id(url).filter(|id| *id != fid && included.contains(id))
            else {
                continue;
            };
            diagnostics.extend(
                analysis
                    .file_diagnostics(id, config)
                    .expect("no other thread is here to cancel it")
                    .into_iter()
                    .map(|d| (Some(path.display().to_string()), d)),
            );
        }
        diagnostics
    }));
    match res {
        Ok(diags) => {
            let errs: Vec<_> = diags
                .iter()
                .filter(|(_, d)| matches!(d.severity, ide_diagnostics::Severity::Error))
                .collect();
            let starts = errs
                .iter()
                .map(|(file, d)| file.is_none().then(|| usize::from(d.range.range.start())))
                .collect();
            let errors = errs
                .iter()
                .map(|(file, d)| {
                    let message = file
                        .as_ref()
                        .map_or_else(|| d.message.clone(), |file| format!("{file}: {}", d.message));
                    (d.code.as_str().to_string(), message)
                })
                .collect();
            Outcome { name, errors, starts, crashed: false }
        }
        Err(_) => Outcome { name, errors: Vec::new(), starts: Vec::new(), crashed: true },
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list = args.iter().any(|a| a == "--list");
    let detail = args.iter().any(|a| a == "--fail-detail");
    let typecheck_only = args.iter().any(|a| a == "--typecheck-only");
    let strict_bitvector = !args.iter().any(|a| a == "--default-bitvector");
    let only = args
        .iter()
        .position(|a| a == "--only")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .filter(|s| !s.starts_with('-'));
    let root = args
        .iter()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .find(|a| Some((*a).clone()) != only)
        .cloned()
        .unwrap_or_else(|| {
            eprintln!(
                "Usage: conformance_check [--list] [--only SUBSTR] [--fail-detail] [--typecheck-only] [--default-bitvector] \
                 /path/to/sail/test/typecheck"
            );
            std::process::exit(2);
        });

    let root = PathBuf::from(&root).canonicalize().unwrap_or_else(|err| {
        eprintln!("cannot open typecheck suite {root}: {err}");
        std::process::exit(2);
    });
    for suite in ["pass", "fail"] {
        let dir = root.join(suite);
        if !dir.is_dir() {
            eprintln!("missing typecheck suite directory: {}", dir.display());
            std::process::exit(2);
        }
    }
    let handle = std::thread::Builder::new()
        .name("conformance".into())
        .stack_size(512 * 1024 * 1024)
        .spawn(move || run(&root, list, only.as_deref(), detail, typecheck_only, strict_bitvector))
        .expect("spawn");
    std::process::exit(handle.join().unwrap());
}

fn run(
    root: &Path,
    list: bool,
    only: Option<&str>,
    detail: bool,
    typecheck_only: bool,
    strict_bitvector: bool,
) -> i32 {
    let lib_dir = match find_sail_lib() {
        Some(d) => d,
        None => {
            eprintln!("sail-lib not found; set SAIL_LIB or run from the repo root");
            return 2;
        }
    };
    let lib_files = discover_sail_files(&lib_dir);
    eprintln!("prelude: {} files from {}", lib_files.len(), lib_dir.display());

    println!("Typecheck mode: strict-bitvector={strict_bitvector}; extended corpora: default");
    let config = ide_diagnostics::DiagnosticsConfig::new();

    let mut pass_files: Vec<_> = std::fs::read_dir(root.join("pass"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sail"))
        .collect();
    pass_files.sort();
    let mut fail_files: Vec<_> = std::fs::read_dir(root.join("fail"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "sail"))
        .collect();
    fail_files.sort();

    let load = |p: &PathBuf| -> Option<(Url, PathBuf, String)> {
        let text = std::fs::read_to_string(p).ok()?;
        let url = Url::from_file_path(p).ok()?;
        Some((url, p.clone(), text))
    };

    let keep = |p: &PathBuf| -> bool {
        only.is_none_or(|s| p.file_name().unwrap_or_default().to_string_lossy().contains(s))
    };

    let t0 = std::time::Instant::now();

    // ---- pass/ : upstream accepts, sail-lsp must not report errors ----
    let mut fp: Vec<Outcome> = Vec::new();
    let mut crashes: Vec<String> = Vec::new();
    let mut pass_ok = 0usize;
    let mut pass_n = 0usize;
    for p in pass_files.iter().filter(|p| keep(p)) {
        let Some(t) = load(p) else { continue };
        pass_n += 1;
        let out = run_one(&lib_files, &t, &config, strict_bitvector);
        if out.crashed {
            crashes.push(out.name.clone());
        }
        if out.errors.is_empty() && !out.crashed {
            pass_ok += 1;
        } else {
            fp.push(out);
        }
        if pass_n.is_multiple_of(25) {
            eprintln!("  pass {}/{}", pass_n, pass_files.len());
        }
    }

    // ---- fail/ : upstream rejects, sail-lsp should report an error ----
    let mut fneg: Vec<String> = Vec::new();
    let mut fail_ok = 0usize;
    let mut fail_n = 0usize;
    // Beyond yes/no: does the first thing a reader sees agree with what
    // upstream says, and where it says it?
    let mut located = 0usize;
    let mut worded = 0usize;
    let mut with_expect = 0usize;
    let mut misplaced = 0usize;
    let mut unmatchable = 0usize;
    let mut disagreements: Vec<(String, String, String)> = Vec::new();
    let mut misplaced_at: Vec<(String, String, usize, usize)> = Vec::new();
    for p in fail_files.iter().filter(|p| keep(p)) {
        let Some(t) = load(p) else { continue };
        fail_n += 1;
        let out = run_one(&lib_files, &t, &config, strict_bitvector);
        if out.crashed {
            crashes.push(out.name.clone());
        } else if out.errors.is_empty() {
            fneg.push(out.name.clone());
        } else {
            fail_ok += 1;
        }
        let stem = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
        if let Some(want) = read_expectation(p, &stem) {
            with_expect += 1;
            let at = offset_of(&t.2, want.line, want.col);
            let hit = out.starts.iter().position(|s| *s == Some(at));
            if detail {
                println!(
                    "DIAGNOSTIC_AUDIT {}",
                    serde_json::json!({
                        "file": out.name,
                        "expected_offset": at,
                        "expected_message": want.message,
                        "upstream_gensym": has_gensym(&want.message),
                        "actual": out.errors.iter().zip(&out.starts).map(|((code, message), start)| {
                            serde_json::json!({"code": code, "message": message, "start": start})
                        }).collect::<Vec<_>>()
                    })
                );
            }
            if hit.is_some() {
                located += 1;
            }
            // Upstream's finding is a sequence of `Line`s and the
            // expectation is its first; a message here that carries more
            // than one line is compared on its first too, or every
            // multi-line finding would read as a disagreement about
            // wording it agrees with.
            let said = hit.map(|i| first_line(&out.errors[i].1).to_string());
            // Upstream's words at some *other* offset: the check is right
            // and the span is wrong, which is a different kind of work
            // from not having the check at all.
            if hit.is_none() {
                if let Some(i) = out.errors.iter().position(|(_, m)| first_line(m) == want.message)
                {
                    if let Some(start) = out.starts[i] {
                        misplaced += 1;
                        misplaced_at.push((out.name.clone(), want.message.clone(), at, start));
                    }
                }
            }
            if said.as_deref() == Some(want.message.as_str()) {
                worded += 1;
            } else if has_gensym(&want.message) {
                // Upstream names the witness of an existential with a
                // counter — `bool('ex1#)`, `int('ex189#)` — that numbers
                // every existential it has opened in that run. Nothing
                // outside `sail`'s own process can reproduce it, so a
                // message carrying one can never match textually. Counted
                // apart rather than left in the disagreement list, where it
                // would read as work outstanding for ever.
                unmatchable += 1;
            } else {
                disagreements.push((
                    want.message.clone(),
                    out.name.clone(),
                    said.unwrap_or_else(|| {
                        if hit.is_none() && !out.errors.is_empty() {
                            format!("(elsewhere) {}", out.errors[0].1)
                        } else {
                            "(nothing)".to_string()
                        }
                    }),
                ));
            }
        }
        if fail_n.is_multiple_of(25) {
            eprintln!("  fail {}/{}", fail_n, fail_files.len());
        }
    }

    // ---- the rest of upstream's corpora, by `docs/upstream-accepts.txt` ----
    //
    // `typecheck/` is one directory out of two dozen, and it was the only one
    // measured. The oracle for the others already existed — over
    // `test/{c,mono,ocaml,lean,rocq,builtins,smt,float,pattern_completeness}`
    // — and nothing read it. A raw diagnostic count on those corpora is not a
    // false-positive count, which is exactly what the manifest is for: only a
    // file upstream accepts is one sail-lsp has to be silent on.
    //
    // Nor is a silence count a false-negative count, which took a second
    // verdict to say. The manifest was built by running upstream on each file
    // *alone*, while `run_one` hands sail-lsp the whole bundled library
    // whether or not the file `$include`s it — so the two sides were not
    // given the same program, and every false negative outside `typecheck/`
    // was that difference. `SCOPE` marks the files it accounts for.
    let mut corpora: Vec<(String, usize, usize, usize, usize)> = Vec::new();
    let mut corpus_fp: Vec<Outcome> = Vec::new();
    let mut corpus_fneg: Vec<String> = Vec::new();
    let manifest = if typecheck_only { Vec::new() } else { read_accepts_manifest() };
    for (corpus, entries) in manifest {
        let (mut ok, mut fp_n, mut fneg_n, mut scope_n) = (0usize, 0usize, 0usize, 0usize);
        for (path, verdict) in entries {
            if !keep(&path) {
                continue;
            }
            let Some(t) = load(&path) else { continue };
            let out = run_one(&lib_files, &t, &config, false);
            if out.crashed {
                crashes.push(out.name.clone());
                continue;
            }
            let clean = out.errors.is_empty();
            match (verdict, clean) {
                // A file upstream only rejects for want of the prelude is not
                // evidence in either direction: this harness gives every file
                // that prelude on purpose. It is counted and printed so the
                // number is visible rather than absorbed into "agree".
                (Verdict::Scope, _) => scope_n += 1,
                (Verdict::Ok, true) | (Verdict::Reject, false) => ok += 1,
                (Verdict::Ok, false) => {
                    fp_n += 1;
                    corpus_fp.push(out);
                }
                (Verdict::Reject, true) => {
                    fneg_n += 1;
                    corpus_fneg.push(out.name);
                }
            }
        }
        if ok + fp_n + fneg_n + scope_n > 0 {
            corpora.push((corpus, ok, fp_n, fneg_n, scope_n));
        }
    }

    if pass_n + fail_n == 0 {
        eprintln!("no typecheck cases executed; check the suite path and --only filter");
        return 2;
    }

    // ---- report ----
    if list {
        println!("## FALSE POSITIVES (upstream accepts, sail-lsp errors)");
        for o in fp.iter().chain(corpus_fp.iter()) {
            if o.crashed {
                println!("{}\tCRASH", o.name);
                continue;
            }
            for (code, msg) in &o.errors {
                println!("{}\t{}\t{}", o.name, code, msg.replace('\n', " "));
            }
        }
        println!();
        println!("## FALSE NEGATIVES (upstream rejects, sail-lsp silent)");
        for n in fneg.iter().chain(corpus_fneg.iter()) {
            println!("{n}");
        }
        println!();
    }

    let mut by_code: HashMap<String, usize> = HashMap::new();
    for o in fp.iter().chain(corpus_fp.iter()) {
        if o.crashed {
            *by_code.entry("CRASH".into()).or_default() += 1;
        }
        for (c, _) in &o.errors {
            *by_code.entry(c.clone()).or_default() += 1;
        }
    }

    println!("============================================================");
    println!("CONFORMANCE vs upstream Sail typecheck suite ({} ms)", t0.elapsed().as_millis());
    println!("============================================================");
    println!(
        "  pass/: {}/{} clean   ({} files with false-positive errors)",
        pass_ok,
        pass_n,
        fp.len()
    );
    println!("  fail/: {}/{} rejected ({} false negatives)", fail_ok, fail_n, fneg.len());
    println!(
        "         {located}/{with_expect} at upstream's caret, {worded}/{with_expect} in \
         upstream's words"
    );
    println!("         {misplaced} say upstream's words somewhere else");
    println!(
        "         {unmatchable} upstream messages contain process-local witnesses (`'ex1#`); \
         excluded from literal wording comparison"
    );
    if detail {
        for (file, msg, want, got) in &misplaced_at {
            println!("           {file}: upstream {want}, here {got} — {}", first_line(msg));
        }
    }
    if !disagreements.is_empty() {
        // Grouped by *upstream's* message, because the grouping is what
        // says which defect is worth one fix and which is worth six —
        // the same move that took `mutation_check` from 90.4% to 98.1%.
        let mut by_message: std::collections::BTreeMap<String, Vec<(String, String)>> =
            std::collections::BTreeMap::new();
        for (want, file, said) in disagreements {
            by_message.entry(want).or_default().push((file, said));
        }
        let mut groups: Vec<_> = by_message.into_iter().collect();
        groups.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
        let (top, examples) = if detail { (groups.len(), usize::MAX) } else { (20, 2) };
        println!("\n  fail/ disagreements, by upstream's own message:");
        for (want, cases) in groups.iter().take(top) {
            println!("    {:3}  {}", cases.len(), first_line(want));
            for (file, said) in cases.iter().take(examples) {
                println!("         {file}: {}", first_line(said));
            }
        }
        if groups.len() > top {
            println!("    ... and {} more groups (--fail-detail for all)", groups.len() - top);
        }
    }
    for f in &fneg {
        println!("    accepted but upstream rejects: {f}");
    }
    println!();
    if !corpora.is_empty() {
        println!();
        println!("  The rest of upstream's corpora (oracle: docs/upstream-accepts.txt):");
        for (name, ok, fp_n, fneg_n, scope_n) in &corpora {
            println!(
                "    {name:<22} {ok:>4} agree   {fp_n:>3} false positive  \
                 {fneg_n:>3} false negative  {scope_n:>3} scope-only"
            );
        }
        let scope_total: usize = corpora.iter().map(|c| c.4).sum();
        if scope_total > 0 {
            println!();
            println!(
                "    scope-only: {scope_total} files upstream rejects on their own and accepts\n\
                 \x20   once the bundled prelude is in scope. This harness hands every file\n\
                 \x20   that prelude, because that is what an editor has. Scope in a real\n\
                 \x20   Sail project does not come from `$include` at all: sail-riscv's 158\n\
                 \x20   model files contain no `$include` between them, and\n\
                 \x20   riscv.sail_project lists them per module. Neither column."
            );
        }
    }
    println!();
    println!("  False-positive diagnostics by code:");
    let mut sorted: Vec<_> = by_code.iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(a.1));
    for (code, n) in sorted {
        println!("    {code:<44} {n}");
    }

    for name in &crashes {
        println!("  CHECKER CRASH (not a rejection): {name}");
    }
    if crashes.is_empty()
        && fp.is_empty()
        && fneg.is_empty()
        && corpus_fp.is_empty()
        && corpus_fneg.is_empty()
    {
        0
    } else {
        1
    }
}

/// `OK` / `SCOPE` / `REJECT` followed by an absolute path, grouped by the
/// corpus directory each path sits in. Missing files are skipped: the manifest
/// holds absolute paths for one checkout on purpose, and a moved checkout
/// should measure less rather than fail.
/// Regenerate with `scripts/regen_upstream_accepts.py`.
/// What upstream does with a file, in the manifest's own three verdicts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Upstream accepts it on its own; a diagnostic here is a false positive.
    Ok,
    /// Upstream rejects it on its own and accepts it once the bundled prelude
    /// is in scope — which is what this harness hands every file, because it
    /// is what an editor has. Scope in a real Sail project does not come from
    /// `$include` at all: sail-riscv's 158 model files contain no `$include`
    /// between them, and `riscv.sail_project` lists them per module with
    /// `requires` edges. Says nothing about the checker in either direction;
    /// counted on its own line so it is not mistaken for evidence.
    Scope,
    /// Upstream rejects it either way; silence here is a false negative.
    Reject,
}

fn read_accepts_manifest() -> Vec<(String, Vec<(PathBuf, Verdict)>)> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .map(|root| root.join("docs/upstream-accepts.txt"));
    let Some(text) = path.and_then(|p| std::fs::read_to_string(p).ok()) else {
        return Vec::new();
    };
    let mut by_corpus: std::collections::BTreeMap<String, Vec<(PathBuf, Verdict)>> =
        Default::default();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((verdict, file)) = line.split_once(char::is_whitespace) else { continue };
        let accepted = match verdict {
            "OK" => Verdict::Ok,
            "SCOPE" => Verdict::Scope,
            // `BAD` is the marker the two-verdict manifest used, and it meant
            // "upstream rejects it standalone" — which is `SCOPE` for most of
            // the files that carried it. Read as `Reject` so an un-regenerated
            // manifest still measures something, not as a silent `Scope`.
            "REJECT" | "BAD" => Verdict::Reject,
            _ => continue,
        };
        let file = PathBuf::from(file.trim());
        if !file.exists() {
            continue;
        }
        // The corpus is the directory under `test/`.
        let corpus = file
            .iter()
            .skip_while(|c| *c != "test")
            .nth(1)
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_else(|| "other".to_string());
        by_corpus.entry(corpus).or_default().push((file, accepted));
    }
    by_corpus.into_iter().collect()
}

#[cfg(test)]
mod expectation_tests {
    use super::parse_expectation;

    #[test]
    fn warnings_before_errors_do_not_change_error_location_or_message() {
        let text = "Warning: deprecated\nfail/example.sail:2.4-8:\n | warning detail\nType error:\nfail/example.sail:9.7-12:\n | ^----^\n | Type mismatch between int and string\n";
        let expected = parse_expectation(text, "example").unwrap();
        assert_eq!((expected.line, expected.col), (9, 7));
        assert_eq!(expected.message, "Type mismatch between int and string");
        assert!(parse_expectation(
            "Warning: deprecated\nfail/example.sail:2.4-8:\n | warning detail\n",
            "example"
        )
        .is_none());
    }
}
