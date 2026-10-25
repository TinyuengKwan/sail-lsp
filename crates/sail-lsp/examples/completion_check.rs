//! Completion check: when a name is typed, is it offered?
//!
//! The sixth probe over the corpus, and the first one that looks at
//! completion at all. The other five say nothing about it: `corpus_check`
//! and `conformance_check` score diagnostics, `mutation_check` scores
//! diagnostics on mutated files, `analysis_stats` measures goto-definition
//! and hover, and `protocol_check` measures ranges. Completion is the
//! feature an editor user touches most often per keystroke and the one
//! nothing here could put a number on.
//!
//! ## The oracle is the corpus
//!
//! There is no upstream tool to differ against — `sail` has no completion —
//! so the corpus is its own oracle, the same way `mutation_check` uses a
//! mutated file's rejection: **at a name that is already written in the
//! source, typing its first characters must offer that name.** The source
//! says what the right answer is, and it says it 3,000 times.
//!
//! The cursor is placed after the first `--prefix` characters of an
//! identifier that is a *reference* — an `IDENT_EXPR` or a `TYPE_NAMED`, not
//! a declaration's own name and not a binding, because completing a name
//! being *invented* has no right answer. `completion_prefix` reads
//! backwards from the cursor only, so no edit is needed to simulate typing:
//! the prefix the engine sees is the prefix an editor would send.
//!
//! ## What is measured
//!
//! - **offered**: the written name is somewhere in the list. This is
//!   recall, and it is the number that matters: an item that is not offered
//!   cannot be chosen however good the ranking is.
//! - **rank**: where it sits in the list the client shows. Items arrive
//!   sorted by relevance and carry a `sort_text` that preserves that order,
//!   so the index *is* what the user scrolls past.
//! - **size**: how many items come back. The list is truncated to 200 by
//!   `deduplicate_and_sort`, so a name ranked past that is dropped outright
//!   — size and rank are one question, not two.
//! - **latency**: per query, p50 and p95. Completion runs on a keystroke;
//!   it is the one feature where a millisecond figure is a correctness
//!   figure.
//!
//! ## Usage
//!
//! ```sh
//! cargo run --release --example completion_check -- /path/to/sail-riscv
//! cargo run --release --example completion_check -- --list /path/to/sail-riscv
//! cargo run --release --example completion_check -- --prefix 1 /path/to/sail-riscv
//! ```

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use base_db::{FileId, Files};
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use url::Url;

/// How many identifiers to ask about. The same cap `analysis_stats` uses for
/// goto-definition and hover, so the three percentages describe one sample.
const IDENT_SAMPLE: usize = 3000;

// ---------------------------------------------------------------------------
// Workspace loading — shared pattern with analysis_stats.rs and corpus_check.rs
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

type LoadedFiles = Vec<(Url, FileId)>;

/// Project files and prelude, kept apart: the sample is the project's own
/// names, the index is everything the server would have.
fn load_workspace(root: &Path) -> (AnalysisHost, Files, LoadedFiles, LoadedFiles) {
    let mut all_sail_files = Vec::new();
    if let Some(lib_dir) = find_sail_lib() {
        all_sail_files.extend(discover_sail_files(&lib_dir));
    }
    let project_start = all_sail_files.len();
    all_sail_files.extend(discover_sail_files(root));

    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    let mut vfs = vfs::Vfs::default();
    for (url, _path, text) in &all_sail_files {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    host.apply_change(change);
    let files = host.raw_database().files().clone();

    {
        let db = host.raw_database_mut();
        let file_texts: Vec<base_db::FileText> = all_sail_files
            .iter()
            .filter_map(|(url, _, _)| files.file_text(vfs.lookup_file_id_by_url(url)?))
            .collect();
        hir::def_query::set_workspace_files(db, file_texts);
    }

    let mut url_fids = Vec::new();
    for (url, _path, _text) in &all_sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_fids.push((url.clone(), fid));
        }
    }
    let project_url_fids = url_fids.split_off(project_start);
    (host, files, project_url_fids, url_fids)
}

fn make_analysis(host: &AnalysisHost, files: &Files, url_fids: &[(Url, FileId)]) -> Analysis {
    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    for (url, fid) in url_fids {
        url_to_fid.insert(url.clone(), *fid);
        fid_to_url.insert(*fid, url.clone());
    }
    let url_map = UrlMap::new(url_to_fid.clone(), fid_to_url);
    let paths: Vec<(Url, PathBuf)> = url_fids
        .iter()
        .filter_map(|(url, _)| url.to_file_path().ok().map(|p| (url.clone(), p)))
        .collect();
    let graph = ide::analysis::build_include_graph(
        host.raw_database(),
        &paths,
        &url_to_fid,
        find_sail_lib().as_deref(),
    );
    Analysis::with_include_graph(
        host.raw_database().clone(),
        files.clone(),
        url_map,
        std::sync::Arc::new(graph),
    )
}

