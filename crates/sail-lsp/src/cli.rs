//! CLI subcommands for developer-facing operations.
//!
//! analysis, debugging, and testing without starting the LSP server.
//!
//! # Available subcommands
//!
//! - `parse <file>` — parse a file and print the CST
//! - `symbols <file>` — print all symbols in a file
//! - `check <file>` — run type checking and print diagnostics
//! - `highlight <file>` — print semantic tokens as text
//! - `scip <dir>` — generate SCIP index (JSON) for code navigation

use std::path::Path;

/// Run the `parse` subcommand: parse a file and print its CST.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn cmd_parse(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let (root, errors) = syntax::parse_text(&text);
    println!("{:#?}", root);
    if !errors.is_empty() {
        eprintln!("\n--- {} parse error(s) ---", errors.len());
        for err in &errors {
            eprintln!("  offset {}: {}", err.offset, err.message);
        }
    }
    Ok(())
}

/// Run the `symbols` subcommand: print all top-level symbols.
#[allow(clippy::print_stdout)]
pub fn cmd_symbols(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let (root, _) = syntax::parse_text(&text);
    let tree = hir_def::item_tree::ItemTree::build_from_cst(&root);

    println!("# Symbols in {}", path.display());
    println!();
    for item in tree.top_level_items() {
        let name = item.name(&tree);
        let kind = item.item_kind(&tree);
        let span = item.span(&tree);
        println!("  {:?} {} [{}-{}]", kind, name, span.start, span.end);
    }
    println!("\n  Total: {} items", tree.top_level_items().len());
    Ok(())
}

/// Run the `check` subcommand: parse + build ItemTree + DefMap + report issues.
/// Note: full type inference requires workspace setup (not available in CLI mode).
/// This command checks parse errors and name resolution only.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn cmd_check(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;

    // Parse
    let (root, parse_errors) = syntax::parse_text(&text);

    // Build ItemTree + DefMap
    let tree = std::sync::Arc::new(hir_def::item_tree::ItemTree::build_from_cst(&root));
    let def_map = hir_def::nameres::DefMap::build(&tree);
    let def_diags = def_map.diagnostics();

    let total_issues = parse_errors.len() + def_diags.len();
    if total_issues == 0 {
        println!("No issues for {}", path.display());
    } else {
        println!("# Issues for {} ({} total)", path.display(), total_issues);
        for err in &parse_errors {
            eprintln!("  [parse] offset {}: {}", err.offset, err.message);
        }
        for diag in def_diags {
            eprintln!("  [nameres] {:?}", diag);
        }
    }
    Ok(())
}

/// Run the `highlight` subcommand: print semantic tokens.
#[allow(clippy::print_stdout)]
pub fn cmd_highlight(path: &Path) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
    let (root, _) = syntax::parse_text(&text);

    use parser::SyntaxKind as SK;
    for token in root.descendants_with_tokens().filter_map(|el| el.into_token()) {
        let kind = token.kind();
        let classification = match kind {
            SK::KW_FUNCTION
            | SK::KW_VAL
            | SK::KW_LET
            | SK::KW_VAR
            | SK::KW_MATCH
            | SK::KW_IF
            | SK::KW_ELSE
            | SK::KW_RETURN
            | SK::KW_FOREACH
            | SK::KW_WHILE
            | SK::KW_STRUCT
            | SK::KW_ENUM
            | SK::KW_UNION
            | SK::KW_REGISTER
            | SK::KW_MAPPING
            | SK::KW_TYPE
            | SK::KW_SCATTERED => "keyword",
            SK::NUM_LIT => "number",
            SK::STRING_LIT => "string",
            SK::IDENT => "identifier",
            SK::TY_VAR => "type_parameter",
            SK::LINE_COMMENT | SK::BLOCK_COMMENT | SK::DOC_COMMENT => "comment",
            _ => continue,
        };
        let range = token.text_range();
        println!(
            "  {}..{} {} {:?}",
            u32::from(range.start()),
            u32::from(range.end()),
            classification,
            token.text(),
        );
    }
    Ok(())
}

