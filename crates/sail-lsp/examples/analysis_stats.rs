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
use hir::callgraph::SourceFileInfo as _;
use ide::analysis::{Analysis, AnalysisHost, UrlMap};
use url::Url;

#[path = "support/constraint_stats.rs"]
mod constraint_stats;

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
    // Anchored on the crate, not the cwd: resolving relative to the cwd
    // silently drops the prelude when the tool is run from the corpus
    // directory, and every number it prints then measures nothing.
    let candidates = [
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("data/sail-lib"),
        PathBuf::from("crates/sail-lsp/data/sail-lib"),
        PathBuf::from("data/sail-lib"),
    ];
    candidates.into_iter().find(|c| c.is_dir()).and_then(|p| p.canonicalize().ok())
}

/// `(url, file id)` for a set of loaded files.
type LoadedFiles = Vec<(Url, FileId)>;

/// Load workspace: project files + sail-lib prelude into salsa database.
///
/// Returns `AnalysisHost` + `Files` + the project's files + the prelude's,
/// kept apart because the sample is the project and the index is everything.
/// Builds a `UrlMap` for cross-file workspace name resolution.
fn load_workspace(root: &Path) -> (AnalysisHost, Files, LoadedFiles, LoadedFiles) {
    let mut all_sail_files = Vec::new();

    // Load sail-lib first (prelude types)
    if let Some(lib_dir) = find_sail_lib().filter(|_| root.is_dir()) {
        let lib_files = discover_sail_files(&lib_dir);
        all_sail_files.extend(lib_files);
    }

    // Then project files
    // A manifest argument exercises the same pure resolver as the server.
    // Directory arguments retain the historical whole-corpus benchmark.
    let mut resolved_project = None;
    let project_files: Vec<(Url, PathBuf, String)> = if root.is_file() {
        let source = std::fs::read_to_string(root).expect("read project manifest");
        let project = project_model::ProjectManifest::parse(&source)
            .and_then(|m| m.resolve(&project_model::ProjectOptions::default()))
            .expect("resolve project manifest");
        let files = project
            .files
            .iter()
            .map(|relative| {
                let path = root.parent().unwrap().join(relative);
                let text = std::fs::read_to_string(&path).expect("read active project source");
                let url = Url::from_file_path(&path).expect("project source URL");
                (url, path, text)
            })
            .collect();
        resolved_project = Some(project);
        files
    } else {
        discover_sail_files(root)
    };
    if root.is_file() {
        let roots: Vec<_> = project_files.iter().map(|(_, p, _)| p.clone()).collect();
        for path in
            hir::preprocess::discover_candidates(&roots, find_sail_lib().as_deref(), &[], |p| {
                std::fs::read_to_string(p).ok()
            })
        {
            if !roots.contains(&path) {
                let text = std::fs::read_to_string(&path).expect("read include candidate");
                all_sail_files.push((Url::from_file_path(&path).unwrap(), path, text));
            }
        }
    }
    let project_start = all_sail_files.len();
    all_sail_files.extend(project_files);

    let mut host = AnalysisHost::new();
    let mut change = ide_db::apply_change::Change::new();
    let mut vfs = vfs::Vfs::default();

    for (url, _path, text) in &all_sail_files {
        let fid = vfs.file_id_for_url(url);
        change.change_file(fid, text.clone());
    }
    if let Some(project) = resolved_project {
        let modules: Vec<_> = project
            .modules
            .iter()
            .enumerate()
            .map(|(id, m)| {
                let files = if project.module_order.contains(&id) {
                    m.files
                        .iter()
                        .filter_map(|p| {
                            let url = Url::from_file_path(root.parent().unwrap().join(p)).ok()?;
                            vfs.lookup_file_id_by_url(&url)
                        })
                        .collect()
                } else {
                    Vec::new()
                };
                base_db::module_graph::ModuleData { files, requires: m.requires.clone() }
            })
            .collect();
        let owned: std::collections::HashSet<_> =
            modules.iter().flat_map(|m| m.files.iter().copied()).collect();
        let globals = all_sail_files
            .iter()
            .filter_map(|(u, _, _)| vfs.lookup_file_id_by_url(u))
            .filter(|id| !owned.contains(id))
            .collect();
        let graph = base_db::module_graph::ModuleGraph { modules, globals };
        change.set_source_project(base_db::preprocess::SourceProject {
            roots: project
                .module_order
                .iter()
                .flat_map(|&id| graph.modules[id].files.iter().copied())
                .collect(),
            root_modules: project
                .module_order
                .iter()
                .flat_map(|&id| graph.modules[id].files.iter().map(move |_| Some(id)))
                .collect(),
            paths: all_sail_files
                .iter()
                .filter_map(|(u, p, _)| vfs.lookup_file_id_by_url(u).map(|id| (p.clone(), id)))
                .collect(),
            library: find_sail_lib(),
            include_paths: Vec::new(),
            target: None,
            modules: graph,
        });
    }
    host.apply_change(std::mem::take(&mut change));
    let files = host.raw_database().files().clone();

    // Directory mode retains the historical whole-corpus benchmark.
    if root.is_dir() {
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

    let mut url_fids = Vec::new();
    for (url, _path, _text) in &all_sail_files {
        if let Some(fid) = vfs.lookup_file_id_by_url(url) {
            url_fids.push((url.clone(), fid));
        }
    }

    // The sample is the project's own identifiers, so the loops iterate
    // these. The *index* gets everything — the server indexes the prelude
    // too, and scoring `Ok`, `None` or `option` as unresolvable because the
    // probe left their declarations out measures the probe, not the feature.
    let project_url_fids = url_fids.split_off(project_start);
    if let Some(ws) = base_db::WorkspaceFiles::try_get(host.raw_database()) {
        let active: std::collections::HashSet<_> = ws
            .file_texts(host.raw_database())
            .iter()
            .map(|f| f.file_id(host.raw_database()))
            .collect();
        url_fids.retain(|(_, id)| active.contains(id));
    }
    println!("Active included/library files: {}", url_fids.len());

    (host, files, project_url_fids, url_fids)
}

/// Build an Analysis with UrlMap from host + files + url_fids.
/// Whether this identifier is a segment of a `config` key path.
///
/// `config extensions.S.supported : bool` names keys in a JSON file, not
/// definitions in the source, so there is nothing for navigation to point at
/// and counting them as misses understates the feature. They are excluded
/// from the sample rather than silently scored — the run prints how many.
///
/// The shape matters: the parser takes `config Id` as the whole
/// `CONFIG_EXPR` and leaves the dotted tail as postfix field access, so the
/// tree is `FIELD_ACCESS_EXPR(FIELD_ACCESS_EXPR(CONFIG_EXPR, S), supported)`
/// and the `CONFIG_EXPR` is the innermost *left child*, never an ancestor.
/// Testing the parent, or any ancestor, catches only the first segment; the
/// other 98 stayed in the sample as field-access misses.
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

/// An identifier navigation has nowhere to send you.
///
/// Not a miss, and not the same exclusion as a config key: these *are*
/// definitions of a sort, they just have no definition **site**. Upstream
/// declares Sail's built-in type constructors in `type_env.ml`'s
/// `builtin_typs` table, not in any `.sail` file; `__FILE__` and `__LINE__`
/// are constants the type checker supplies; an attribute name and the
/// backend name in `val f = pure {cpp: "f"}` are not identifiers at all.
///
/// Scoring them made goto-definition read 94.6% when 157 of its 163 "misses"
/// were questions with no answer. Excluded from the goto sample only —
/// **hover still has something to say about a built-in type**, and counting
/// it there is the point.
fn goto_has_no_target(tok: &syntax::SyntaxToken) -> bool {
    use parser::SyntaxKind as SK;
    let Some(parent) = tok.parent() else { return false };
    let text = tok.text();

    // A built-in type constructor, in a type position. `int` as a *value*
    // name would be an ordinary identifier, so the parent has to say type.
    if parent.kind() == SK::TYPE_NAMED && hir::is_builtin_type_name(text) {
        return true;
    }
    // The type-level numeric functions, same table (`type_env.ml:387`).
    if parent.kind() == SK::TYPE_FORALL && hir::is_builtin_type_level_fn(text) {
        return true;
    }
    // `__FILE__` / `__LINE__`.
    if text.starts_with("__") && text.ends_with("__") {
        return true;
    }
    // An attribute's name: `$[property]`, `$[test ..]`.
    if parent.kind() == SK::ATTRIBUTE {
        return true;
    }
    // The backend name in an extern binding: `val f = pure {cpp: "f"}`.
    // It is a key in a brace-delimited list, so a `:` follows it.
    if parent.kind() == SK::CALLABLE_SPEC {
        let next = tok
            .next_sibling_or_token()
            .and_then(|el| el.into_token())
            .filter(|t| !t.kind().is_trivia())
            .or_else(|| {
                tok.next_sibling_or_token()
                    .and_then(|el| el.into_token())
                    .and_then(|t| t.next_token())
            });
        if next.is_some_and(|t| t.kind() == SK::COLON) {
            return true;
        }
    }
    false
}

fn make_analysis(host: &AnalysisHost, files: &Files, url_fids: &[(Url, FileId)]) -> Analysis {
    let mut url_to_fid = HashMap::new();
    let mut fid_to_url = HashMap::new();
    for (url, fid) in url_fids {
        url_to_fid.insert(url.clone(), *fid);
        fid_to_url.insert(*fid, url.clone());
    }
    let url_map = UrlMap::new(url_to_fid.clone(), fid_to_url);
    // The include graph the server builds, by the same function: an
    // `Analysis` without one is a different program.
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
// Main
// ---------------------------------------------------------------------------

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
    let run_ide = args.iter().any(|a| a == "--run-all-ide-things");
    let mut root = None;
    let mut constraint_path = None;
    let mut options = args.iter().skip(1);
    while let Some(arg) = options.next() {
        match arg.as_str() {
            "--constraint-report" => {
                constraint_path = Some(PathBuf::from(
                    options.next().expect("--constraint-report requires an output path"),
                ))
            }
            "--verbose" | "-v" | "--infer-only" | "--diagnostics-only" | "--run-all-ide-things" => {
            }
            _ if arg.starts_with('-') => panic!("Unknown option: {arg}"),
            _ => assert!(root.replace(arg).is_none(), "Only one workspace path is allowed"),
        }
    }
    let root = root.expect("Usage: analysis_stats [--verbose] [--infer-only | --diagnostics-only] [--run-all-ide-things] [--constraint-report output.json] /path/to/sail-project");
    let root = PathBuf::from(root).canonicalize().expect("workspace path must exist");

    println!("=======================================================");
    println!("sail-lsp analysis-stats");
    println!("=======================================================");

    // 1. Workspace loading
    let t0 = Instant::now();
    let (host, files, url_fids, prelude_url_fids) = load_workspace(&root);
    let t_load = t0.elapsed();
    let n_files = url_fids.len();
    println!("\nWorkspace: {} files", n_files);
    println!("Load + salsa inputs: {:>7} ms", t_load.as_millis());
    report_metric("workspace_load", t_load.as_millis() as u64, "ms");

    // The prelude is in the map for the same reason it is in the index: the
    // server sees one file set, and an `Analysis` that cannot name a prelude
    // file answers about a program with no prelude in it. Measured, a
    // project-only map reports 83 diagnostics here against 68 — fifteen of
    // them the probe's own — and drops hover from 98.2% to 97.9%.
    let analysis = make_analysis(
        &host,
        &files,
        &url_fids.iter().chain(prelude_url_fids.iter()).cloned().collect::<Vec<_>>(),
    );

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
            if let Some(tree) = hir::def_query::file_item_tree(analysis.db(), ft).as_ref() {
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
            if let Some(bodies) = hir::def_query::callable_bodies(analysis.db(), ft).as_ref() {
                body_count += bodies.0.entries().len();
            }
        }
    }
    let t_bodies = t0.elapsed();
    println!("Body lowering:       {:>7} ms ({} bodies)", t_bodies.as_millis(), body_count);
    report_metric("body_lowering", t_bodies.as_millis() as u64, "ms");
    report_metric("bodies", body_count as u64, "count");

    // Type clauses used to be counted as executable bodies. Report their
    // separate declaration count so coverage baselines can be reconciled.
    let type_clauses: usize = url_fids
        .iter()
        .filter_map(|(_, fid)| files.file_text(*fid))
        .filter_map(|ft| syntax::parse_query::semantic_parse_file(analysis.db(), ft).syntax_node())
        .map(|root| {
            root.children()
                .filter(|node| {
                    node.kind() == parser::SyntaxKind::SCATTERED_CLAUSE_DEF
                        && node.children_with_tokens().any(|t| {
                            matches!(
                                t.kind(),
                                parser::SyntaxKind::KW_ENUM | parser::SyntaxKind::KW_UNION
                            )
                        })
                })
                .count()
        })
        .sum();
    println!("Scattered type clauses: {type_clauses} (declarations, not executable bodies)");
    report_metric("scattered_type_clauses", type_clauses as u64, "count");

    // 4. Type inference for all callables
    let t0 = Instant::now();
    let mut constraint_report =
        constraint_path.as_ref().map(|_| constraint_stats::ConstraintReport::default());
    let mut callable_count = 0usize;
    let mut type_error_count = 0usize;
    // How much of the program inference actually resolves, in
    // rust-analyzer's `analysis-stats` terms: `??ty` is an expression with no
    // type at all, `?ty` one whose type contains an error somewhere (a
    // `bits(?)` whose width never resolved), `!ty` a recorded mismatch.
    let mut num_exprs = 0usize;
    let mut num_exprs_unknown = 0usize;
    let mut num_exprs_never = 0usize;
    let mut num_exprs_partially_unknown = 0usize;
    let mut num_expr_mismatches = 0usize;
    let mut num_exprs_unvisited = 0usize;
    let mut whole_bodies_skipped = 0usize;
    let mut exprs_in_skipped_bodies = 0usize;
    let mut unknown_by_kind: std::collections::HashMap<&'static str, usize> = Default::default();
    let mut unknown_idents: std::collections::HashMap<String, usize> = Default::default();
    let mut unknown_ops: std::collections::HashMap<String, usize> = Default::default();
    let mut unknown_calls: std::collections::HashMap<String, usize> = Default::default();
    let mut slow_callables: Vec<(String, u128)> = Vec::new();
    for (url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let ids = hir::def_query::file_def_with_body_ids(analysis.db(), ft);
            for &id in ids {
                callable_count += 1;
                let tc = Instant::now();
                let tcr = hir::ty_query::infer(analysis.db(), id);
                let elapsed = tc.elapsed().as_millis();
                type_error_count += tcr.0.inference_diagnostics().len();
                type_error_count += tcr.0.type_mismatches.len();
                if verbose {
                    for diagnostic in tcr.0.inference_diagnostics() {
                        if let hir::InferenceDiagnostic::UnresolvedCallQuantifiers {
                            id: callee,
                            quants,
                            ..
                        } = diagnostic
                        {
                            println!(
                                "{url}: {}: unresolved call quantifiers for {callee}: {quants:?}",
                                id.name(analysis.db())
                            );
                        }
                    }
                }
                num_expr_mismatches += tcr.0.expr_type_mismatches_iter().count();
                let body = &hir::def_query::body_with_source_map(analysis.db(), id).0 .0;
                if let Some(report) = &mut constraint_report {
                    let map = &hir::def_query::body_with_source_map(analysis.db(), id).0 .1;
                    let original_base = hir::def_query::callable_body(analysis.db(), id)
                        .as_ref()
                        .map_or(map.def_base, |cb| cb.body.def_span.start);
                    let path = url.to_file_path().expect("workspace file URL");
                    let relative = path.strip_prefix(&root).unwrap_or(&path).to_string_lossy();
                    report.record(
                        &relative,
                        &id.name(analysis.db()),
                        id.clause_index(analysis.db()),
                        ft.text(analysis.db()),
                        body,
                        map,
                        original_base,
                        &tcr.0,
                    );
                }
                let mut body_total = 0usize;
                let mut body_unvisited = 0usize;
                for (expr_id, expr) in body.store.exprs.iter() {
                    num_exprs += 1;
                    body_total += 1;
                    // A diverging expression's type is `error()` and means
                    // *never* — this checker spells the two the same way, so
                    // inference records which is which. Counting them as
                    // unknown said 26 untyped blocks on sail-riscv, most of
                    // them blocks that simply cannot fall through, and the
                    // loop this number drives would chase them for ever.
                    let diverges = tcr.0.diverging_exprs.contains(&expr_id);
                    if diverges {
                        num_exprs_never += 1;
                    }
                    let unknown = !diverges
                        && match tcr.0.expr_ty(expr_id) {
                            None => {
                                num_exprs_unvisited += 1;
                                body_unvisited += 1;
                                true
                            }
                            Some(ty) => ty.is_error(),
                        };
                    let partial = !diverges
                        && tcr
                            .0
                            .expr_ty(expr_id)
                            .is_some_and(|ty| !ty.is_error() && ty.references_error());
                    if unknown || partial {
                        if std::env::var("SAIL_UT_WHERE").is_ok() {
                            let sm = &hir::def_query::body_with_source_map(analysis.db(), id).0 .1;
                            let line = sm
                                .expr_syntax(expr_id)
                                .map(|sp| {
                                    let text = files
                                        .file_text(*fid)
                                        .map(|t| t.text(analysis.db()).to_string())
                                        .unwrap_or_default();
                                    let at = sp.start.min(text.len());
                                    let start = text[..at].rfind('\n').map_or(0, |p| p + 1);
                                    let end =
                                        text[start..].find('\n').map_or(text.len(), |p| start + p);
                                    format!(
                                        "{}:{}",
                                        text[..at].matches('\n').count() + 1,
                                        text[start..end].trim()
                                    )
                                })
                                .unwrap_or_else(|| "?".to_string());
                            eprintln!(
                                "{}\t{}\t{}\t{}\t{:?}",
                                if partial { "PARTIAL" } else { "UT" },
                                expr_kind_name(expr),
                                url,
                                line,
                                tcr.0.expr_ty(expr_id)
                            );
                        }
                    }
                    if unknown {
                        *unknown_by_kind.entry(expr_kind_name(expr)).or_insert(0usize) += 1;
                        match expr {
                            hir::Expr::Ident(name) => {
                                *unknown_idents.entry(name.to_string()).or_insert(0usize) += 1;
                            }
                            hir::Expr::BinaryOp { op, .. } => {
                                *unknown_ops.entry(format!("{op:?}")).or_insert(0usize) += 1;
                            }
                            hir::Expr::Call { callee, .. } => {
                                let name = match body.expr(*callee) {
                                    Some(hir::Expr::Ident(n)) => n.to_string(),
                                    _ => "<non-ident>".to_string(),
                                };
                                *unknown_calls.entry(name).or_insert(0usize) += 1;
                            }
                            _ => {}
                        }
                    }
                    // The same exclusion as `unknown` above: a diverging
                    // expression's `error()` means *never*. Counting it here
                    // and not there is how the headline number and the
                    // by-kind breakdown disagreed by 24.
                    match tcr.0.expr_ty(expr_id) {
                        _ if diverges => {}
                        None => num_exprs_unknown += 1,
                        Some(ty) if ty.is_error() => num_exprs_unknown += 1,
                        Some(ty) if ty.references_error() => num_exprs_partially_unknown += 1,
                        Some(_) => {}
                    }
                }
                if body_total > 0 && body_unvisited == body_total {
                    whole_bodies_skipped += 1;
                    exprs_in_skipped_bodies += body_total;
                }
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
        // Not "type errors": this counts everything inference records, and on
        // sail-riscv 68 of the 69 are the lints it also produces (unused
        // variable, trailing `return`). The mismatch count is `!ty` below.
        "Infer all:           {:>7} ms ({} callables, {} inference findings)",
        t_infer.as_millis(),
        callable_count,
        type_error_count
    );
    let pct = |n: usize, total: usize| -> f64 {
        if total == 0 {
            0.0
        } else {
            n as f64 * 100.0 / total as f64
        }
    };
    println!(
        "  exprs: {}, ??ty: {} ({:.1}%), ?ty: {} ({:.1}%), !ty: {}, never: {}",
        num_exprs,
        num_exprs_unknown,
        pct(num_exprs_unknown, num_exprs),
        num_exprs_partially_unknown,
        pct(num_exprs_partially_unknown, num_exprs),
        num_expr_mismatches,
        num_exprs_never
    );
    report_metric("infer_all", t_infer.as_millis() as u64, "ms");
    report_metric("callables", callable_count as u64, "count");
    report_metric("exprs", num_exprs as u64, "count");
    report_metric("unknown type", num_exprs_unknown as u64, "#");
    report_metric("partially unknown type", num_exprs_partially_unknown as u64, "#");
    report_metric("type mismatches", num_expr_mismatches as u64, "#");
    println!(
        "  of the untyped, {num_exprs_unvisited} were never visited by inference          ({} typed as error)",
        num_exprs_unknown - num_exprs_unvisited
    );
    println!(
        "  {whole_bodies_skipped} bodies were skipped entirely ({exprs_in_skipped_bodies} exprs)"
    );
    if verbose {
        let mut by_kind: Vec<_> = unknown_by_kind.into_iter().collect();
        by_kind.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        let mut idents: Vec<_> = unknown_idents.iter().collect();
        idents.sort_by_key(|(n, c)| (std::cmp::Reverse(**c), (*n).clone()));
        println!("  untyped idents (top 30 of {}):", idents.len());
        for (name, n) in idents.iter().take(30) {
            println!("    {name:<28} {n:>6}");
        }
        let mut ops: Vec<_> = unknown_ops.iter().collect();
        ops.sort_by_key(|(n, c)| (std::cmp::Reverse(**c), (*n).clone()));
        println!("  untyped binary ops (top 20 of {}):", ops.len());
        for (name, n) in ops.iter().take(20) {
            println!("    {name:<44} {n:>6}");
        }
        let mut calls: Vec<_> = unknown_calls.iter().collect();
        calls.sort_by_key(|(n, c)| (std::cmp::Reverse(**c), (*n).clone()));
        println!("  untyped calls (top 25 of {}):", calls.len());
        for (name, n) in calls.iter().take(25) {
            println!("    {name:<28} {n:>6}");
        }
        println!("  untyped expressions by kind:");
        for (kind, n) in by_kind.iter().take(20) {
            println!("    {kind:<24} {n:>7}");
        }
    }
    if let (Some(mut report), Some(path)) = (constraint_report, constraint_path) {
        println!(
            "Deferred obligations: {} proved, {} refuted, {} unknown ({} total)",
            report.totals.proved,
            report.totals.refuted,
            report.totals.unknown,
            report.totals.total()
        );
        report.write(&path, &root).expect("write constraint report");
        println!("Constraint report: {}", path.display());
    }
    if args.iter().any(|a| a == "--infer-only") {
        return;
    }
    // Distribution histogram
    let mut all_times: Vec<u128> = Vec::new();
    // Re-run to collect all times (cheap — salsa cached)
    for (_url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let ids = hir::def_query::file_def_with_body_ids(analysis.db(), ft);
            for &id in ids {
                let tc = Instant::now();
                let _ = hir::ty_query::infer(analysis.db(), id);
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
    let mut by_severity = std::collections::BTreeMap::<String, usize>::new();
    for (_url, fid) in &url_fids {
        let diags =
            analysis.file_diagnostics(*fid, &config).expect("no other thread is here to cancel it");
        diag_count += diags.len();
        for diag in &diags {
            *by_severity.entry(format!("{:?}", diag.severity)).or_default() += 1;
            if verbose {
                println!(
                    "{_url}: {:?}: {} (bytes {:?})",
                    diag.severity, diag.message, diag.range.range
                );
            }
        }
    }
    let t_diag = t0.elapsed();
    println!("Diagnostics all:     {:>7} ms ({} diagnostics)", t_diag.as_millis(), diag_count);
    report_metric("diagnostics_all", t_diag.as_millis() as u64, "ms");
    println!("Diagnostics by severity: {by_severity:?}");
    if args.iter().any(|a| a == "--diagnostics-only") {
        return;
    }

    // 5b. Inlay hints for all files
    let t0 = Instant::now();
    let mut hint_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(sf) = analysis.file_by_id(*fid) {
            let text_len = sf.text().len();
            let range = base_db::text_range(0, text_len);
            if let Ok(hints) =
                analysis.inlay_hints(*fid, range, &ide::inlay_hints::InlayHintsConfig::default())
            {
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

    // 5d. Goto-definition and hover, at identifiers that actually exist.
    //
    // This used to fire at line 3 column 5 of ten files and throw the answer
    // away, which measured latency on what is usually whitespace and said
    // nothing about whether either feature works. Both are now asked at real
    // identifier positions and the answers are counted: a resolve rate is
    // the only thing that distinguishes a working feature from a fast one.
    const IDENT_SAMPLE: usize = 3000;
    // Navigation is asked through the same entry point the server uses, index
    // and all: measuring only the same-file half reported a third of the
    // feature as the whole of it.
    // Index over the prelude as well as the project. The server does, and a
    // name whose declaration is in the prelude — `Ok`, `None`, `option`,
    // `unsigned` — was being scored as unresolvable because the probe's
    // `UrlMap` had the project files only.
    let indexed_analysis = make_analysis(
        &host,
        &files,
        &url_fids.iter().chain(prelude_url_fids.iter()).cloned().collect::<Vec<_>>(),
    );
    // The same index the server reads, and now by the same route: the probe
    // used to hand `Analysis` an index of its own through
    // `with_symbol_index`, and that override is gone — it was the only
    // caller, and the two features that read the field instead of the
    // accessor were dead in the server while this measured them at 99.7%.
    // From `indexed_analysis`, not `analysis`: a navigation target's URL
    // comes from the `UrlMap`, so a handle whose map holds the project only
    // cannot name a prelude file.
    let nav_analysis = indexed_analysis.clone();
    let t0 = Instant::now();
    let (mut goto_asked, mut goto_hit) = (0usize, 0usize);
    let mut config_skipped = 0usize;
    let mut no_target_skipped = 0usize;
    let (mut hover_asked, mut hover_hit) = (0usize, 0usize);
    'files: for (url, fid) in &url_fids {
        let Some(text) = files.file_text(*fid).map(|t| t.text(analysis.db()).to_string()) else {
            continue;
        };
        let line_index = ide_db::line_index::LineIndex::new(&text);
        let (root, _) = syntax::parse_text(&text);
        for tok in root.descendants_with_tokens().filter_map(|el| el.into_token()) {
            if tok.kind() != parser::SyntaxKind::IDENT {
                continue;
            }
            // `config extensions.V.vlen_exp` names keys in a JSON file, not
            // definitions in the source, so there is nothing for navigation
            // to point at. Counting them as misses would understate the
            // feature; they are left out of the sample rather than silently
            // scored.
            if is_config_key_segment(&tok) {
                config_skipped += 1;
                continue;
            }
            if goto_asked >= IDENT_SAMPLE {
                break 'files;
            }
            let offset = u32::from(tok.text_range().start()) as usize;
            let pos = line_index.line_col(offset);
            // Asked of hover either way — a built-in type has a kind to
            // report even though it has no definition to jump to.
            if goto_has_no_target(&tok) {
                no_target_skipped += 1;
            } else {
                goto_asked += 1;
                if !nav_analysis
                    .goto_definition(url, pos)
                    .expect("no other thread is here to cancel it")
                    .is_empty()
                {
                    goto_hit += 1;
                } else if std::env::var_os("SAIL_DBG_GOTO").is_some() {
                    let parent =
                        tok.parent().map(|p| format!("{:?}", p.kind())).unwrap_or_default();
                    let chain: Vec<String> =
                        tok.parent_ancestors().take(4).map(|a| format!("{:?}", a.kind())).collect();
                    eprintln!("GOTOMISS {parent} {} [{}]", tok.text(), chain.join(">"));
                }
            }
            hover_asked += 1;
            // A hover that repeats the identifier and nothing else is not an
            // answer — that is what an unresolved name produces — so it only
            // counts when the markup carries something the source did not
            // already say.
            let name = tok.text();
            if analysis
                .hover(*fid, pos, &ide::hover::HoverConfig::default())
                .ok()
                .flatten()
                .is_some_and(|h| {
                    h.markup
                        .replace("```sail", "")
                        .replace("```", "")
                        .replace(name, "")
                        .trim()
                        .len()
                        > 1
                })
            {
                hover_hit += 1;
            } else if std::env::var_os("SAIL_DBG_HOVER").is_some() {
                let parent = tok.parent().map(|p| format!("{:?}", p.kind())).unwrap_or_default();
                let chain: Vec<String> =
                    tok.parent_ancestors().take(4).map(|a| format!("{:?}", a.kind())).collect();
                eprintln!("HOVERMISS {parent} {} [{}]", tok.text(), chain.join(">"));
            }
        }
    }
    let t_goto = t0.elapsed();
    let pct = |hit: usize, asked: usize| {
        if asked == 0 {
            0.0
        } else {
            100.0 * hit as f64 / asked as f64
        }
    };
    println!(
        "Goto-def:            {:>7} of {} identifiers resolve ({:.1}%)",
        goto_hit,
        goto_asked,
        pct(goto_hit, goto_asked)
    );
    println!(
        "Hover:               {:>7} of {} identifiers answer ({:.1}%)",
        hover_hit,
        hover_asked,
        pct(hover_hit, hover_asked)
    );
    println!(
        "  over {} identifiers in {} ms ({} config keys, {} built-ins skipped: \
         nothing to point at)",
        goto_asked,
        t_goto.as_millis(),
        config_skipped,
        no_target_skipped
    );
    report_metric("goto_def_resolve_pct", pct(goto_hit, goto_asked) as u64, "%");
    report_metric("hover_answer_pct", pct(hover_hit, hover_asked) as u64, "%");

    // 6. Workspace symbol index
    let t0 = Instant::now();
    let all_salsa = analysis.all_salsa_files();
    let pairs: Vec<_> = all_salsa
        .iter()
        .filter_map(|(url, _)| {
            let fid = analysis.file_id(url)?;
            Some((fid, analysis.files_ref().file_text(fid)?))
        })
        .collect();
    let index = ide_db::symbol_index::SymbolIndex::build(analysis.db(), pairs);
    let symbol_count = index.all_entries().count();
    let t_index = t0.elapsed();
    println!("Symbol index:        {:>7} ms ({} symbols)", t_index.as_millis(), symbol_count);
    report_metric("symbol_index", t_index.as_millis() as u64, "ms");
    report_metric("symbols", symbol_count as u64, "count");

    // 8. Run all IDE things (optional, `--run-all-ide-things` flag).
    if run_ide {
        run_ide_things(&analysis, &url_fids, &files);
    }

    // 9. Parallel prime_caches benchmark (fresh db)
    {
        let (host2, files2, _, _) = load_workspace(&root);
        let ncpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
        let cancel = hir::CancellationToken::new();

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
        let (host3, files3, _, _) = load_workspace(&root);
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
                let ids = hir::def_query::file_def_with_body_ids(analysis.db(), ft);
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
        let diags =
            analysis.file_diagnostics(*fid, &config).expect("no other thread is here to cancel it");
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
            if let Ok(hints) =
                analysis.inlay_hints(*fid, range, &ide::inlay_hints::InlayHintsConfig::default())
            {
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
        if let Ok(items) = analysis.completions(*fid, pos, &Default::default()) {
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
        if let Ok(lenses) = analysis.code_lenses(*fid, &Default::default()) {
            lens_count += lenses.len();
        }
    }
    let t_lenses = t0.elapsed();
    println!("  Code lenses:       {:>7} ms ({} annotations)", t_lenses.as_millis(), lens_count);

    // 6. Hover
    let t0 = Instant::now();
    for (_url, fid) in url_fids {
        let _ = analysis.hover(*fid, pos, &ide::hover::HoverConfig::default());
    }
    let t_hover = t0.elapsed();
    println!("  Hover:             {:>7} ms", t_hover.as_millis());

    let t_total = t_all.elapsed();
    println!("  -------");
    println!("  Total IDE:         {:>7} ms", t_total.as_millis());
    report_metric("ide_all", t_total.as_millis() as u64, "ms");
}

/// The `Expr` variant's name, for the untyped-expression breakdown.
fn expr_kind_name(expr: &hir::Expr) -> &'static str {
    use hir::Expr;
    match expr {
        Expr::Missing => "Missing",
        Expr::Literal(_) => "Literal",
        Expr::Ident(_) => "Ident",
        Expr::Call { .. } => "Call",
        Expr::Field { .. } => "Field",
        Expr::Index { .. } => "Index",
        Expr::BinaryOp { .. } => "BinaryOp",
        Expr::UnaryOp { .. } => "UnaryOp",
        Expr::If { .. } => "If",
        Expr::Match { .. } => "Match",
        Expr::Block { .. } => "Block",
        Expr::Let { .. } => "Let",
        Expr::Assign { .. } => "Assign",
        Expr::Tuple(_) => "Tuple",
        Expr::List(_) => "List",
        Expr::Struct { .. } => "Struct",
        Expr::Return(_) => "Return",
        Expr::Cast { .. } => "Cast",
        Expr::Error { .. } => "Error",
        Expr::TypeVar(_) => "TypeVar",
        Expr::Ref(_) => "Ref",
        Expr::Config(_) => "Config",
        Expr::SizeOf { .. } => "SizeOf",
        Expr::Constraint(_) => "Constraint",
        Expr::Throw(_) => "Throw",
        Expr::Exit(_) => "Exit",
        Expr::Attribute { .. } => "Attribute",
        Expr::Var { .. } => "Var",
        Expr::Try { .. } => "Try",
        Expr::While { .. } => "While",
        Expr::Repeat { .. } => "Repeat",
        Expr::Foreach { .. } => "Foreach",
        Expr::Array(_) => "Array",
        Expr::Update { .. } => "Update",
        Expr::Assert { .. } => "Assert",
        Expr::Subrange { .. } => "Subrange",
    }
}