// ---------------------------------------------------------------------------
// The sample
// ---------------------------------------------------------------------------

/// Is this identifier a *reference* to a name declared somewhere?
///
/// Only a reference has a right answer. A definition's own name and a
/// pattern binding are names being *invented*, and asking what the editor
/// would have offered while they were typed measures nothing — the answer is
/// whatever else is in scope, and none of it is right.
///
/// `IDENT_EXPR` and `TYPE_NAMED` are the two nodes the parser builds for a
/// use: a value or callee in an expression, and a type by name. A
/// declaration's name sits as a bare `IDENT` under its definition node and a
/// binding under `IDENT_PAT`, so neither is in the sample.
fn is_reference(tok: &syntax::SyntaxToken) -> bool {
    use parser::SyntaxKind as SK;
    let Some(parent) = tok.parent() else { return false };
    if !matches!(parent.kind(), SK::IDENT_EXPR | SK::TYPE_NAMED) {
        return false;
    }
    // `enum clause extension = Ext_M` declares `Ext_M`, and the parser puts
    // the new member in an `IDENT_EXPR` like any other name — the shape
    // alone cannot tell a scattered clause's *declaration* from a use. A
    // member being invented has no right answer either.
    let declares = parent.parent().is_some_and(|g| g.kind() == SK::SCATTERED_CLAUSE_DEF);
    !declares
}

/// Names that denote nothing a completion list could contain.
///
/// The same exclusions `analysis_stats` makes for goto-definition, and for
/// the same reason: `config extensions.V.vlen_exp` names keys in a JSON
/// file, and a built-in type constructor is declared in upstream's
/// `type_env.ml` rather than in any `.sail` file. Scoring them would measure
/// the probe's sample, not the feature. Built-in type names *are* offered
/// (they are in `SAIL_BUILTINS`), so only the config keys are dropped.
fn is_config_key_segment(tok: &syntax::SyntaxToken) -> bool {
    use parser::SyntaxKind as SK;
    let Some(mut node) = tok.parent() else { return false };
    if node.kind() == SK::CONFIG_EXPR {
        return true;
    }
    while node.kind() == SK::FIELD_ACCESS_EXPR {
        let Some(child) = node.first_child() else { return false };
        if child.kind() == SK::CONFIG_EXPR {
            return true;
        }
        node = child;
    }
    false
}