/// Run the `scip` subcommand: generate a SCIP-compatible JSON index.
/// Scans all `.sail` files in a directory, builds a `StaticIndex`,
/// and outputs a JSON document with definitions, references, and monikers.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn cmd_scip(dir: &Path) -> Result<(), String> {
    use ide::static_index::StaticIndex;
    use ide_db::FileDb;

    eprintln!("Generating SCIP index for {}...", dir.display());
    let mut sw = profile::StopWatch::start();

    // Discover and parse all .sail files
    let files_on_disk = crate::reload::scan_sail_files(dir);
    if files_on_disk.is_empty() {
        return Err(format!("No .sail files found in {}", dir.display()));
    }
    eprintln!("  Found {} files", files_on_disk.len());

    // Build TestFile-like objects for each file
    let mut file_data: Vec<(url::Url, ScipFile)> = Vec::new();
    for (path, text) in &files_on_disk {
        let url = url::Url::from_file_path(path).map_err(|_| "bad path")?;
        let scip_file = ScipFile::new(text);
        file_data.push((url, scip_file));
    }

    // Build file references for StaticIndex
    let all_files: Vec<(&url::Url, &dyn FileDb)> =
        file_data.iter().map(|(url, f)| (url, f as &dyn FileDb)).collect();

    // Derive project name from directory
    let project_name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("sail-project");

    let index = StaticIndex::compute(&all_files, project_name);
    eprintln!("  Indexed {} tokens across {} files", index.tokens.len(), index.files.len());

    // Output as JSON
    let mut documents = Vec::new();
    for indexed_file in &index.files {
        let mut occurrences = Vec::new();
        for (range, token_id) in &indexed_file.tokens {
            let data = index.tokens.get(*token_id);
            let symbol = data
                .and_then(|d| d.moniker.as_ref())
                .and_then(|m| match m {
                    ide::moniker::MonikerResult::Moniker(mono) => {
                        Some(mono.identifier.to_scip_symbol())
                    }
                    ide::moniker::MonikerResult::Local => None,
                })
                .unwrap_or_default();
            let is_def = data
                .and_then(|d| d.definition.as_ref())
                .is_some_and(|(url, def_range)| &indexed_file.url == url && *def_range == *range);
            occurrences.push(serde_json::json!({
                "range": [base_db::range_start(*range), base_db::range_end(*range)],
                "symbol": symbol,
                "is_definition": is_def,
            }));
        }
        documents.push(serde_json::json!({
            "relative_path": indexed_file.url.path(),
            "occurrences": occurrences,
        }));
    }

    // Symbol information
    let mut symbols = Vec::new();
    for (token_id, data) in index.tokens.iter() {
        let symbol = data.moniker.as_ref().and_then(|m| match m {
            ide::moniker::MonikerResult::Moniker(mono) => Some(mono.identifier.to_scip_symbol()),
            ide::moniker::MonikerResult::Local => None,
        });
        if let Some(sym) = symbol {
            symbols.push(serde_json::json!({
                "symbol": sym,
                "display_name": data.display_name,
                "kind": format!("{:?}", data.kind),
                "documentation": data.hover,
            }));
        }
        let _ = token_id; // used implicitly via iter
    }

    let scip_json = serde_json::json!({
        "metadata": {
            "version": 1,
            "tool_info": {
                "name": "sail-lsp",
                "version": env!("CARGO_PKG_VERSION"),
            },
            "project_root": dir.to_string_lossy(),
        },
        "documents": documents,
        "external_symbols": symbols,
    });

    let output = serde_json::to_string_pretty(&scip_json)
        .map_err(|e| format!("JSON serialization error: {e}"))?;
    println!("{output}");

    eprintln!("  Done in {}", sw.elapsed());
    Ok(())
}

/// Minimal file wrapper for CLI SCIP generation (no salsa database).
struct ScipFile {
    text: String,
    tokens: Vec<(parser::Token, parser::Span)>,
    parsed: Option<syntax::parser_lower::ParsedFile>,
    item_tree: Option<std::sync::Arc<hir_def::ItemTree>>,
    bodies: Option<std::sync::Arc<hir_def::bodies::CallableBodies>>,
    line_starts: Vec<usize>,
}

