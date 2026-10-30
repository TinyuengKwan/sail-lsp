//! Mutation check: how many *real* type errors does sail-lsp actually catch?
//!
//! `corpus_check` and `conformance_check` both measure one direction — they
//! count diagnostics on programs that are correct, so they can only ever go
//! down. Nothing stops a change from buying silence by checking less, and a
//! checker that reported nothing at all would score perfectly on both.
//!
//! This measures the other direction. It takes programs upstream accepts,
//! breaks each one in a small, mechanical way, and asks whether sail-lsp
//! notices. Upstream Sail is the oracle: a mutant only counts if `sail
//! --just-check` rejects it, so a mutation that happens to stay valid is
//! discarded rather than scored as a miss.
//!
//! ## Usage
//!
//! ```sh
//! # Measure against a recorded oracle (fast, no `sail` needed).
//! cargo run --release --example mutation_check -- \
//!     /path/to/sail/test/typecheck/pass --oracle docs/mutation-oracle.txt
//!
//! # Regenerate the oracle (slow; needs `sail` on PATH or in $SAIL).
//! cargo run --release --example mutation_check -- \
//!     /path/to/sail/test/typecheck/pass --regen-oracle docs/mutation-oracle.txt
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use base_db::FileId;
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use url::Url;

// ---------------------------------------------------------------------------
// Mutations
// ---------------------------------------------------------------------------

/// A single mechanical edit to one file, identified so that an oracle
/// recorded on one run can be matched against another.
struct Mutant {
    /// `file.sail#kind#n` — stable as long as the corpus does not change.
    id: String,
    file_index: usize,
    text: String,
    what: String,
}

/// Widen one bitvector width: `bits(8)` -> `bits(9)`.
///
/// Chosen because it is the mutation Sail programs are least able to absorb —
/// a width participates in constraints almost everywhere it appears — and
/// because it needs no understanding of the surrounding code to apply.
fn widen_bits(text: &str) -> Vec<Site> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find("bits(") {
        let start = search + rel;
        let digits_start = start + "bits(".len();
        let mut end = digits_start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        search = digits_start;
        if end == digits_start || bytes.get(end) != Some(&b')') {
            continue;
        }
        let Ok(n) = text[digits_start..end].parse::<u32>() else { continue };
        let mut mutated = String::with_capacity(text.len() + 1);
        mutated.push_str(&text[..digits_start]);
        mutated.push_str(&(n + 1).to_string());
        mutated.push_str(&text[end..]);
        out.push((start, mutated, format!("bits({n}) -> bits({})", n + 1)));
    }
    out
}

/// Narrow one integer range: `range(0, 7)` -> `range(0, 6)`.
fn narrow_range(text: &str) -> Vec<Site> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find("range(") {
        let start = search + rel;
        search = start + "range(".len();
        let Some(comma) = text[search..].find(',') else { continue };
        let hi_start = search + comma + 1;
        let mut i = hi_start;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let digits_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == digits_start || bytes.get(i) != Some(&b')') {
            continue;
        }
        let Ok(n) = text[digits_start..i].parse::<u32>() else { continue };
        if n == 0 {
            continue;
        }
        let mut mutated = String::with_capacity(text.len());
        mutated.push_str(&text[..digits_start]);
        mutated.push_str(&(n - 1).to_string());
        mutated.push_str(&text[i..]);
        out.push((start, mutated, format!("range(.., {n}) -> range(.., {})", n - 1)));
    }
    out
}

/// Swap the first two arguments of a call: `f(a, b)` -> `f(b, a)`.
///
/// Argument order is the class of error a checker that only compares
/// arities cannot see, and it is invisible to the two width operators
/// above: both arguments keep their own types, they just arrive in the
/// wrong parameter. Only applied where the two arguments are single
/// identifiers, so the edit cannot change what the call means beyond the
/// swap.
fn swap_args(text: &str) -> Vec<Site> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find('(') {
        let open = search + rel;
        search = open + 1;
        // The callee has to be a name, or this is a grouping paren.
        if !ident_ends_at(bytes, open) {
            continue;
        }
        let Some(close_rel) = text[open + 1..].find(')') else { continue };
        let close = open + 1 + close_rel;
        let inner = &text[open + 1..close];
        let Some((a, b)) = inner.split_once(',') else { continue };
        if b.contains(',') {
            continue;
        }
        let (a, b) = (a.trim(), b.trim());
        if !is_plain_ident(a) || !is_plain_ident(b) || a == b {
            continue;
        }
        let mut mutated = String::with_capacity(text.len());
        mutated.push_str(&text[..open + 1]);
        mutated.push_str(b);
        mutated.push_str(", ");
        mutated.push_str(a);
        mutated.push_str(&text[close..]);
        out.push((open, mutated, format!("({a}, {b}) -> ({b}, {a})")));
    }
    out
}