struct Miss {
    name: String,
    file: String,
    line: usize,
    context: String,
    offered_size: usize,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list = args.iter().any(|a| a == "--list");
    let prefix_len: usize = args
        .iter()
        .position(|a| a == "--prefix")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse().ok())
        .unwrap_or(3);
    let root = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-') && a.parse::<usize>().is_err())
        .cloned()
        .unwrap_or_else(|| {
            eprintln!("Usage: completion_check [--list] [--prefix N] /path/to/corpus");
            std::process::exit(2);
        });

    let t0 = Instant::now();
    let (host, files, url_fids, prelude_url_fids) = load_workspace(&PathBuf::from(&root));
    let analysis = make_analysis(
        &host,
        &files,
        &url_fids.iter().chain(prelude_url_fids.iter()).cloned().collect::<Vec<_>>(),
    );
    let t_load = t0.elapsed();

    let mut asked = 0usize;
    let mut offered = 0usize;
    let mut ranks: Vec<usize> = Vec::new();
    let mut sizes: Vec<usize> = Vec::new();
    let mut times: Vec<u128> = Vec::new();
    let mut misses: Vec<Miss> = Vec::new();
    let mut skipped_config = 0usize;
    let mut by_context: HashMap<String, (usize, usize)> = HashMap::new();

    'files: for (_url, fid) in &url_fids {
        let Some(text) = files.file_text(*fid).map(|t| t.text(analysis.db()).to_string()) else {
            continue;
        };
        let name_of_file = analysis
            .url_for_file_id(*fid)
            .and_then(|u| u.to_file_path().ok())
            .map(|p| p.file_name().unwrap_or_default().to_string_lossy().to_string())
            .unwrap_or_default();
        let line_index = ide_db::line_index::LineIndex::new(&text);
        let (root_node, _) = syntax::parse_text(&text);
        for tok in root_node.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() != parser::SyntaxKind::IDENT || !is_reference(&tok) {
                continue;
            }
            if is_config_key_segment(&tok) {
                skipped_config += 1;
                continue;
            }
            if asked >= IDENT_SAMPLE {
                break 'files;
            }
            let name = tok.text().to_string();
            let start = usize::from(tok.text_range().start());
            // The cursor after `prefix_len` characters of the name, which is
            // the buffer an editor has once that much is typed.
            let cut = name.char_indices().nth(prefix_len).map(|(i, _)| i).unwrap_or(name.len());
            let pos = line_index.line_col(start + cut);

            asked += 1;
            let t = Instant::now();
            let items =
                analysis.completions(*fid, pos, &Default::default()).expect("nothing cancels this");
            times.push(t.elapsed().as_micros());
            sizes.push(items.len());

            let context =
                tok.parent().map(|p| format!("{:?}", p.kind())).unwrap_or_else(|| "?".to_string());
            let entry = by_context.entry(context.clone()).or_insert((0, 0));
            entry.0 += 1;
            match items.iter().position(|i| i.label == name) {
                Some(rank) => {
                    offered += 1;
                    entry.1 += 1;
                    ranks.push(rank);
                }
                None => misses.push(Miss {
                    name,
                    file: name_of_file.clone(),
                    line: line_index.line_col(start).line as usize + 1,
                    context,
                    offered_size: items.len(),
                }),
            }
        }
    }

    let pct = |n: usize, d: usize| if d == 0 { 0.0 } else { n as f64 * 100.0 / d as f64 };
    let quantile = |v: &mut Vec<u128>, q: f64| -> u128 {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[((v.len() as f64 - 1.0) * q) as usize]
    };
    let quantile_us = |v: &mut Vec<usize>, q: f64| -> usize {
        if v.is_empty() {
            return 0;
        }
        v.sort_unstable();
        v[((v.len() as f64 - 1.0) * q) as usize]
    };

    if list {
        println!("## NOT OFFERED (the name is written here, completion does not offer it)");
        for m in &misses {
            println!("{}\t{}:{}\t{}\t{} items", m.name, m.file, m.line, m.context, m.offered_size);
        }
        println!();
    }

    let top = |n: usize| ranks.iter().filter(|r| **r < n).count();

    println!("============================================================");
    println!("COMPLETION CHECK (prefix {prefix_len}, {} ms to load)", t_load.as_millis());
    println!("============================================================");
    println!("  asked:            {asked} references");
    println!("  offered:          {offered} ({:.1}%)", pct(offered, asked));
    println!("  not offered:      {}", misses.len());
    println!();
    println!("  rank  top 1:      {} ({:.1}%)", top(1), pct(top(1), asked));
    println!("  rank  top 5:      {} ({:.1}%)", top(5), pct(top(5), asked));
    println!("  rank  top 20:     {} ({:.1}%)", top(20), pct(top(20), asked));
    println!();
    println!(
        "  list size:        p50 {}   p95 {}   max {}",
        quantile_us(&mut sizes.clone(), 0.5),
        quantile_us(&mut sizes.clone(), 0.95),
        sizes.iter().copied().max().unwrap_or(0)
    );
    println!(
        "  latency:          p50 {} us   p95 {} us   max {} us",
        quantile(&mut times.clone(), 0.5),
        quantile(&mut times.clone(), 0.95),
        times.iter().copied().max().unwrap_or(0)
    );
    if skipped_config > 0 {
        println!();
        println!("  {skipped_config} config-key segments left out of the sample");
    }

    if !misses.is_empty() {
        println!();
        println!("  Not offered, by syntactic context:");
        let mut rows: Vec<(&String, &(usize, usize))> = by_context.iter().collect();
        rows.sort_by_key(|(_, (n, hit))| n - hit);
        rows.reverse();
        for (ctx, (n, hit)) in rows {
            if n > hit {
                println!("    {ctx:<24} {:>5} of {n}", n - hit);
            }
        }
        let mut names: HashMap<&str, usize> = HashMap::new();
        for m in &misses {
            *names.entry(m.name.as_str()).or_default() += 1;
        }
        let mut names: Vec<(&str, usize)> = names.into_iter().collect();
        names.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        println!();
        println!("  The names most often missing:");
        for (name, n) in names.iter().take(10) {
            println!("    {name:<32} {n}");
        }
    }

    println!();
    println!("METRIC:completion_offered_pct:{}:%", pct(offered, asked) as u64);
    println!("METRIC:completion_top1_pct:{}:%", pct(top(1), asked) as u64);
    println!("METRIC:completion_p95_us:{}:us", quantile(&mut times.clone(), 0.95));
    // No exit code: unlike a false positive there is no clean line here yet,
    // and a probe that always failed would be read as noise. The METRIC
    // lines are the interface, as they are for `analysis_stats`.
}