impl ScipFile {
    fn new(source: &str) -> Self {
        let tokens = parser::tokenize(source);
        let (cst_root, _) = syntax::parse_text(source);
        let parsed = Some(syntax::cst_lower::parsed_file_from_cst(&cst_root, source));
        let item_tree = Some(std::sync::Arc::new(hir_def::ItemTree::build_from_cst(&cst_root)));
        let bodies =
            Some(std::sync::Arc::new(hir_def::bodies::CallableBodies::from_cst(&cst_root)));
        let mut line_starts = vec![0usize];
        for (i, ch) in source.char_indices() {
            if ch == '\n' {
                line_starts.push(i + 1);
            }
        }
        Self { text: source.to_string(), tokens, parsed, item_tree, bodies, line_starts }
    }
}

impl hir_def::callgraph::WorkspaceFile for ScipFile {
    fn content_hash(&self) -> u64 {
        0
    }
    fn callgraph(&self) -> Option<&hir_def::callgraph::CallGraph> {
        None
    }
}

impl hir_def::callgraph::SourceFileInfo for ScipFile {
    fn text(&self) -> &str {
        &self.text
    }
    fn item_tree(&self) -> Option<&hir_def::ItemTree> {
        self.item_tree.as_deref()
    }
}

impl ide_db::FileDb for ScipFile {
    fn position_at(&self, offset: usize) -> ide_db::LineCol {
        let line = self.line_starts.partition_point(|&s| s <= offset).saturating_sub(1);
        let col = offset - self.line_starts[line];
        ide_db::LineCol { line: line as u32, col: col as u32 }
    }
    fn offset_at(&self, pos: &ide_db::LineCol) -> usize {
        let line = (pos.line as usize).min(self.line_starts.len() - 1);
        (self.line_starts[line] + pos.col as usize).min(self.text.len())
    }
    fn tokens(&self) -> Option<&[(parser::Token, parser::Span)]> {
        Some(&self.tokens)
    }
    fn token_at(&self, _pos: ide_db::LineCol) -> Option<&(parser::Token, parser::Span)> {
        None
    }
    fn parsed(&self) -> Option<&syntax::parser_lower::ParsedFile> {
        self.parsed.as_ref()
    }
    fn signature_index(
        &self,
    ) -> Option<&std::collections::HashMap<String, ide_db::CallableSignature>> {
        None
    }
    fn ref_counts(&self) -> &std::collections::HashMap<String, usize> {
        static EMPTY: std::sync::OnceLock<std::collections::HashMap<String, usize>> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(std::collections::HashMap::new)
    }
    fn impl_counts(&self) -> &std::collections::HashMap<String, usize> {
        static EMPTY: std::sync::OnceLock<std::collections::HashMap<String, usize>> =
            std::sync::OnceLock::new();
        EMPTY.get_or_init(std::collections::HashMap::new)
    }
    fn bodies(&self) -> Option<&hir_def::bodies::CallableBodies> {
        self.bodies.as_deref()
    }
}