/// Drop the last argument of a call: `f(a, b)` -> `f(a)`.
///
/// Arity, which nothing else here measures.
fn drop_arg(text: &str) -> Vec<Site> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find('(') {
        let open = search + rel;
        search = open + 1;
        if !ident_ends_at(bytes, open) {
            continue;
        }
        let Some(close_rel) = text[open + 1..].find(')') else { continue };
        let close = open + 1 + close_rel;
        let inner = &text[open + 1..close];
        let Some((head, last)) = inner.rsplit_once(',') else { continue };
        if !is_plain_ident(last.trim()) || head.trim().is_empty() {
            continue;
        }
        let mut mutated = String::with_capacity(text.len());
        mutated.push_str(&text[..open + 1]);
        mutated.push_str(head.trim_end());
        mutated.push_str(&text[close..]);
        out.push((open, mutated, format!("dropped last arg `{}`", last.trim())));
    }
    out
}

/// Rename one struct field at its declaration: `{ foo : T }` -> `{ zzz : T }`.
///
/// Every use of the field then names something that does not exist. Field
/// names are the third class the width operators cannot reach.
fn rename_field(text: &str) -> Vec<Site> {
    let mut out = Vec::new();
    for (decl_start, body) in struct_bodies(text) {
        for (rel, name) in field_names(body) {
            let at = decl_start + rel;
            let mut mutated = String::with_capacity(text.len() + 4);
            mutated.push_str(&text[..at]);
            mutated.push_str("zz_gone");
            mutated.push_str(&text[at + name.len()..]);
            out.push((at, mutated, format!("field `{name}` renamed away")));
        }
    }
    out
}

/// Change a declared return type to `bool`: `-> int` -> `-> bool`.
///
/// The body then returns something the signature does not admit. Skips
/// signatures that already return `bool`, and skips `unit`, where a body
/// that ends in a statement often type-checks either way.
fn retype_return(text: &str) -> Vec<Site> {
    const RETURNS: [&str; 5] = ["int", "nat", "string", "real", "bit"];
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find("->") {
        let arrow = search + rel;
        search = arrow + 2;
        let mut i = arrow + 2;
        while i < bytes.len() && bytes[i] == b' ' {
            i += 1;
        }
        let Some(name) = RETURNS.iter().find(|r| text[i..].starts_with(**r)) else { continue };
        let end = i + name.len();
        // A whole word, not the prefix of a longer type name.
        if bytes.get(end).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_') {
            continue;
        }
        let mut mutated = String::with_capacity(text.len() + 2);
        mutated.push_str(&text[..i]);
        mutated.push_str("bool");
        mutated.push_str(&text[end..]);
        out.push((arrow, mutated, format!("-> {name} became -> bool")));
    }
    out
}

/// Whether an identifier character ends at `at` — i.e. `text[at]` opens a
/// call rather than a grouping paren.
fn ident_ends_at(bytes: &[u8], at: usize) -> bool {
    at > 0 && (bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'_')
}