/// Run the `analysis-stats` subcommand: load a workspace and report
/// parse/inference/diagnostic statistics.
///
/// Loads all `.sail` files via the salsa-backed Analysis pipeline,
/// runs type inference on every callable, and reports metrics in
/// `METRIC:name:value:unit` format.
#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn cmd_analysis_stats(dir: &Path, verbose: bool) -> Result<(), String> {
    use hir_def::callgraph::SourceFileInfo as _;
    use ide::analysis::{Analysis, AnalysisHost};
    use std::time::Instant;

    fn report_metric(name: &str, value: u64, unit: &str) {
        println!("METRIC:{name}:{value}:{unit}");
    }

    eprintln!("=======================================================");
    eprintln!("sail-lsp analysis-stats");
    eprintln!("=======================================================");

    // 1. Discover and load workspace.
    let sail_files = crate::reload::scan_sail_files(dir);
    if sail_files.is_empty() {
        return Err(format!("No .sail files found in {}", dir.display()));
    }

    let t0 = Instant::now();
    let mut host = AnalysisHost::new(None);
    let mut files = base_db::Files::default();
    let mut vfs = vfs::Vfs::default();
    let mut url_fids = Vec::new();

    for (path, text) in &sail_files {
        let url = url::Url::from_file_path(path).map_err(|_| "bad path")?;
        let fid = vfs.file_id_for_url(&url);
        files.set_file_contents(fid, text, base_db::Durability::HIGH);
    }
    files.apply_pending(host.raw_database_mut());

    for (path, _text) in &sail_files {
        if let Ok(url) = url::Url::from_file_path(path) {
            if let Some(fid) = vfs.lookup_file_id_by_url(&url) {
                url_fids.push((url, fid));
            }
        }
    }

    let t_load = t0.elapsed();
    let n_files = url_fids.len();
    eprintln!("\nWorkspace: {} files", n_files);
    eprintln!("Load + salsa inputs: {:>7} ms", t_load.as_millis());
    report_metric("workspace_load", t_load.as_millis() as u64, "ms");

    let analysis = Analysis::new(host.raw_database().clone(), files.clone());

    // 2. Parse all files.
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
    eprintln!("Total lines:         {:>7}", total_lines);
    eprintln!("Parse all:           {:>7} ms ({} errors)", t_parse.as_millis(), parse_error_count);
    report_metric("parse_all", t_parse.as_millis() as u64, "ms");
    report_metric("total_lines", total_lines as u64, "lines");

    // 3. ItemTree for all files.
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
    eprintln!("ItemTree all:        {:>7} ms ({} items)", t_items.as_millis(), item_count);
    report_metric("item_tree_all", t_items.as_millis() as u64, "ms");

    // 4. Type inference for all callables.
    let t0 = Instant::now();
    let mut callable_count = 0usize;
    let mut type_error_count = 0usize;
    for (_url, fid) in &url_fids {
        if let Some(ft) = files.file_text(*fid) {
            let ids = hir_def::def_query::file_def_with_body_ids(analysis.db(), ft);
            for &id in ids {
                callable_count += 1;
                let tcr = hir_ty::query::infer(analysis.db(), id);
                type_error_count += tcr.0.inference_diagnostics().len();
                type_error_count += tcr.0.type_mismatches.len();
            }
        }
    }
    let t_infer = t0.elapsed();
    eprintln!(
        "Infer all:           {:>7} ms ({} callables, {} type errors)",
        t_infer.as_millis(), callable_count, type_error_count
    );
    report_metric("infer_all", t_infer.as_millis() as u64, "ms");
    report_metric("callables", callable_count as u64, "count");

    // 5. Diagnostics for all files.
    let t0 = Instant::now();
    let config = ide_diagnostics::DiagnosticsConfig::new();
    let mut diag_count = 0usize;
    for (_url, fid) in &url_fids {
        let diags = analysis.file_diagnostics(*fid, &config);
        diag_count += diags.len();
    }
    let t_diag = t0.elapsed();
    eprintln!("Diagnostics all:     {:>7} ms ({} diagnostics)", t_diag.as_millis(), diag_count);
    report_metric("diagnostics_all", t_diag.as_millis() as u64, "ms");

    // 6. Summary.
    let t_total = t_load + t_parse + t_items + t_infer + t_diag;
    eprintln!("\n-------------------------------------------------------");
    eprintln!("Total analysis:      {:>7} ms", t_total.as_millis());
    report_metric("total_analysis", t_total.as_millis() as u64, "ms");

    if verbose {
        eprintln!("\n--- Per-file details ---");
        for (url, fid) in &url_fids {
            let path: &str = url.path();
            let short = path.rsplit('/').take(3).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
            if let Some(sf) = analysis.file_by_id(*fid) {
                let lines = sf.text().lines().count();
                let ft = files.file_text(*fid).unwrap();
                let ids = hir_def::def_query::file_def_with_body_ids(analysis.db(), ft);
                eprintln!("  {:<50} {:>5} lines  {:>3} callables", short, lines, ids.len());
            }
        }
    }

    eprintln!("\n=======================================================");
    Ok(())
}