fn is_plain_ident(s: &str) -> bool {
    !s.is_empty()
        && s.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// `(offset of the body, the body text)` for each `struct N = { .. }`.
fn struct_bodies(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut search = 0usize;
    while let Some(rel) = text[search..].find("struct ") {
        let start = search + rel;
        search = start + "struct ".len();
        let Some(open_rel) = text[search..].find('{') else { continue };
        let open = search + open_rel + 1;
        let Some(close_rel) = text[open..].find('}') else { continue };
        out.push((open, &text[open..open + close_rel]));
    }
    out
}

/// `(offset within `body`, name)` for each `name : type` field.
fn field_names(body: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut at = 0usize;
    for part in body.split(',') {
        let here = at;
        at += part.len() + 1;
        let Some((name, _)) = part.split_once(':') else { continue };
        let lead = name.len() - name.trim_start().len();
        let name = name.trim();
        if is_plain_ident(name) {
            out.push((here + lead, name));
        }
    }
    out
}

/// At most this many mutants of one kind per file. The point is coverage of
/// constructs, not of every occurrence in a long file.
const PER_FILE_PER_KIND: usize = 2;

/// `(byte offset, mutated text, human description)` for one mutation site.
type Site = (usize, String, String);

/// A mutation kind: its name, and how to enumerate its sites in a file.
type Kind = (&'static str, fn(&str) -> Vec<Site>);

fn build_mutants(root: &Path, files: &[(Url, PathBuf, String)]) -> Vec<Mutant> {
    let kinds: [Kind; 6] = [
        ("bits", widen_bits),
        ("range", narrow_range),
        ("argorder", swap_args),
        ("arity", drop_arg),
        ("field", rename_field),
        ("ret", retype_return),
    ];
    let mut out = Vec::new();
    for (file_index, (_, path, text)) in files.iter().enumerate() {
        // Relative to the corpus root, not just the file name: several
        // corpora hold more than one `v1.sail`, and a bare name would make
        // two different mutants share an oracle entry.
        let name = path.strip_prefix(root).unwrap_or(path).to_str().unwrap_or("?").to_string();
        for (kind, f) in kinds {
            let sites = f(text);
            if sites.is_empty() {
                continue;
            }
            // Spread the picks across the file rather than clustering at the
            // top, where the includes and the `default Order` live.
            let step = sites.len().div_ceil(PER_FILE_PER_KIND);
            for (n, (_, mutated, what)) in sites.into_iter().enumerate().step_by(step.max(1)) {
                out.push(Mutant {
                    id: format!("{name}#{kind}#{n}"),
                    file_index,
                    text: mutated,
                    what,
                });
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Workspace loading — same shape as corpus_check.rs
// ---------------------------------------------------------------------------

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
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/sail-lib"),
        PathBuf::from("crates/sail-lsp/data/sail-lib"),
        PathBuf::from("data/sail-lib"),
    ];
    candidates.into_iter().find(|c| c.is_dir()).and_then(|p| p.canonicalize().ok())
}

type LoadedWorkspace = (Analysis, Vec<(Url, FileId)>);

fn load_workspace(
    lib_files: &[(Url, PathBuf, String)],
    project_files: Vec<(Url, PathBuf, String)>,
) -> LoadedWorkspace {
    let mut all: Vec<(Url, PathBuf, String)> = lib_files.to_vec();
    let project_start = all.len();
    all.extend(project_files);

    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    let mut vfs = vfs::Vfs::default();
    for (url, _, text) in &all {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    let roots: Vec<_> = all[project_start..]
        .iter()
        .map(|(url, _, _)| vfs.lookup_file_id_by_url(url).unwrap())
        .collect();
    change.set_source_project(base_db::preprocess::SourceProject {
        roots: roots.clone(),
        paths: all
            .iter()
            .map(|(url, path, _)| (path.clone(), vfs.lookup_file_id_by_url(url).unwrap()))
            .collect(),
        library: find_sail_lib(),
        modules: base_db::module_graph::ModuleGraph {
            modules: vec![base_db::module_graph::ModuleData { files: roots, requires: vec![] }],
            globals: vec![],
        },
        ..Default::default()
    });
    host.apply_change(change);
    let files = host.raw_database().files().clone();
    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    let mut url_fids = Vec::new();
    for (url, _, _) in &all {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_to_fid.insert(url.clone(), fid);
            fid_to_url.insert(fid, url.clone());
            url_fids.push((url.clone(), fid));
        }
    }
    let url_map = UrlMap::new(url_to_fid.clone(), fid_to_url);
    // The include graph the server builds, by the same function: an
    // `Analysis` without one is a different program, and `$include` cycles
    // are reported only by a holder of the whole graph.
    let paths: Vec<(Url, std::path::PathBuf)> =
        all.iter().map(|(url, p, _)| (url.clone(), p.clone())).collect();
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
    let project_url_fids = url_fids.split_off(project_start);
    (analysis, project_url_fids)
}

/// Whether sail-lsp reports any error on the mutated program.
fn reports_error(
    lib_files: &[(Url, PathBuf, String)],
    project: Vec<(Url, PathBuf, String)>,
) -> bool {
    let (analysis, url_fids) = load_workspace(lib_files, project);
    let config = ide_diagnostics::DiagnosticsConfig::new();
    url_fids.iter().any(|(_, fid)| {
        analysis
            .file_diagnostics(*fid, &config)
            .expect("no other thread is here to cancel it")
            .iter()
            .any(|d| d.severity == ide_diagnostics::Severity::Error)
    })
}

// ---------------------------------------------------------------------------
// Oracle
// ---------------------------------------------------------------------------

/// Mutant ids upstream Sail rejects. Anything absent either was not recorded
/// or stayed valid under mutation; both are excluded from the score.
fn read_oracle(path: &Path) -> HashSet<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        eprintln!("no oracle at {} — run with --regen-oracle first", path.display());
        std::process::exit(1);
    };
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter_map(|l| l.strip_prefix("REJECTED "))
        // `REJECTED <id>\t<description>` — only the id identifies the mutant.
        .map(|l| l.split('\t').next().unwrap_or(l).trim().to_string())
        .collect()
}

fn regen_oracle(
    files: &[(Url, PathBuf, String)],
    mutants: &[Mutant],
    out_path: &Path,
) -> std::io::Result<()> {
    let sail = std::env::var("SAIL").unwrap_or_else(|_| "sail".to_string());
    let tmp = std::env::temp_dir().join("sail-lsp-mutation-oracle");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;

    let probe = tmp.join("probe.sail");
    std::fs::write(
        &probe,
        "default Order dec\n$include <prelude.sail>\nfunction probe() -> int = 1\n",
    )?;
    let checked = std::process::Command::new(&sail)
        .args(["--no-memo-z3", "--just-check"])
        .arg(&probe)
        .output()?;
    if !checked.status.success() {
        return Err(std::io::Error::other(format!(
            "Upstream oracle preflight failed: {}",
            String::from_utf8_lossy(&checked.stderr)
        )));
    }

    // A file upstream already rejects cannot say anything about a mutation
    // of it: every mutant of it is "rejected", for a reason that was there
    // before the mutation. Ten of `typecheck/pass`'s files are like this —
    // `config_bits_types`, the `enum_map` variants and the rest need flags
    // this invocation does not pass — and scoring their mutants counted 12
    // defects sail-lsp does not have. The same intersection
    // `docs/upstream-accepts.txt` exists for.
    let mut unusable: std::collections::HashSet<usize> = Default::default();
    for (fi, (_, path, text)) in files.iter().enumerate() {
        let name = path.file_name().unwrap();
        let dir = tmp.join(format!("base{fi}"));
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(name), text)?;
        let ok = std::process::Command::new(&sail)
            .arg("--no-memo-z3")
            .arg("--just-check")
            .arg(name)
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            unusable.insert(fi);
        }
    }
    eprintln!(
        "{} of {} files are rejected unmutated; their mutants are not scored",
        unusable.len(),
        files.len()
    );

    let mut lines = Vec::new();
    for (i, m) in mutants.iter().enumerate() {
        if unusable.contains(&m.file_index) {
            lines.push(format!("SKIPPED {}", m.id));
            continue;
        }
        let (_, path, _) = &files[m.file_index];
        let name = path.file_name().unwrap();
        let dir = tmp.join(i.to_string());
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join(name), &m.text)?;
        let status = std::process::Command::new(&sail)
            .arg("--no-memo-z3")
            .arg("--just-check")
            .arg(name)
            .current_dir(&dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let rejected = match status {
            Ok(s) => !s.success(),
            Err(e) => {
                eprintln!("could not run `{sail}`: {e}");
                std::process::exit(1);
            }
        };
        lines.push(format!(
            "{} {}\t{}",
            if rejected { "REJECTED" } else { "ACCEPTED" },
            m.id,
            m.what
        ));
        if (i + 1) % 25 == 0 {
            eprintln!("  oracle: {}/{}", i + 1, mutants.len());
        }
    }
    let header = "\
# Which mutants upstream Sail rejects — the oracle for `mutation_check`.
#
# Regenerate with:
#   cargo run --release --example mutation_check -- \\
#       <corpus> --regen-oracle docs/mutation-oracle.txt
#
# ACCEPTED means the mutation left a valid program; those are excluded from
# the score rather than counted as misses.
";
    std::fs::write(out_path, format!("{header}{}\n", lines.join("\n")))?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flag_value = |name: &str| -> Option<PathBuf> {
        args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).map(PathBuf::from)
    };
    let verbose = args.iter().any(|a| a == "-v" || a == "--verbose");
    let regen = flag_value("--regen-oracle");
    let oracle_path = flag_value("--oracle");
    let dump_missed = flag_value("--dump-missed");
    let root = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .filter(|a| Path::new(a).is_dir())
        .cloned()
        .unwrap_or_else(|| {
            eprintln!(
                "Usage: mutation_check <corpus-dir> [--oracle FILE | --regen-oracle FILE] [-v]"
            );
            std::process::exit(1);
        });

    // File URLs require absolute paths; otherwise a relative corpus silently
    // produces zero files in discover_sail_files.
    let root = PathBuf::from(root).canonicalize().expect("resolve corpus directory");
    let handle = std::thread::Builder::new()
        .name("mutation-check".into())
        .stack_size(256 * 1024 * 1024)
        .spawn(move || run(&root, regen, oracle_path, verbose, dump_missed))
        .expect("failed to spawn check thread");
    std::process::exit(handle.join().unwrap());
}

fn run(
    root: &Path,
    regen: Option<PathBuf>,
    oracle: Option<PathBuf>,
    verbose: bool,
    dump_missed: Option<PathBuf>,
) -> i32 {
    let files = discover_sail_files(root);
    let mutants = build_mutants(root, &files);
    eprintln!("{} files, {} mutants", files.len(), mutants.len());

    if let Some(out) = regen {
        if let Err(e) = regen_oracle(&files, &mutants, &out) {
            eprintln!("failed to write oracle: {e}");
            return 1;
        }
        eprintln!("wrote {}", out.display());
        return 0;
    }

    let Some(oracle_path) = oracle else {
        eprintln!("pass --oracle FILE (or --regen-oracle FILE to build one)");
        return 1;
    };
    let rejected = read_oracle(&oracle_path);

    let lib_files = find_sail_lib().map(|d| discover_sail_files(&d)).unwrap_or_default();
    if lib_files.is_empty() {
        eprintln!("Warning: sail-lib not found; results will not mean anything.");
    }

    let t0 = std::time::Instant::now();
    let (mut caught, mut missed) = (0usize, 0usize);
    let mut misses: Vec<&Mutant> = Vec::new();
    // Per operator, because one percentage over five of them says nothing
    // about any: the figure was 90.4% when only widths and ranges were
    // measured, and adding argument order, arity, field names and return
    // types put it at 57.3% without a line of checker code changing.
    let mut by_kind: HashMap<&str, (usize, usize)> = HashMap::new();
    for m in &mutants {
        if !rejected.contains(&m.id) {
            continue;
        }
        let (url, path, _) = &files[m.file_index];
        let project = vec![(url.clone(), path.clone(), m.text.clone())];
        let kind = m.id.rsplit('#').nth(1).unwrap_or("?");
        let slot = by_kind.entry(kind).or_default();
        if reports_error(&lib_files, project) {
            caught += 1;
            slot.0 += 1;
        } else {
            missed += 1;
            slot.1 += 1;
            misses.push(m);
        }
    }

    let scored = caught + missed;
    println!("\n{}", "=".repeat(59));
    println!("MUTATION CHECK ({} ms)", t0.elapsed().as_millis());
    println!("{}", "=".repeat(59));
    if scored == 0 {
        println!("  no scorable mutants — is the oracle stale?");
        return 1;
    }
    println!("  scored:  {scored} mutants upstream rejects");
    println!("  caught:  {caught} ({:.1}%)", 100.0 * caught as f64 / scored as f64);
    println!("  missed:  {missed}");
    println!();
    let mut kinds: Vec<_> = by_kind.into_iter().collect();
    kinds.sort_by_key(|(k, _)| *k);
    for (kind, (c, m)) in kinds {
        let n = c + m;
        println!("    {kind:<10} {c:>4}/{n:<4} ({:.0}%)", 100.0 * c as f64 / n as f64);
    }
    // `--dump-missed DIR` writes each missed mutant out, so upstream's own
    // message for it can be read rather than guessed at. Grouping 42 misses
    // by what `sail --just-check` says about them is what tells a checkable
    // shape from the constraint solver's business.
    if let Some(dir) = dump_missed {
        let _ = std::fs::create_dir_all(&dir);
        for m in &misses {
            let (_, path, _) = &files[m.file_index];
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("x");
            let tag = m.id.replace(['/', '#'], "_");
            let _ = std::fs::write(dir.join(format!("{tag}.sail")), &m.text);
            let _ = std::fs::write(dir.join(format!("{tag}.orig")), stem);
        }
        println!("\n  dumped {} missed mutants to {}", misses.len(), dir.display());
    }
    if verbose && !misses.is_empty() {
        println!("\n## MISSED");
        for m in &misses {
            println!("  {}\t{}", m.id, m.what);
        }
    }
    0
}
