//! Analysis / AnalysisHost — RA's core pattern for request isolation.
//!
//! `AnalysisHost` is the mutable owner of the salsa database.
//! `Analysis` is an immutable snapshot that handlers use for queries.
//! Each request gets its own `Analysis` snapshot so mutations from
//! `did_change` don't affect in-flight queries.
//!
//! Created in .

use std::collections::HashMap;
use std::sync::Arc;

use base_db::{FileId, Files};
use hir::callgraph::SourceFileInfo;
use hir::DefKind;
use ide_db::root_database::{RootDatabase, SalsaFile};
use ide_db::FileDb;

mod hierarchy;
#[cfg(test)]
mod imports_tests;
mod ordered_names;
#[cfg(test)]
mod ordered_names_tests;

/// Snapshot of URL ↔ FileId mapping from the VFS layer.
///
/// Cheap to clone (Arc-wrapped). Used by Analysis for URL resolution
/// without depending on the mutable `vfs::Vfs` struct.
#[derive(Clone, Default)]
pub struct UrlMap {
    url_to_file_id: Arc<HashMap<url::Url, FileId>>,
    file_id_to_url: Arc<HashMap<FileId, url::Url>>,
}

impl UrlMap {
    /// Create from raw maps (called when taking a VFS snapshot).
    pub fn new(
        url_to_file_id: HashMap<url::Url, FileId>,
        file_id_to_url: HashMap<FileId, url::Url>,
    ) -> Self {
        Self { url_to_file_id: Arc::new(url_to_file_id), file_id_to_url: Arc::new(file_id_to_url) }
    }

    pub fn lookup_file_id(&self, url: &url::Url) -> Option<FileId> {
        self.url_to_file_id.get(url).copied()
    }

    pub fn lookup_url(&self, id: FileId) -> Option<&url::Url> {
        self.file_id_to_url.get(&id)
    }

    pub fn all_url_file_ids(&self) -> impl Iterator<Item = (&url::Url, FileId)> + '_ {
        self.url_to_file_id.iter().map(|(u, &id)| (u, id))
    }
}

/// Mutable owner of the salsa database.
///
/// Exactly.
/// The VFS layer (`Files`) lives on `Backend`, not here.
pub struct AnalysisHost {
    db: RootDatabase,
}

/// Read the model configuration named by `$option --config <path>`.
///
/// The path is written relative to the compiler's working directory, which an
/// editor does not share, so it is resolved by file name against the source
/// file's own directory. Values are flattened to dotted keys.
fn collect_config_values(
    db: &impl salsa::Database,
    ft: base_db::FileText,
    url: &url::Url,
    out: &mut std::collections::HashMap<String, String>,
) {
    let text = ft.text(db);
    let Some(arg) = text.lines().find_map(|line| {
        let line = line.trim();
        let rest = line.strip_prefix("$option")?;
        let idx = rest.find("--config").or_else(|| rest.find("-config"))?;
        rest[idx..].split_whitespace().nth(1)
    }) else {
        return;
    };
    let Ok(source) = url.to_file_path() else { return };
    let Some(dir) = source.parent() else { return };
    let name = arg.rsplit('/').next().unwrap_or(arg);
    let Ok(body) = std::fs::read_to_string(dir.join(name)) else { return };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) else { return };
    flatten_config(&json, String::new(), out);
}

fn flatten_config(
    value: &serde_json::Value,
    prefix: String,
    out: &mut std::collections::HashMap<String, String>,
) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() { k.clone() } else { format!("{prefix}.{k}") };
                flatten_config(v, key, out);
            }
        }
        serde_json::Value::Array(items) => {
            // A bitvector is written as an array of booleans; keep the whole
            // array so a check can see a non-bit element.
            for (i, item) in items.iter().enumerate() {
                flatten_config(item, format!("{prefix}[{i}]"), out);
            }
            out.insert(prefix, value.to_string());
        }
        other => {
            out.insert(prefix, other.to_string());
        }
    }
}

/// The last path segment of a file URL.
fn file_name_of(url: &url::Url) -> String {
    url.path().rsplit('/').next().unwrap_or_default().to_string()
}

/// The message of an `$include_error` this file reaches under `symbols`.
fn taken_include_error(
    db: &dyn salsa::Database,
    ft: base_db::FileText,
    symbols: &std::collections::HashSet<String>,
) -> Option<String> {
    let parsed = syntax::parse_query::semantic_parse_file(db, ft);
    let green = parsed.green.as_ref()?;
    let root = syntax::SyntaxNode::new_root(green.as_ref().clone());
    let mut symbols = symbols.clone();
    let mut cond = syntax::preprocess::CondFilter::new();
    for child in root.children() {
        let accepted = cond.accept(&child, &mut symbols, None);
        if accepted || child.kind() != parser::SyntaxKind::DIRECTIVE_DEF || !cond.taking() {
            continue;
        }
        let text = child.text().to_string();
        if let Some(msg) = text.trim().strip_prefix("$include_error") {
            return Some(msg.trim().to_string());
        }
    }
    None
}

impl AnalysisHost {
    /// Create a new AnalysisHost with optional LRU capacity hint.
    ///
    /// salsa 0.25 sets LRU per query at compile time via
    /// `#[salsa::tracked(lru = N)]`, so a runtime capacity cannot be
    /// honoured and this argument is ignored. rust-analyzer is in the same
    /// position: its `update_base_query_lru_capacities` is a function whose
    /// body is entirely commented out. Kept so the call sites read the same
    /// as RA'''s, and so the day salsa grows a runtime setter there is
    /// somewhere to put it.
    pub fn new(lru_capacity: Option<u16>) -> Self {
        let _ = lru_capacity; // TODO: wire to per-query LRU config if needed
        Self { db: RootDatabase::default() }
    }

    /// Raw mutable database access (for Files to write salsa inputs).
    pub fn raw_database_mut(&mut self) -> &mut RootDatabase {
        &mut self.db
    }

    /// Trigger cancellation of all in-flight salsa queries.
    ///
    /// Called from `process_changes()` before applying VFS updates so
    /// that any handler threads running stale queries will unwind via
    /// `salsa::Cancelled`.
    pub fn trigger_cancellation(&mut self) {
        use salsa::Database;
        self.db.trigger_cancellation();
    }

    /// Trigger garbage collection — evict stale LRU query results.
    ///
    /// (`ide/src/lib.rs:207-211`). Called during quiescence to
    /// prevent memory bloat on long-running sessions.
    pub fn trigger_garbage_collection(&mut self) {
        use salsa::Database;
        self.db.trigger_lru_eviction();
    }

    /// Raw read-only database access.
    pub fn raw_database(&self) -> &RootDatabase {
        &self.db
    }

    /// Take an immutable db snapshot.: cheap Arc clone.
    /// Callers combine this with a `Files` clone to create an `Analysis`.
    pub fn db_snapshot(&self) -> RootDatabase {
        self.db.clone()
    }

    /// Convenience: create an Analysis snapshot from this host + external VFS.
    /// Analogous to RA's `GlobalState::snapshot()` which combines
    /// `analysis_host.analysis()` + `vfs` clone.
    pub fn analysis(&self, files: &Files) -> Analysis {
        Analysis::new(self.db.clone(), files.clone())
    }

    /// Create an Analysis snapshot with URL mapping from VFS.
    pub fn analysis_with_url_map(&self, files: &Files, url_map: UrlMap) -> Analysis {
        Analysis::with_url_map(self.db.clone(), files.clone(), url_map)
    }

    /// Apply a batch of file changes to the database.
    ///
    /// All changes are applied in a single salsa revision bump. This is the
    /// one door, as it is in rust-analyzer: it took a `base_db::FileChange`
    /// and had no caller, while the server reached past it to
    /// `raw_database_mut().apply_change(..)` with a different change type and
    /// the CLI loaders wrote through a third path of their own.
    /// The workspace's `$include` graph, from what its files say.
    ///
    /// A method on the host because the server may not call a free function
    /// in `ide` — `the_server_goes_through_analysis` is the rule, and the
    /// graph has to exist *before* the `Analysis` it goes into, so it cannot
    /// be a method on that. The probes call [`build_include_graph`]
    /// directly; they are not the server.
    pub fn include_graph(
        &self,
        files: &[(url::Url, std::path::PathBuf)],
        url_to_fid: &std::collections::HashMap<url::Url, base_db::FileId>,
        lib_dir: Option<&std::path::Path>,
    ) -> hir::include_graph::IncludeGraph {
        build_include_graph(self.raw_database(), files, url_to_fid, lib_dir)
    }

    pub fn apply_change(&mut self, change: ide_db::apply_change::Change) -> bool {
        self.db.apply_change(change)
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new(None)
    }
}

/// The `$include` graph of a workspace, from what its files say.
///
/// In `ide` because both sides need the same one: the server built it in
/// its main loop and every corpus probe had none, so `Analysis` behaved
/// differently in the two — an `$include` cycle is reported only by a
/// holder of the whole graph, which made it a diagnostic no probe could
/// ever produce. Same shape as the symbol index, which was an override only
/// one probe set.
///
/// A quoted include is relative to the including file; an angle-bracket one
/// names a file in `lib_dir` — the library directory itself, not its parent,
/// because the server materialises it as `$SAIL_DIR/lib` and the probes
/// bundle it as `data/sail-lib`. When the resolved path is not
/// a file the workspace holds, the filename is matched by suffix — an editor
/// may have the file under a path the include does not spell.
pub fn build_include_graph(
    db: &ide_db::root_database::RootDatabase,
    files: &[(url::Url, std::path::PathBuf)],
    url_to_fid: &std::collections::HashMap<url::Url, base_db::FileId>,
    lib_dir: Option<&std::path::Path>,
) -> hir::include_graph::IncludeGraph {
    use std::collections::HashMap;
    let mut graph = hir::include_graph::IncludeGraph::new();
    if let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(db) {
        for &(from, to) in &hir::preprocess::workspace_plan(db, project).edges {
            graph.add_edge(from, to);
        }
        return graph;
    }
    // Build canonical path → FileId map once upfront.
    //
    // CrateGraph is built with pre-resolved file IDs
    // (reload.rs:741-804). No per-edge canonicalize() calls.
    //
    // Previously: O(edges × files) canonicalize() calls.
    // Now: O(files) canonicalize() calls + O(1) HashMap lookup per edge.
    let canonical_map: HashMap<std::path::PathBuf, base_db::FileId> = url_to_fid
        .iter()
        .filter_map(|(url, &fid)| {
            url.to_file_path().ok().and_then(|p| p.canonicalize().ok()).map(|c| (c, fid))
        })
        .collect();
    // Also build suffix map for fallback (when canonicalize fails)
    let suffix_map: HashMap<String, base_db::FileId> = url_to_fid
        .iter()
        .filter_map(|(url, &fid)| {
            let path = url.path();
            // Extract filename for suffix matching
            path.rsplit('/').next().map(|name| (name.to_string(), fid))
        })
        .collect();

    for (uri, path) in files {
        if let Some(&from_fid) = url_to_fid.get(uri) {
            // A file the loader could not read has no text: deleted
            // between the scan and the load, or not UTF-8. It used to be
            // impossible here — the scan read every file itself — and
            // the `unwrap` said so.
            let Some(text) = db.files().file_text(from_fid) else { continue };
            let include_paths = hir::def_query::typed_include_paths(db, text);
            let parent_dir = path.parent();

            for inc in include_paths {
                let resolved = match inc {
                    hir::def_query::IncludePath::Relative(ref p) => parent_dir.map(|d| d.join(p)),
                    hir::def_query::IncludePath::Library(ref p) => lib_dir.map(|d| d.join(p)),
                };

                // O(1) lookup via canonical_map instead of O(n) scan.
                if let Some(resolved_path) = resolved {
                    let found = resolved_path
                        .canonicalize()
                        .ok()
                        .and_then(|canon| canonical_map.get(&canon).copied());

                    if let Some(target_fid) = found {
                        graph.add_edge(from_fid, target_fid);
                    } else {
                        // Fallback: suffix matching for unresolved paths
                        let target_name = match inc {
                            hir::def_query::IncludePath::Relative(ref p)
                            | hir::def_query::IncludePath::Library(ref p) => p.clone(),
                        };
                        // Try suffix map first (O(1)), then full path suffix scan
                        let filename = target_name.rsplit('/').next().unwrap_or(&target_name);
                        let fallback = suffix_map.get(filename).copied().or_else(|| {
                            url_to_fid
                                .iter()
                                .find(|(u, _)| u.path().ends_with(&target_name))
                                .map(|(_, &fid)| fid)
                        });
                        if let Some(target_fid) = fallback {
                            graph.add_edge(from_fid, target_fid);
                        }
                    }
                }
            }
        }
    }

    graph
}

/// One code action, before it becomes a protocol message.
///
/// `for_diagnostic` indexes back into the diagnostics the caller passed,
/// so the handler can attach the one this fixes without matching on
/// ranges. `is_fix_all` marks the whole-document action, which the client
/// asks for under a different kind.
/// The counts `sail-lsp/analyzerStatus` reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct ServerStatus {
    pub files: usize,
    pub symbols: usize,
    pub include_nodes: usize,
}

#[derive(Debug, Clone)]
pub struct CodeAction {
    pub import: Option<ide_db::imports::import_assets::ImportRequest>,
    pub title: String,
    pub kind: ide_db::assists::AssistKind,
    /// What applying it does. A `SourceChange` rather than one file's
    /// edits, because an assist may reach another file — and because the
    /// handler already converts one of these for rename.
    pub source_change: ide_db::source_change::SourceChange,
    pub for_diagnostic: Option<usize>,
    pub is_preferred: Option<bool>,
    pub is_fix_all: bool,
}

/// Immutable snapshot of the world, analogous to RA's `GlobalStateSnapshot`.
///
/// Combines a salsa db snapshot (cheap Arc clone) with a `Files` snapshot
/// (FileText handles) and a `UrlMap` snapshot (URL↔FileId mapping from VFS).
/// Each handler gets its own `Analysis` so in-flight queries are not
/// affected by mutations.
///
/// In RA, `GlobalStateSnapshot` holds `analysis: Analysis` + `vfs`.
/// Here we flatten them into one struct for convenience since our
/// tower-lsp architecture doesn't have a separate snapshot layer.
/// + K2-3: Clone is cheap (salsa db clone = Arc bump, Files clone = Arc bump).
///   Enables passing to worker threads for parallel diagnostics, prime_caches.
#[derive(Clone)]
pub struct Analysis {
    db: RootDatabase,
    files: Files,
    /// URL ↔ FileId mapping snapshot (from vfs::Vfs).
    url_map: UrlMap,
    /// Selected semantic view for a position-based request. URLs remain physical.
    context: Option<(FileId, FileId)>,
    /// Include graph for scope-aware analysis.
    include_graph: Option<std::sync::Arc<hir::include_graph::IncludeGraph>>,
    // No `symbol_index` field. It was an override only `analysis_stats`
    // ever set, and two features read it *instead of* `symbol_index()` — so
    // `workspace/symbol` answered null and goto-definition could not leave a
    // file, while the probe that set it measured navigation at 99.7%. The
    // index is a salsa query; every database with files has it.
}

impl Analysis {
    /// Create from an AnalysisHost db snapshot + Files (VFS) snapshot.
    pub fn new(db: RootDatabase, files: Files) -> Self {
        Self { db, files, url_map: UrlMap::default(), context: None, include_graph: None }
    }

    /// Create with URL map (from VFS snapshot).
    /// Edits that keep `$include` paths correct when files move.
    pub fn will_rename_file_edits(
        &self,
        renames: &[(String, String)],
    ) -> crate::Cancellable<
        Option<std::collections::HashMap<url::Url, Vec<ide_db::text_edit::TextEdit>>>,
    > {
        self.with_db(|a| a.will_rename_file_edits_inner(renames))
    }

    fn will_rename_file_edits_inner(
        &self,
        renames: &[(String, String)],
    ) -> Option<std::collections::HashMap<url::Url, Vec<ide_db::text_edit::TextEdit>>> {
        let owned = self.all_salsa_files_vec();
        crate::navigation::will_rename_file_edits(owned.iter().map(|(u, s)| (u, s)), renames)
    }

    /// Types whose alias edges resolve to this declaration.
    pub fn type_subtypes(
        &self,
        data: &serde_json::Value,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::TypeHierarchyItem>> {
        self.with_db(|a| a.subtype_items(data))
    }

    /// Direct type aliases resolved in their declaration's environment.
    pub fn type_supertypes(
        &self,
        data: &serde_json::Value,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::TypeHierarchyItem>> {
        self.with_db(|a| a.supertype_items(data))
    }

    /// Callers of the declaration returned by prepare-call-hierarchy.
    pub fn incoming_calls(
        &self,
        data: &serde_json::Value,
    ) -> crate::Cancellable<Vec<(ide_db::ide_types::CallItem, Vec<base_db::TextRange>)>> {
        self.with_db(|a| a.incoming_call_items(data))
    }

    /// Callees in the selected declaration's physical source file.
    pub fn outgoing_calls(
        &self,
        data: &serde_json::Value,
    ) -> crate::Cancellable<Vec<(ide_db::ide_types::CallItem, Vec<base_db::TextRange>)>> {
        self.with_db(|a| a.outgoing_call_items(data))
    }

    /// Foldable ranges in a file.
    pub fn folding_ranges(
        &self,
        url: &url::Url,
    ) -> crate::Cancellable<Vec<crate::folding_ranges::Fold>> {
        self.with_db(|a| a.folding_ranges_inner(url))
    }

    fn folding_ranges_inner(&self, url: &url::Url) -> Vec<crate::folding_ranges::Fold> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        crate::folding_ranges::folding_ranges(&sf)
    }

    /// Per-callable effect annotations, for the code lens.
    pub fn effect_annotations(
        &self,
        url: &url::Url,
    ) -> crate::Cancellable<Vec<crate::effect_annotations::EffectAnnotation>> {
        self.with_db(|a| a.effect_annotations_inner(url))
    }

    fn effect_annotations_inner(
        &self,
        url: &url::Url,
    ) -> Vec<crate::effect_annotations::EffectAnnotation> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        crate::effect_annotations::effect_annotations(&sf)
    }

    /// Semantic tokens for one range.
    pub fn semantic_tokens_range(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
    ) -> crate::Cancellable<Option<crate::syntax_highlighting::HlRanges>> {
        self.with_db(|a| a.semantic_tokens_range_inner(url, range))
    }

    fn semantic_tokens_range_inner(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
    ) -> Option<crate::syntax_highlighting::HlRanges> {
        let id = self.file_id(url)?;
        Some(crate::syntax_highlighting::HlRanges::merge(
            base_db::preprocess::file_instances(&self.db, id).into_iter().filter_map(|view| {
                self.file_by_id(view).map(|sf| {
                    crate::syntax_highlighting::compute_semantic_tokens_range(&sf, &range)
                })
            }),
        ))
    }

    /// Fill in the parts of an inlay hint that are computed on demand.
    pub fn resolve_inlay_hint(
        &self,
        hint: &mut crate::inlay_hints::InlayHint,
    ) -> crate::Cancellable<()> {
        let hint = std::panic::AssertUnwindSafe(hint);
        self.with_db(move |a| {
            let hint = hint;
            a.resolve_inlay_hint_inner(hint.0)
        })
    }

    fn resolve_inlay_hint_inner(&self, hint: &mut crate::inlay_hints::InlayHint) {
        crate::inlay_hints::resolve_inlay_hint(hint);
    }

    /// Fill in the parts of a completion item that are computed on demand.
    pub fn resolve_completion_item(
        &self,
        item: &mut ide_completion::item::CompletionItem,
    ) -> crate::Cancellable<()> {
        let item = std::panic::AssertUnwindSafe(item);
        self.with_db(move |a| {
            let item = item;
            a.resolve_completion_item_inner(item.0)
        })
    }

    fn resolve_completion_item_inner(&self, item: &mut ide_completion::item::CompletionItem) {
        if let Some(request) = &item.import {
            item.additional_text_edits.clear();
            item.documentation = None;
            if let Some((candidate, edit)) =
                ide_db::imports::import_assets::resolve(&self.db, request)
            {
                item.additional_text_edits.push(edit);
                item.documentation = Some(format!(
                    "```sail\n{}\n```\n\n{}",
                    candidate.signature,
                    candidate.documentation.unwrap_or_default()
                ));
            }
            return;
        }

        let program = item
            .scope_file
            .and_then(|scope| self.file_text(scope))
            .and_then(|file| hir::source_order::program(&self.db, file));
        if program.is_some()
            || item.origin.is_some()
            || hir::source_order::declarations(&self.db).is_some()
        {
            if let Some(origin) = item.origin {
                item.documentation = None;
                let Some(program) = program else { return };
                let Some(file) = self.file_text(origin.file) else { return };
                let declaration =
                    hir::source_order::Declaration { file, index: origin.declaration };
                if !program.declarations(&self.db).contains(&declaration) {
                    return;
                }
                let Some(def) = hir::def_at_declaration_in_namespace(
                    &self.db,
                    declaration,
                    &item.label,
                    origin.namespace,
                ) else {
                    return;
                };
                let loc = def.location();
                let mut docs = Vec::new();
                if let Some(signature) = loc.lookup_signature(&self.db) {
                    docs.push(format!("```sail\n{signature}\n```"));
                }
                if let Some(comment) = loc.doc_text(&self.db) {
                    docs.push(comment);
                }
                item.documentation = (!docs.is_empty()).then(|| docs.join("\n\n"));
            } else {
                // Builtins may provide documentation without a declaration.
                // Locals and incomplete items must not resolve through an
                // unrelated global name after the completion request.
                crate::completion::resolve_completion_item_ide(item, &[]);
            }
            return;
        }
        let owned: Vec<_> = match item.scope_file {
            Some(from) => self
                .visible_salsa_files(from)
                .into_iter()
                .filter(|(_, sf)| {
                    sf.file_text().is_some_and(|(_, ft)| {
                        hir::visibility::symbol_visible_from(&self.db, from, ft, &item.label)
                    })
                })
                .collect(),
            None => self.all_salsa_files_vec(),
        };
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, s)| (u, s as &dyn FileDb)).collect();
        crate::completion::resolve_completion_item_ide(item, &all);
    }

    /// The title a code lens should show, from the lens's own data.
    pub fn code_lens_title(&self, data: &serde_json::Value) -> crate::Cancellable<Option<String>> {
        self.with_db(|a| a.code_lens_title_inner(data))
    }

    fn code_lens_title_inner(&self, data: &serde_json::Value) -> Option<String> {
        if data.get("contexts").is_none() || data["kind"] == "runnable" {
            return crate::annotations::code_lens_title(data);
        }
        let source = u32::try_from(data["source"].as_u64()?).ok()?;
        let source = self
            .url_map
            .all_url_file_ids()
            .find_map(|(_, id)| (id.index() == source).then_some(id))?;
        let offset = usize::try_from(data["offset"].as_u64()?).ok()?;
        let name = data["name"].as_str()?;
        let url = self.url_for_file_id(source)?;
        let active = base_db::preprocess::file_instances(&self.db, source);
        let mut sites = std::collections::HashSet::new();
        for view in active.into_iter().filter(|view| {
            data["contexts"].as_array().is_some_and(|contexts| {
                contexts.iter().any(|id| id.as_u64() == Some(u64::from(view.index())))
            })
        }) {
            let sf = self.file_by_id(view)?;
            // A retained lens must not silently resolve a different declaration
            // after an edit or configuration change.
            if !sf.item_tree().is_some_and(|tree| {
                tree.top_level_items().iter().any(|item| {
                    item.name_span(tree).start == offset && item.name(tree).as_str() == name
                })
            }) {
                continue;
            }
            let mut analysis = self.clone();
            analysis.context = Some((source, view));
            for result in
                analysis.find_all_refs_inner(url, sf.position_at(offset), None).unwrap_or_default()
            {
                if data["kind"] == "refs" {
                    for (id, ranges) in result.references {
                        let physical = base_db::preprocess::physical_file(&self.db, id);
                        sites.extend(ranges.into_iter().map(|(range, _)| (physical, range)));
                    }
                } else if data["kind"] == "impls" {
                    for declaration in result.declarations {
                        let Some(file) = self.file_by_id(declaration.file_id) else { continue };
                        let Some(tree) = file.item_tree() else { continue };
                        if tree.top_level_items().iter().any(|item| {
                            item.name_span(tree).start == base_db::range_start(declaration.range)
                                && matches!(
                                    item.def_kind(tree),
                                    DefKind::Function | DefKind::Mapping | DefKind::ScatteredClause
                                )
                        }) {
                            sites.insert((
                                base_db::preprocess::physical_file(&self.db, declaration.file_id),
                                declaration.range,
                            ));
                        }
                    }
                }
            }
        }
        let mut resolved = data.clone();
        resolved["count"] = serde_json::json!(sites.len());
        crate::annotations::code_lens_title(&resolved)
    }

    /// The `$include` graph rendered for the debugging command.
    pub fn render_include_graph(&self) -> crate::Cancellable<String> {
        self.with_db(|a| a.render_include_graph_inner())
    }

    fn render_include_graph_inner(&self) -> String {
        let Some(graph) = &self.include_graph else { return String::new() };
        crate::include_graph_view::render_include_graph(graph, &|fid| self.file_label(fid))
    }

    /// Every file a file pulls in, inlined.
    pub fn expand_includes(
        &self,
        url: &url::Url,
    ) -> crate::Cancellable<Option<crate::expand_include::ExpandIncludeResult>> {
        self.with_db(|a| a.expand_includes_inner(url))
    }

    fn expand_includes_inner(
        &self,
        url: &url::Url,
    ) -> Option<crate::expand_include::ExpandIncludeResult> {
        let graph = self.include_graph.as_ref()?;
        let file_id = self.file_id(url)?;
        Some(crate::expand_include::expand_includes(
            graph,
            file_id,
            &|fid| {
                self.url_for_file_id(fid).and_then(|u| self.file(u)).map(|sf| sf.text().to_string())
            },
            &|fid| self.file_label(fid),
        ))
    }

    /// A short, human-readable name for a file — its basename, or a
    /// placeholder when the id is not one the workspace knows.
    fn file_label(&self, fid: FileId) -> String {
        self.url_for_file_id(fid)
            .map(|u| u.path().rsplit('/').next().unwrap_or("?").to_string())
            .unwrap_or_else(|| format!("file:{}", fid.index()))
    }

    // `format` — the whole document, by URL — was `format_document` by
    // FileId, and the server calls the second. Removed rather than kept as
    // an alias: two doors is how one of them stops being wrapped.

    fn format_inner(
        &self,
        url: &url::Url,
        options: &ide_db::ide_types::FormatOptions,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let sf = self.file(url)?;
        crate::formatting::format_document_edits(&sf, options)
    }

    /// Format one range.
    pub fn range_format(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
        options: &ide_db::ide_types::FormatOptions,
    ) -> crate::Cancellable<Option<Vec<ide_db::text_edit::TextEdit>>> {
        self.with_db(|a| a.range_format_inner(url, range, options))
    }

    fn range_format_inner(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
        options: &ide_db::ide_types::FormatOptions,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let sf = self.file(url)?;
        crate::formatting::range_format_document_edits(&sf, range, options)
    }

    /// Edits to apply when the user presses Enter.
    pub fn on_enter(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<Vec<ide_db::text_edit::TextEdit>>> {
        self.with_db(|a| a.on_enter_inner(url, position))
    }

    fn on_enter_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let sf = self.file(url)?;
        crate::formatting::on_enter_edits(&sf, position)
    }

    /// The counterpart of the bracket at `offset`.
    pub fn matching_brace(
        &self,
        url: &url::Url,
        offset: usize,
    ) -> crate::Cancellable<Option<usize>> {
        self.with_db(|a| a.matching_brace_inner(url, offset))
    }

    fn matching_brace_inner(&self, url: &url::Url, offset: usize) -> Option<usize> {
        let sf = self.file(url)?;
        crate::matching_brace::matching_brace(&sf, offset)
    }

    /// Join the lines `range` spans into one.
    pub fn join_lines(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
    ) -> crate::Cancellable<Vec<ide_db::text_edit::TextEdit>> {
        self.with_db(|a| a.join_lines_inner(url, range).unwrap_or_default())
    }

    fn join_lines_inner(
        &self,
        url: &url::Url,
        range: base_db::TextRange,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let sf = self.file(url)?;
        crate::join_lines::join_lines(&sf, range)
    }

    /// Move the item at `offset` one place up or down among its siblings.
    pub fn move_item(
        &self,
        url: &url::Url,
        offset: usize,
        direction: crate::MoveDirection,
    ) -> crate::Cancellable<Vec<ide_db::text_edit::TextEdit>> {
        self.with_db(|a| a.move_item_inner(url, offset, direction).unwrap_or_default())
    }

    fn move_item_inner(
        &self,
        url: &url::Url,
        offset: usize,
        direction: crate::MoveDirection,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let sf = self.file(url)?;
        crate::move_item::move_item(&sf, offset, direction)
    }

    /// Every assist available at `range`.
    ///
    /// One door for all 64 registered handlers. The LSP handler used to keep
    /// its own table — fifteen of them, named and kinded inline — and called
    /// `ide_assists` free functions directly, so the other forty-nine were
    /// registered and unreachable. This is rust-analyzer's shape, where the
    /// server calls `analysis.assists(..)` and converts what comes back.
    pub fn assists(
        &self,
        url: &url::Url,
        range: ide_db::line_index::TextRange,
        config: &ide_assists::AssistConfig,
    ) -> crate::Cancellable<Vec<ide_db::assists::Assist>> {
        self.with_db(|a| a.assists_inner(url, range, config))
    }

    fn assists_inner(
        &self,
        url: &url::Url,
        range: ide_db::line_index::TextRange,
        config: &ide_assists::AssistConfig,
    ) -> Vec<ide_db::assists::Assist> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        ide_assists::assists(config, &sf, range)
    }

    /// Every code action available at `range`.
    ///
    /// Assists, the quick fixes for the diagnostics the client sent back,
    /// the `$include` an unresolved name needs, and format-fix-all — one
    /// answer, because the handler's job is to convert it, not to assemble
    /// it from four.
    pub fn code_actions(
        &self,
        url: &url::Url,
        range: ide_db::line_index::TextRange,
        diagnostics: &[ide_diagnostics::Diagnostic],
        format: &ide_db::ide_types::FormatOptions,
        assists: &ide_assists::AssistConfig,
    ) -> crate::Cancellable<Vec<CodeAction>> {
        self.with_db(|a| a.code_actions_inner(url, range, diagnostics, format, assists))
    }

    fn code_actions_inner(
        &self,
        url: &url::Url,
        range: ide_db::line_index::TextRange,
        diagnostics: &[ide_diagnostics::Diagnostic],
        format: &ide_db::ide_types::FormatOptions,
        assists: &ide_assists::AssistConfig,
    ) -> Vec<CodeAction> {
        let Some(file_id) = self.file_id(url) else { return Vec::new() };
        let one_file = |edits: Vec<ide_db::text_edit::TextEdit>| {
            let mut change = ide_db::source_change::SourceChange::new();
            change.insert_source_edit(file_id, edits);
            change
        };
        let mut out = Vec::new();
        for (i, diagnostic) in diagnostics.iter().enumerate() {
            if let Some((title, edit, is_preferred)) = self.quick_fix_inner(url, diagnostic) {
                out.push(CodeAction {
                    import: None,
                    title,
                    kind: ide_db::assists::AssistKind::QuickFix,
                    source_change: one_file(vec![edit]),
                    for_diagnostic: Some(i),
                    is_preferred: Some(is_preferred),
                    is_fix_all: false,
                });
            }
        }
        for assist in self.assists_inner(url, range, assists) {
            let Some(source_change) = assist.source_change else { continue };
            out.push(CodeAction {
                import: assist.import,
                title: assist.label.0,
                kind: assist.id.1,
                source_change,
                for_diagnostic: None,
                is_preferred: None,
                is_fix_all: false,
            });
        }
        if let Some(edits) = self.format_inner(url, format) {
            if !edits.is_empty() {
                out.push(CodeAction {
                    import: None,
                    title: "Format document".to_string(),
                    kind: ide_db::assists::AssistKind::Source,
                    source_change: one_file(edits),
                    for_diagnostic: None,
                    is_preferred: None,
                    is_fix_all: true,
                });
            }
        }
        out.retain(|a| !a.source_change.is_empty());
        out
    }

    pub fn resolve_auto_include(
        &self,
        request: &ide_db::imports::import_assets::ImportRequest,
    ) -> crate::Cancellable<Option<ide_db::source_change::SourceChange>> {
        self.with_db(|a| {
            ide_db::imports::import_assets::resolve(&a.db, request).map(|(_, edit)| {
                ide_db::source_change::SourceChange::from_file_edit(request.source, edit)
            })
        })
    }

    fn quick_fix_inner(
        &self,
        url: &url::Url,
        diagnostic: &ide_diagnostics::Diagnostic,
    ) -> Option<(String, ide_db::text_edit::TextEdit, bool)> {
        let sf = self.file(url)?;
        ide_assists::quick_fix_for_diagnostic(&sf, diagnostic)
    }

    /// Edits to apply when the user types `ch`, if it triggers formatting.
    ///
    /// Which characters trigger, and what they trigger, is knowledge about
    /// Sail — that `->` and `=>` are complete only once the second character
    /// arrives, so the trigger is the arrow head behind the cursor. It lived
    /// in the LSP handler, which also read the buffer to find that head.
    pub fn on_type_formatting(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        ch: &str,
        config: &ide_db::ide_types::FormatOptions,
    ) -> crate::Cancellable<Option<Vec<ide_db::text_edit::TextEdit>>> {
        self.with_db(|a| a.on_type_formatting_inner(url, position, ch, config))
    }

    fn on_type_formatting_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        ch: &str,
        config: &ide_db::ide_types::FormatOptions,
    ) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        match ch {
            "}" | ";" => self.format_inner(url, config),
            "\n" => self.on_enter_inner(url, position),
            "=" | ">" => {
                let sf = self.file(url)?;
                let offset = sf.offset_at(&position);
                let closes_an_arrow = offset >= 2
                    && matches!(sf.text().as_bytes().get(offset - 2), Some(b'-') | Some(b'='));
                closes_an_arrow.then(|| self.format_inner(url, config)).flatten()
            }
            _ => None,
        }
    }

    /// Ranges an editor should keep in sync while typing.
    pub fn linked_editing_ranges(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<ide_db::ide_types::LinkedEditingRanges>> {
        self.with_db(|a| a.at_position(url, position).linked_editing_ranges_inner(url, position))
    }

    fn linked_editing_ranges_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<ide_db::ide_types::LinkedEditingRanges> {
        let sf = self.file(url)?;
        crate::formatting::linked_editing_ranges_for_position(&sf, position)
    }

    /// The selection-range chain at `position`.
    pub fn selection_range(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<ide_db::ide_types::SelectionRange>> {
        self.with_db(|a| a.selection_range_inner(url, position))
    }

    fn selection_range_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<ide_db::ide_types::SelectionRange> {
        let sf = self.file(url)?;
        let offset = sf.offset_at(&position);
        crate::extend_selection::selection_range(&sf, offset)
    }

    /// `$include` links in a file.
    pub fn document_links(
        &self,
        url: &url::Url,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::DocumentLink>> {
        self.with_db(|a| a.document_links_inner(url))
    }

    fn document_links_inner(&self, url: &url::Url) -> Vec<ide_db::ide_types::DocumentLink> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        crate::formatting::document_links_for_file(url, &sf)
    }

    /// The lowered HIR at `position`, for the debugging command.
    pub fn view_hir(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<String>> {
        self.with_db(|a| a.at_position(url, position).view_hir_inner(url, position))
    }

    fn view_hir_inner(&self, url: &url::Url, position: ide_db::LineCol) -> Option<String> {
        let sf = self.file(url)?;
        let ft = self.file_text(self.file_id(url)?)?;
        Some(crate::view_hir::view_hir(self.db(), ft, &sf, position))
    }

    /// The item tree, for the debugging command.
    pub fn view_item_tree(&self, url: &url::Url) -> crate::Cancellable<Option<String>> {
        self.with_db(|a| a.view_item_tree_inner(url))
    }

    fn view_item_tree_inner(&self, url: &url::Url) -> Option<String> {
        let sf = self.file(url)?;
        Some(crate::view_item_tree::view_item_tree(&sf))
    }

    /// The syntax tree, for the debugging command.
    pub fn view_syntax_tree(&self, url: &url::Url) -> Option<String> {
        let sf = self.file(url)?;
        Some(crate::view_syntax_tree::view_syntax_tree(&sf))
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn document_symbols(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<crate::navigation_target::NavigationTarget>> {
        self.with_db(|a| a.document_symbols_inner(file_id))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn diagnostics(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_diagnostics::Diagnostic>> {
        self.with_db(|a| a.diagnostics_inner(file_id))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn signature_index(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<
        Option<std::sync::Arc<HashMap<String, ide_db::CallableSignature>>>,
    > {
        self.with_db(|a| a.signature_index_inner(file_id))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn hover(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
        config: &crate::hover::HoverConfig,
    ) -> ide_db::ide_types::Cancellable<Option<crate::hover::HoverResult>> {
        self.with_db(|a| {
            a.hover_inner(
                a.position_instances(file_id, position).first().copied().unwrap_or(file_id),
                position,
                config,
            )
        })?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn completions(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_completion::item::CompletionItem>> {
        self.with_db(|a| {
            a.completions_inner(
                a.position_instances(file_id, position).first().copied().unwrap_or(file_id),
                position,
            )
        })?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn inlay_hints(
        &self,
        file_id: FileId,
        range: ide_db::line_index::TextRange,
        config: &crate::inlay_hints::InlayHintsConfig,
    ) -> ide_db::ide_types::Cancellable<Vec<crate::inlay_hints::InlayHint>> {
        self.with_db(|a| a.inlay_hints_inner(file_id, range, config))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn code_lenses(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::Annotation>> {
        self.with_db(|a| a.code_lenses_inner(file_id))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn semantic_tokens(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<crate::syntax_highlighting::HlRanges> {
        self.with_db(|a| a.semantic_tokens_inner(file_id))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn format_document(
        &self,
        file_id: FileId,
        options: &ide_db::ide_types::FormatOptions,
    ) -> ide_db::ide_types::Cancellable<Option<Vec<ide_db::ide_types::IdeTextEdit>>> {
        self.with_db(|a| a.format_document_inner(file_id, options))?
    }

    /// See [`Analysis::with_db`]: a salsa cancellation reaches the caller
    /// as `Err` rather than as an unwind.
    pub fn signature_help(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Option<ide_db::ide_types::SignatureHelp>> {
        self.with_db(|a| {
            a.signature_help_inner(
                a.position_instances(file_id, position).first().copied().unwrap_or(file_id),
                position,
            )
        })?
    }

    /// Run a query, turning a salsa cancellation into an error value.
    ///
    /// rust-analyzer wraps every `Analysis` method this way
    /// (`Analysis::with_db`), which is what makes `Cancellable<T>` mean
    /// something: a caller that is not the LSP server — a test, a CLI, one of
    /// the corpus tools — gets a `Result` rather than an unwind it has to
    /// catch itself. Here the catching happened only at the server's dispatch
    /// boundary, so the methods that already returned `Cancellable` never
    /// actually produced `Err`, and every other caller had to wrap its own
    /// calls in `catch_unwind`.
    pub fn with_db<T>(
        &self,
        f: impl FnOnce(&Self) -> T + std::panic::UnwindSafe,
    ) -> ide_db::ide_types::Cancellable<T> {
        // `Cancelled::catch` is salsa's own: it re-raises anything that is
        // not a cancellation, so a real bug stays a panic rather than being
        // reported as a cancelled request.
        let _solver_budget =
            hir::SolverBudget::enter(hir::CancellationToken::never(), hir::SolverLimits::default());
        let this = std::panic::AssertUnwindSafe(self);
        salsa::Cancelled::catch(move || f(*this)).map_err(|_| ide_db::ide_types::CancelledError)
    }

    /// The call-hierarchy item for whatever is at `position`.
    pub fn prepare_call_hierarchy(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<ide_db::ide_types::CallItem>> {
        self.with_db(|a| a.at_position(url, position).prepare_call_item(url, position))
    }

    /// The type-hierarchy item for whatever is at `position`.
    pub fn prepare_type_hierarchy(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<ide_db::ide_types::TypeHierarchyItem>> {
        self.with_db(|a| a.at_position(url, position).prepare_type_item(url, position))
    }

    /// The symbol key at `position`, if the token there names something.
    fn symbol_key_at(&self, url: &url::Url, position: ide_db::LineCol) -> Option<String> {
        let sf = self.file(url)?;
        let (token, _) = sf.token_at(position)?;
        ide_db::token_symbol_key(token)
    }

    /// Declaration sites — the `val` rather than the `function`.
    pub fn goto_declaration(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::FileLocation>> {
        self.with_db(|a| a.at_position(url, position).goto_declaration_inner(url, position))
    }

    fn goto_declaration_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Vec<ide_db::ide_types::FileLocation> {
        if self.file(url).is_some_and(|sf| {
            sf.file_text()
                .is_some_and(|(_, ft)| hir::source_order::declarations_for(&self.db, ft).is_some())
        }) {
            let Some(sf) = self.file(url) else { return Vec::new() };
            return ide_db::defs::Definition::classify_at(&sf, position)
                .and_then(|d| self.definition_navigation(&sf, d.canonical(&self.db)))
                .map(|nav| ide_db::ide_types::FileLocation { url: nav.url, range: nav.focus_range })
                .into_iter()
                .collect();
        }
        let (Some(index), Some(key)) =
            (self.symbol_index_from(url), self.symbol_key_at(url, position))
        else {
            return Vec::new();
        };
        crate::navigation::declaration_locations_indexed(&index, &key, url, &|fid| {
            self.url_for_file_id(fid).cloned()
        })
    }

    /// Implementation sites — the clauses of a scattered definition.
    pub fn goto_implementation(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::FileLocation>> {
        self.with_db(|a| a.at_position(url, position).goto_implementation_inner(url, position))
    }

    fn goto_implementation_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Vec<ide_db::ide_types::FileLocation> {
        if self.file(url).is_some_and(|sf| {
            sf.file_text()
                .is_some_and(|(_, ft)| hir::source_order::declarations_for(&self.db, ft).is_some())
        }) {
            let Some(sf) = self.file(url) else { return Vec::new() };
            let Some(def) = ide_db::defs::Definition::classify_at(&sf, position) else {
                return Vec::new();
            };
            return self
                .definition_implementations(&def)
                .into_iter()
                .filter_map(|d| self.definition_navigation(&sf, d))
                .map(|nav| ide_db::ide_types::FileLocation { url: nav.url, range: nav.focus_range })
                .collect();
        }
        let (Some(index), Some(key)) =
            (self.symbol_index_from(url), self.symbol_key_at(url, position))
        else {
            return Vec::new();
        };
        crate::navigation::implementation_locations_indexed(&index, &key, url, &|fid| {
            self.url_for_file_id(fid).cloned()
        })
    }

    /// The definition of the *type* of whatever is at `position`.
    pub fn goto_type_definition(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::FileLocation>> {
        self.with_db(|a| a.at_position(url, position).goto_type_definition_inner(url, position))
    }

    fn goto_type_definition_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Vec<ide_db::ide_types::FileLocation> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        if self.file(url).is_some_and(|sf| {
            sf.file_text()
                .is_some_and(|(_, ft)| hir::source_order::declarations_for(&self.db, ft).is_some())
        }) {
            use hir::source_order::{
                self,
                bindings::{self, Namespace},
            };
            let selected = (|| {
                if let Some(def @ ide_db::defs::Definition::TypeDef(_)) =
                    ide_db::defs::Definition::classify_at(&sf, position)
                {
                    return Some(def);
                }
                let (_, ft) = sf.file_text()?;
                let offset = sf.offset_at(&position);
                let sema = hir::Semantics::new(&self.db);
                let ty = sema.type_of_expr(ft, offset).or_else(|| sema.type_of_pat(ft, offset))?;
                let name = ty.as_name()?;
                let at = source_order::at_offset(&self.db, ft, offset)?;
                let declaration =
                    bindings::resolve(&self.db, at, hir::Name::new(name), Namespace::Type)?;
                hir::def_at_declaration_in_namespace(&self.db, declaration, name, Namespace::Type)
                    .and_then(ide_db::defs::Definition::from_def)
            })();
            return selected
                .and_then(|def| self.definition_navigation(&sf, def))
                .map(|nav| ide_db::ide_types::FileLocation { url: nav.url, range: nav.focus_range })
                .into_iter()
                .collect();
        }
        let Some(key) = self.symbol_key_at(url, position) else { return Vec::new() };
        let bindings = crate::goto_type_definition::typed_bindings(&sf);
        let Some(ty_name) = bindings.get(&key) else { return Vec::new() };
        let owned = self.visible_salsa_files(self.file_id(url).unwrap());
        // A named type is tried first: `bits(32)` and `xlen` both appear as
        // annotations, and only the second names something to navigate to.
        if let Some(parsed) = crate::goto_type_definition::parse_named_type(ty_name) {
            let locs = crate::goto_type_definition::goto_type_definition(
                owned.iter().map(|(u, s)| (u, s)),
                url,
                &parsed,
            );
            if !locs.is_empty() {
                return locs;
            }
        }
        crate::goto_type_definition::goto_type_definition(
            owned.iter().map(|(u, s)| (u, s)),
            url,
            ty_name,
        )
    }

    /// The whole-workspace edit that renames whatever is at `position`.
    ///
    /// Validation, type-variable normalisation, reference search and the
    /// `SourceChange` were all assembled in the LSP handler, which made
    /// rename a feature only the server had.
    /// The name under the cursor and the range to highlight, if renaming is
    /// possible there.
    ///
    /// The handler used to walk the token stream itself and build the
    /// placeholder, which meant `ide::rename::prepare_rename` — the function
    /// written for exactly this — had no caller, and the handler's own copy
    /// prefixed a type variable with an apostrophe the lexer had already
    /// kept, so `'n` came back as `''n`.
    ///
    /// Returns line/column bounds rather than a byte range so the handler
    /// needs nothing else to answer.
    pub fn prepare_rename(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Option<(String, ide_db::LineCol, ide_db::LineCol)>> {
        self.with_db(|a| a.at_position(url, position).prepare_rename_inner(url, position))
    }

    fn prepare_rename_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Option<(String, ide_db::LineCol, ide_db::LineCol)> {
        let sf = self.file(url)?;
        let (name, range) = crate::rename::prepare_rename(&sf, position)?;
        Some((
            name,
            sf.position_at(base_db::range_start(range)),
            sf.position_at(base_db::range_end(range)),
        ))
    }

    pub fn rename(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        new_name: &str,
    ) -> crate::Cancellable<Result<Option<ide_db::source_change::SourceChange>, String>> {
        self.with_db(|a| a.at_position(url, position).rename_inner(url, position, new_name))
    }

    fn rename_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        new_name: &str,
    ) -> Result<Option<ide_db::source_change::SourceChange>, String> {
        let Some(sf) = self.file(url) else { return Ok(None) };
        let source = base_db::preprocess::physical_file(&self.db, self.file_id(url).unwrap());
        if self.position_instances(source, position).len() > 1 {
            return Err(
                "This position belongs to multiple include contexts; rename is ambiguous.".into()
            );
        }
        ide_db::rename::IdentifierKind::classify(new_name).map_err(|e| e.to_string())?;
        if crate::references::resolve_symbol_at(&sf, position).is_none() {
            return Ok(None);
        }
        let Some((token, _)) = sf.token_at(position) else { return Ok(None) };
        let effective = match crate::references::normalize_validated_rename(
            token,
            new_name,
            ide_db::SAIL_KEYWORDS,
        ) {
            Ok(Some(v)) => v,
            Ok(None) => new_name.to_string(),
            Err(e) => return Err(e.to_string()),
        };
        // Through `Definition::rename`, which is rust-analyzer's rename.
        // This used to assemble the edits here out of `find_all_refs`,
        // while `Definition::rename` sat unused in `ide-db` scanning raw
        // text for the old name.
        let Some(def) = ide_db::defs::Definition::classify_at(&sf, position) else {
            return Ok(None);
        };
        let scope = def
            .is_local()
            .then(|| self.file_id(url).map(ide_db::search::SearchScope::single_file))
            .flatten();
        let owned: Vec<(base_db::FileId, SalsaFile<'_>)> = self
            .all_salsa_files()
            .into_iter()
            .filter_map(|(_, sf)| Some((sf.file_text()?.1.file_id(&self.db), sf)))
            .collect();
        let files: Vec<(base_db::FileId, &dyn ide_db::FileDb)> =
            owned.iter().map(|(fid, sf)| (*fid, sf as &dyn ide_db::FileDb)).collect();
        let change = def
            .rename(&self.db, &files, scope, &effective)
            .map_err(|e: ide_db::rename::RenameError| e.to_string())?;
        let mut physical = ide_db::source_change::SourceChange::new();
        for (view, edits) in change.source_file_edits {
            let source = base_db::preprocess::physical_file(&self.db, view);
            for edit in edits {
                // A physical edit affects every inclusion of this token. Do
                // not silently rename a different binding in another module.
                if self.instances_at_offset(source, base_db::range_start(edit.range)).len() > 1 {
                    return Err("This source token is active in multiple include contexts; rename would also modify the other contexts.".into());
                }
                let list = physical.source_file_edits.entry(source).or_default();
                if !list.contains(&edit) {
                    list.push(edit);
                }
            }
        }
        if physical.is_empty() {
            Ok(None)
        } else {
            Ok(Some(physical))
        }
    }

    /// Ranges to highlight for whatever is at `position`.
    ///
    /// The handler used to pick the config, call the free function, and hold
    /// the file open to convert offsets afterwards. Only the last of those is
    /// conversion.
    pub fn highlight_related(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Vec<crate::highlight_related::HighlightedRange>> {
        self.with_db(|a| a.at_position(url, position).highlight_related_inner(url, position))
    }

    fn highlight_related_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Vec<crate::highlight_related::HighlightedRange> {
        let Some(sf) = self.file(url) else { return Vec::new() };
        let config = crate::highlight_related::HighlightRelatedConfig {
            references: true,
            exit_points: true,
            break_points: true,
            branch_exit_points: true,
        };
        crate::highlight_related::highlight_related(&sf, &config, position)
    }

    /// Find every reference to whatever is at `position`.
    ///
    /// The workspace file list is assembled here rather than by the caller.
    /// rust-analyzer's `Analysis::find_all_refs` takes a position and a
    /// scope and nothing else; here every caller had to build the
    /// `(FileId, &dyn FileDb)` pairs itself, which is both duplicated work
    /// and a way for two callers to disagree about what the workspace is.
    pub fn find_all_refs(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        scope: Option<ide_db::search::SearchScope>,
    ) -> crate::Cancellable<Option<Vec<crate::references::ReferenceSearchResult>>> {
        self.with_db(|a| a.at_position(url, position).find_all_refs_inner(url, position, scope))
    }

    fn find_all_refs_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
        scope: Option<ide_db::search::SearchScope>,
    ) -> Option<Vec<crate::references::ReferenceSearchResult>> {
        let sf = self.file(url)?;
        let target_file_id = self.file_id(url)?;
        let all = self.all_salsa_files();
        let pairs: Vec<(base_db::FileId, &dyn ide_db::FileDb)> = all
            .iter()
            .filter_map(|(_, f)| {
                Some((f.file_text()?.1.file_id(&self.db), f as &dyn ide_db::FileDb))
            })
            .collect();
        crate::references::find_all_refs(&pairs, target_file_id, &sf, position, scope)
    }

    /// What the server reports about itself.
    ///
    /// One method rather than three calls from the handler: the counts are
    /// one question, and `handlers_do_not_accumulate_logic` holds every
    /// handler to one `ide` feature.
    pub fn status(&self) -> crate::Cancellable<ServerStatus> {
        self.with_db(|a| ServerStatus {
            files: a.all_salsa_files().len(),
            symbols: a.symbol_index().map(|i| i.all_entries().count()).unwrap_or(0),
            include_nodes: a.include_graph().map(|g| g.all_files().len()).unwrap_or(0),
        })
    }

    /// The workspace's symbols, from the database.
    ///
    /// `GlobalState` used to hold this map and maintain it by hand off a
    /// dirty-file list, cloning the whole thing on every batch. It is a
    /// salsa query now, so an unchanged file's symbols are reused and the
    /// merge happens once per revision rather than once per batch.
    fn symbol_index(&self) -> Option<std::sync::Arc<ide_db::symbol_index::SymbolIndex>> {
        let ws = base_db::WorkspaceFiles::try_get(&self.db)?;
        Some(ide_db::db_query::workspace_symbol_index(&self.db, ws).0.clone())
    }

    fn symbol_index_from(
        &self,
        url: &url::Url,
    ) -> Option<std::sync::Arc<ide_db::symbol_index::SymbolIndex>> {
        let from = self.file_id(url)?;
        let ws = base_db::WorkspaceFiles::try_get(&self.db)?;
        if base_db::module_graph::file_scope(&self.db, from).is_none() {
            return self.symbol_index();
        }
        Some(ide_db::db_query::visible_symbol_index(&self.db, ws, from).0.clone())
    }

    fn visible_salsa_files(&self, from: FileId) -> Vec<(url::Url, SalsaFile<'_>)> {
        self.all_salsa_files()
            .into_iter()
            .filter(|(_, sf)| {
                sf.file_text().is_some_and(|(_, ft)| {
                    base_db::module_graph::can_reference_file(&self.db, from, ft.file_id(&self.db))
                })
            })
            .collect()
    }

    /// Goto-definition, whole.
    ///
    /// The three steps were spread across the LSP handler: local and
    /// same-file resolution here, field and method resolution there, an
    /// index lookup last. A caller that used only the first — as
    /// `analysis_stats` did — measured a third of the feature and reported
    /// it as the whole.
    pub fn goto_definition(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> crate::Cancellable<Vec<crate::navigation_target::NavigationTarget>> {
        self.with_db(|a| a.at_position(url, position).goto_definition_inner(url, position))
    }

    fn goto_definition_inner(
        &self,
        url: &url::Url,
        position: ide_db::LineCol,
    ) -> Vec<crate::navigation_target::NavigationTarget> {
        let Some(sf) = self.file(url) else { return Vec::new() };

        if sf
            .file_text()
            .is_some_and(|(_, ft)| hir::source_order::declarations_for(&self.db, ft).is_some())
        {
            let Some(class) = ide_db::defs::IdentClass::classify_at(&sf, position) else {
                return Vec::new();
            };
            let (def, is_reference) = match class {
                ide_db::defs::IdentClass::NameClass(c) => (c.defined(), false),
                ide_db::defs::IdentClass::NameRefClass(ide_db::defs::NameRefClass::Definition(
                    d,
                )) => (Some(d), true),
            };
            let Some(def) = def else { return Vec::new() };
            if is_reference {
                let implementations = self.definition_implementations(&def);
                if !implementations.is_empty() {
                    return implementations
                        .into_iter()
                        .filter_map(|d| self.definition_navigation(&sf, d))
                        .collect();
                }
            }
            return self.definition_navigation(&sf, def).into_iter().collect();
        }

        // 0. A pattern binding is its own definition. rust-analyzer answers
        //    `Name` nodes with themselves (`NameClass::classify`), and an
        //    editor expects goto on a binding site to stay put rather than
        //    report nothing.
        if let Some(file_id) = self.file_id(url) {
            if let Some(ft) = self.file_text(file_id) {
                let text = ft.text(self.db()).to_string();
                let offset = sf.offset_at(&position);
                if let Some((token, _)) = sf.token_at(position) {
                    if let Some(name) = ide_db::token_symbol_key(token) {
                        let (root, _) = syntax::parse_text(&text);
                        if let crate::goto_definition::IdentClass::Definition { span, .. } =
                            crate::goto_definition::classify_ident(&root, offset, &name)
                        {
                            return vec![crate::navigation_target::NavigationTarget {
                                file_id: None,
                                url: url.clone(),
                                name,
                                kind: ide_db::defs::SymbolKind::Variable,
                                full_range: span,
                                focus_range: span,
                                detail: None,
                                docs: None,
                                children: Vec::new(),
                            }];
                        }
                    }
                }
            }
        }

        // 1. Scope-aware local bindings and same-file definitions.
        if let Some(navs) = crate::goto_definition::goto_definition_semantic(&sf, position, url) {
            if !navs.is_empty() {
                return navs;
            }
        }

        // 2. Fields and methods, which need inference and the index.
        if let (Some(index), Some(file_id)) = (self.symbol_index_from(url), self.file_id(url)) {
            if let Some(file_text) = self.file_text(file_id) {
                let offset = sf.offset_at(&position);
                if let Some(navs) = crate::goto_definition::goto_definition_field_or_method(
                    self.db(),
                    file_text,
                    offset,
                    url,
                    &index,
                    &|fid| self.url_for_file_id(fid).cloned(),
                ) {
                    if !navs.is_empty() {
                        return navs;
                    }
                }
            }
        }

        // 3. Cross-file, by name, through the symbol index.
        //
        // Through `symbol_index`, not off the field: only `analysis_stats`
        // ever calls `with_symbol_index`, so in the server this step
        // returned empty and goto-definition stopped at the file it was
        // asked in. The probe set the field and measured 99.7%; the product
        // could not follow a name into another file at all.
        let Some(index) = self.symbol_index_from(url) else { return Vec::new() };
        let offset = sf.offset_at(&position);
        let Some((token, _)) = sf.token_at(position) else { return Vec::new() };
        let Some(key) = ide_db::token_symbol_key(token) else { return Vec::new() };
        let in_type_position = crate::goto_definition::names_a_type(sf.text(), offset);
        crate::navigation::definition_locations_indexed_in(
            &index,
            &key,
            url,
            &|fid| self.url_for_file_id(fid).cloned(),
            in_type_position,
        )
        .into_iter()
        .map(|loc| {
            crate::navigation_target::NavigationTarget::new(
                loc.url,
                key.clone(),
                ide_db::defs::SymbolKind::Function,
                loc.range,
                loc.range,
            )
        })
        .collect()
    }

    /// Answer a workspace-symbol query.
    ///
    /// The handler used to walk `symbol_index` itself and pick a
    /// `SymbolKind` on the way out, with a second `DefKind -> SymbolKind`
    /// table of its own. The two tables disagreed — a `union` was a struct
    /// here and an enum there — which is what a decision made in two
    /// places looks like from the outside.
    pub fn workspace_symbols(
        &self,
        query: &str,
    ) -> crate::Cancellable<Vec<crate::navigation_target::NavigationTarget>> {
        self.with_db(|a| a.workspace_symbols_inner(query))
    }

    fn workspace_symbols_inner(
        &self,
        query: &str,
    ) -> Vec<crate::navigation_target::NavigationTarget> {
        // Through `symbol_index`, which falls back to the salsa query. Read
        // straight off the field, this answered *nothing* in the server: only
        // `analysis_stats` ever calls `with_symbol_index`, so
        // `workspace/symbol` returned null for every query a user typed while
        // the index it needed was one method away and built.
        let Some(index) = self.symbol_index() else { return Vec::new() };
        index
            .search(&query.to_ascii_lowercase())
            .into_iter()
            .filter_map(|entry| {
                let range = base_db::text_range(entry.name_span.start, entry.name_span.end);
                let mut nav = crate::navigation_target::NavigationTarget::new(
                    self.url_for_file_id(entry.file_id).cloned()?,
                    entry.name.clone(),
                    crate::goto_definition::item_kind_to_symbol_kind(entry.kind),
                    base_db::text_range(entry.span.start, entry.span.end),
                    range,
                )
                .with_file_id(entry.file_id);
                if !entry.signature_text.is_empty() {
                    nav = nav.with_detail(entry.signature_text.clone());
                }
                if let Some(doc) = entry.doc.clone() {
                    nav = nav.with_docs(doc);
                }
                Some(nav)
            })
            .collect()
    }

    pub(crate) fn with_url_map(db: RootDatabase, files: Files, url_map: UrlMap) -> Self {
        Self { db, files, url_map, context: None, include_graph: None }
    }

    /// Create with include graph for scope-aware analysis.
    pub fn with_include_graph(
        db: RootDatabase,
        files: Files,
        url_map: UrlMap,
        graph: std::sync::Arc<hir::include_graph::IncludeGraph>,
    ) -> Self {
        Self { db, files, url_map, context: None, include_graph: Some(graph) }
    }

    /// Attach an include graph to an existing snapshot.
    pub fn with_include_graph_arc(
        mut self,
        graph: std::sync::Arc<hir::include_graph::IncludeGraph>,
    ) -> Self {
        self.include_graph = Some(graph);
        self
    }

    /// Get the include graph, if available.
    pub fn include_graph(&self) -> Option<&hir::include_graph::IncludeGraph> {
        self.include_graph.as_deref()
    }
}

impl Analysis {
    fn file_text(&self, id: FileId) -> Option<base_db::FileText> {
        self.db.files().file_text(id).or_else(|| self.files.file_text(id))
    }

    /// Find active views at a source position without joining their scopes.
    fn position_instances(&self, id: FileId, position: ide_db::LineCol) -> Vec<FileId> {
        let Some(file) = self.file_by_id(id) else { return vec![id] };
        let offset = file.offset_at(&position);
        self.instances_at_offset(id, offset)
    }

    fn instances_at_offset(&self, id: FileId, offset: usize) -> Vec<FileId> {
        let candidates = base_db::preprocess::file_instances(&self.db, id);
        if candidates.len() <= 1 {
            return candidates;
        }
        let Some(ft) = self.file_text(id) else { return vec![id] };
        let Some(root) = syntax::parse_query::parse_file(&self.db, ft).syntax_node() else {
            return vec![id];
        };
        let mut region = 0;
        for node in root.children() {
            if usize::from(node.text_range().end()) > offset {
                break;
            }
            if syntax::preprocess::Event::from_node(&node).is_some() {
                region += 1;
            }
        }
        candidates
            .into_iter()
            .filter(|&view| {
                base_db::preprocess::file_regions(&self.db, view)
                    .is_none_or(|regions| regions.get(region).copied().unwrap_or(false))
            })
            .collect()
    }

    fn at_position(&self, url: &url::Url, position: ide_db::LineCol) -> std::borrow::Cow<'_, Self> {
        if let Some(source) = self.file_id(url) {
            let view = self.position_instances(source, position).first().copied().unwrap_or(source);
            if view != source {
                let mut analysis = self.clone();
                analysis.context = Some((source, view));
                return std::borrow::Cow::Owned(analysis);
            }
        }
        std::borrow::Cow::Borrowed(self)
    }

    /// Get a SalsaFile by FileId.: Analysis methods take FileId.
    pub fn file_by_id(&self, id: FileId) -> Option<SalsaFile<'_>> {
        let ft = self.file_text(id)?;
        Some(SalsaFile::new(&self.db, ft))
    }

    /// Resolve URL to FileId. Called at handler layer (not inside Analysis methods).
    /// Analogous to RA's `GlobalStateSnapshot::file_id()`.
    pub fn file_id(&self, url: &url::Url) -> Option<FileId> {
        self.url_map
            .lookup_file_id(url)
            .map(|id| self.context.filter(|(source, _)| *source == id).map_or(id, |(_, view)| view))
    }

    /// Get the URL for a FileId.
    pub fn url_for_file_id(&self, id: FileId) -> Option<&url::Url> {
        self.url_map.lookup_url(base_db::preprocess::physical_file(&self.db, id))
    }

    /// Get the underlying salsa database (for advanced queries).
    /// Pre-warm the salsa caches, reporting progress as it goes.
    ///
    /// rust-analyzer's `Analysis::parallel_prime_caches`, and for the same
    /// reason: the main loop used to reach through `Analysis::db()` and
    /// `AnalysisHost::raw_database().files()` for this, which is the one
    /// rule this layer has — the server goes through `Analysis`. It also
    /// passed a literal `2` for the worker count and `&|_| {}` for the
    /// progress callback, so the phase that dominates startup reported
    /// nothing while it ran.
    pub fn parallel_prime_caches<F>(
        &self,
        num_worker_threads: usize,
        cancel: &hir::CancellationToken,
        cb: F,
    ) -> crate::Cancellable<()>
    where
        F: Fn(ide_db::prime_caches::ParallelPrimeCachesProgress) + Sync + std::panic::UnwindSafe,
    {
        self.with_db(move |analysis| {
            let files = analysis.files_ref().clone();
            ide_db::prime_caches::parallel_prime_caches(
                &analysis.db,
                &files,
                num_worker_threads,
                &cb,
                cancel,
            )
        })
    }

    pub fn db(&self) -> &RootDatabase {
        &self.db
    }

    /// Access the Files collection (for FileId → FileText lookups).
    pub fn files_ref(&self) -> &Files {
        &self.files
    }

    /// Get a SalsaFile for the given URL. Convenience wrapper.
    pub fn file(&self, url: &url::Url) -> Option<SalsaFile<'_>> {
        self.file_by_id(self.file_id(url)?)
    }

    /// Build a list of all files as `(Url, SalsaFile)` pairs.
    /// Callers can iterate with `.iter().map(|(u, s)| (u, s))` to get
    /// `(&Url, &SalsaFile)` for generic `F: FileDb` functions.
    pub fn all_salsa_files(&self) -> Vec<(url::Url, SalsaFile<'_>)> {
        // VFS membership describes known text, not the active build. Keep old
        // buffers available for file-local requests without exporting their
        // declarations after a project configuration switch.
        if let Some(workspace) = base_db::WorkspaceFiles::try_get(&self.db) {
            let mut seen = std::collections::HashSet::new();
            return workspace
                .file_texts(&self.db)
                .iter()
                .filter_map(|&ft| {
                    if !seen.insert(ft.file_id(&self.db)) {
                        return None;
                    }
                    let url = self.url_for_file_id(ft.file_id(&self.db))?;
                    Some((url.clone(), SalsaFile::new(&self.db, ft)))
                })
                .collect();
        }
        self.url_map
            .all_url_file_ids()
            .filter_map(|(url, id)| {
                let ft = self.file_text(id)?;
                Some((url.clone(), SalsaFile::new(&self.db, ft)))
            })
            .collect()
    }

    /// Build an owned vec of `(Url, SalsaFile)` pairs. Callers can then
    /// create `&dyn FileDb` slices from it.
    fn all_salsa_files_vec(&self) -> Vec<(url::Url, SalsaFile<'_>)> {
        self.all_salsa_files()
    }

    //
    // In RA, Analysis methods construct a Semantics internally and
    // delegate through it. Here, Analysis cannot directly construct
    // a Semantics because SalsaFile is created on the fly (no
    // persistent &dyn FileDb storage). Once VFS lands (J-stage),
    // files will be persistently stored and the bridge will work.
    //
    // For now, Analysis methods mirror what Semantics would do: the
    // same query logic, just without the extra indirection. New
    // semantic queries added to Semantics  are the canonical
    // entry points for code that has a WorkspaceDb (e.g. sync_main
    // handlers via TestWorkspace, or future VFS-backed state).

    /// Document symbols for a file.: takes FileId, not URL.
    fn document_symbols_inner(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<crate::navigation_target::NavigationTarget>> {
        let mut symbols = Vec::new();
        for view in base_db::preprocess::file_instances(&self.db, file_id) {
            if let Some(file) = self.file_by_id(view) {
                symbols.extend(crate::file_structure::file_structure(&file));
            }
        }
        symbols.sort_by_key(|symbol| symbol.full_range.start());
        symbols.dedup_by(|a, b| a.full_range == b.full_range && a.name == b.name);
        Ok(symbols)
    }

    /// Diagnostics for a file — salsa-backed, Cancellable wrapper.
    ///
    /// `ide_diagnostics::full_diagnostics()` (two-pipeline architecture).
    fn diagnostics_inner(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_diagnostics::Diagnostic>> {
        let config = ide_diagnostics::DiagnosticsConfig::new();
        Ok(self.file_diagnostics_inner(file_id, &config))
    }

    /// Signature index for a file.: takes FileId.
    fn signature_index_inner(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<
        Option<std::sync::Arc<HashMap<String, ide_db::CallableSignature>>>,
    > {
        let ft = match self.file_text(file_id) {
            Some(ft) => ft,
            None => return Ok(None),
        };
        Ok(Some(ide_db::db_query::signature_index(&self.db, ft).clone()))
    }

    /// Hover information for a symbol at position.
    fn hover_inner(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
        config: &crate::hover::HoverConfig,
    ) -> ide_db::ide_types::Cancellable<Option<crate::hover::HoverResult>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(None),
        };
        let url = match self.url_for_file_id(file_id) {
            Some(u) => u,
            None => return Ok(None),
        };
        let offset = sf.offset_at(&position);
        let (token, _) = match sf.token_at(position) {
            Some(t) => t,
            None => return Ok(None),
        };
        let symbol_key = match ide_db::token_symbol_key(token) {
            Some(k) => k,
            None => return Ok(None),
        };
        let range = base_db::text_range(offset, offset + symbol_key.len());
        let owned: Vec<_> = self
            .visible_salsa_files(file_id)
            .into_iter()
            .filter(|(_, target)| ide_db::symbol_visible_from(&sf, target, &symbol_key))
            .collect();
        let refs: Vec<(&url::Url, &SalsaFile<'_>)> = owned.iter().map(|(u, sf)| (u, sf)).collect();
        let Some(base) = crate::hover::hover_for_symbol(
            refs.iter().map(|(u, f)| (*u, *f)),
            url,
            &sf as &dyn FileDb,
            position,
            range,
            &symbol_key,
            config,
        ) else {
            return Ok(None);
        };
        // Field and method accesses carry information the structural hover
        // cannot see. The LSP handler used to add this afterwards, which
        // meant everyone else — tests, the coverage measurement — got the
        // hover without it.
        if let Some(ft) = self.file_text(file_id) {
            if let Some(extra) = crate::hover::hover_resolve_field_or_method(self.db(), ft, offset)
            {
                return Ok(Some(crate::hover::HoverResult {
                    markup: format!("{}\n\n{}", base.markup, extra),
                    range: base.range,
                    actions: base.actions,
                }));
            }
        }
        Ok(Some(base))
    }

    /// Completions at position.
    ///
    /// `ide_completion::completions()`.
    fn completions_inner(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_completion::item::CompletionItem>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(Vec::new()),
        };
        let url = match self.url_for_file_id(file_id) {
            Some(u) => u,
            None => return Ok(Vec::new()),
        };
        let offset = sf.offset_at(&position);
        let text = sf.text();
        let prefix = crate::completion::completion_prefix(text, offset);
        let file_text = self.file_text(file_id);
        // Ordered candidate filtering is declaration-based. A public overload
        // can be extended in an otherwise invisible module, so filtering its
        // physical file first would discard a globally visible group.
        let owned = if hir::source_order::declarations(&self.db).is_some() {
            self.all_salsa_files()
        } else {
            self.visible_salsa_files(file_id)
        };
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        Ok(crate::completion::completions(
            &self.db,
            file_text,
            &all,
            url,
            &sf as &dyn FileDb,
            text,
            offset,
            prefix,
            ide_db::SAIL_KEYWORDS,
            ide_db::SAIL_BUILTINS,
        ))
    }

    /// Inlay hints for a range.
    fn inlay_hints_inner(
        &self,
        file_id: FileId,
        range: ide_db::line_index::TextRange,
        config: &crate::inlay_hints::InlayHintsConfig,
    ) -> ide_db::ide_types::Cancellable<Vec<crate::inlay_hints::InlayHint>> {
        let mut hints = Vec::new();
        for view in base_db::preprocess::file_instances(&self.db, file_id) {
            hints.extend(self.instance_inlay_hints(view, range, config)?);
        }
        hints.sort_by_key(|hint| hint.offset);
        hints.dedup_by(|a, b| a.offset == b.offset && a.label == b.label && a.kind == b.kind);
        Ok(hints)
    }

    fn instance_inlay_hints(
        &self,
        file_id: FileId,
        range: ide_db::line_index::TextRange,
        config: &crate::inlay_hints::InlayHintsConfig,
    ) -> ide_db::ide_types::Cancellable<Vec<crate::inlay_hints::InlayHint>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(Vec::new()),
        };
        let url = match self.url_for_file_id(file_id) {
            Some(u) => u,
            None => return Ok(Vec::new()),
        };
        let owned = self.visible_salsa_files(file_id);
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();

        // Through `hir`, not `hir_ty`: `ide` talks to one semantic API.
        let sema = hir::Semantics::new(&self.db);
        let ft = self.file_text(file_id);
        let type_check = ft.map(|ft| sema.infer_file(ft));
        let transitive_effects = ft.map(|ft| sema.transitive_effects(ft));

        Ok(crate::inlay_hints::inlay_hints_ide_with_types(
            &all,
            url,
            &sf as &dyn FileDb,
            range,
            type_check.as_deref(),
            transitive_effects.as_deref(),
            config,
        ))
    }

    /// Code lenses for a file.
    fn code_lenses_inner(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::Annotation>> {
        let mut lenses: std::collections::BTreeMap<_, ide_db::ide_types::Annotation> =
            std::collections::BTreeMap::new();
        let empty = HashMap::new();
        for view in base_db::preprocess::file_instances(&self.db, file_id) {
            let Some(sf) = self.file_by_id(view) else { continue };
            for mut lens in crate::annotations::code_lenses_ide(&sf, &empty, &empty) {
                let Some(data) = lens.data.as_mut() else { continue };
                data["source"] =
                    serde_json::json!(base_db::preprocess::physical_file(&self.db, view).index());
                data["offset"] = serde_json::json!(base_db::range_start(lens.range));
                data["contexts"] = serde_json::json!([view.index()]);
                let key = (
                    (base_db::range_start(lens.range), base_db::range_end(lens.range)),
                    data["kind"].as_str().unwrap_or("").to_owned(),
                    data["name"].as_str().unwrap_or("").to_owned(),
                );
                if let Some(previous) = lenses.get_mut(&key) {
                    previous.data.as_mut().unwrap()["contexts"]
                        .as_array_mut()
                        .unwrap()
                        .push(serde_json::json!(view.index()));
                } else {
                    lenses.insert(key, lens);
                }
            }
        }
        Ok(lenses.into_values().collect())
    }

    /// Semantic tokens for a file.: takes FileId.
    fn semantic_tokens_inner(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<crate::syntax_highlighting::HlRanges> {
        Ok(crate::syntax_highlighting::HlRanges::merge(
            base_db::preprocess::file_instances(&self.db, file_id).into_iter().filter_map(|view| {
                self.file_by_id(view)
                    .map(|sf| crate::syntax_highlighting::compute_semantic_tokens(&sf))
            }),
        ))
    }

    /// Format a document.: takes FileId.
    fn format_document_inner(
        &self,
        file_id: FileId,
        options: &ide_db::ide_types::FormatOptions,
    ) -> ide_db::ide_types::Cancellable<Option<Vec<ide_db::ide_types::IdeTextEdit>>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(None),
        };
        // The client's options, not two of them. This took `tab_size` and
        // `insert_spaces` and rebuilt the rest from `Default`, so
        // `trimTrailingWhitespace`, `insertFinalNewline` and
        // `trimFinalNewlines` were parsed, cached for code actions, and
        // dropped on the way to the formatter — while *range* formatting,
        // in the handler beside it, passed all of them. "A feature's config
        // is a parameter, not a default" is a rule this file already
        // states.
        Ok(crate::formatting::format_document_edits(&sf, options))
    }

    /// Compute full diagnostics (parse + semantic + type) for a file.
    ///
    /// Single entry point: delegates entirely to `ide-diagnostics` two-pipeline
    /// architecture (`syntax_diagnostics` + `semantic_diagnostics`).
    ///
    /// Accepts `DiagnosticsConfig` for filtering (RA `lib.rs:224-242`).
    /// Builds `WorkspaceNames` from all workspace files to suppress
    /// cross-file false-positive diagnostics. In RA, this information
    /// comes implicitly from `CrateDefMap` (`hir-def/src/nameres.rs:172`).
    /// K2-2: Fast syntax-only diagnostics (parse errors + name resolution).
    ///
    /// No type inference — returns within milliseconds.
    pub fn syntax_diagnostics(
        &self,
        file_id: FileId,
        config: &ide_diagnostics::DiagnosticsConfig,
    ) -> crate::Cancellable<Vec<ide_diagnostics::Diagnostic>> {
        self.with_db(|a| a.syntax_diagnostics_inner(file_id, config))
    }

    fn syntax_diagnostics_inner(
        &self,
        file_id: FileId,
        config: &ide_diagnostics::DiagnosticsConfig,
    ) -> Vec<ide_diagnostics::Diagnostic> {
        let ft = match self.file_text(file_id) {
            Some(ft) => ft,
            None => return Vec::new(),
        };
        let sf = SalsaFile::new(&self.db, ft);
        ide_diagnostics::syntax_diagnostics(&self.db, config, &sf, ft)
    }

    /// Full diagnostics: syntax + semantic (type inference).
    ///
    /// Slower — involves type checking and constraint solving.
    pub fn file_diagnostics(
        &self,
        file_id: FileId,
        config: &ide_diagnostics::DiagnosticsConfig,
    ) -> crate::Cancellable<Vec<ide_diagnostics::Diagnostic>> {
        self.with_db(|a| a.file_diagnostics_inner(file_id, config))
    }

    fn file_diagnostics_inner(
        &self,
        file_id: FileId,
        config: &ide_diagnostics::DiagnosticsConfig,
    ) -> Vec<ide_diagnostics::Diagnostic> {
        let mut out = Vec::new();
        for view in base_db::preprocess::file_instances(&self.db, file_id) {
            for mut diagnostic in self.instance_diagnostics(view, config) {
                diagnostic.range.file_id =
                    base_db::preprocess::physical_file(&self.db, diagnostic.range.file_id);
                for (range, _) in &mut diagnostic.related {
                    range.file_id = base_db::preprocess::physical_file(&self.db, range.file_id);
                }
                if !out.iter().any(|d: &ide_diagnostics::Diagnostic| {
                    d.range == diagnostic.range
                        && d.message == diagnostic.message
                        && d.code == diagnostic.code
                }) {
                    out.push(diagnostic);
                }
            }
        }
        out
    }

    fn instance_diagnostics(
        &self,
        file_id: FileId,
        config: &ide_diagnostics::DiagnosticsConfig,
    ) -> Vec<ide_diagnostics::Diagnostic> {
        let ft = match self.file_text(file_id) {
            Some(ft) => ft,
            None => return Vec::new(),
        };
        let sf = SalsaFile::new(&self.db, ft);

        // Build workspace cross-file name sets from all files'
        // TopLevelEnv (salsa-cached, no recomputation cost).
        let ws_names = self.build_workspace_names_inner(Some(file_id));

        // Nothing downstream of this reads a diagnostic's fixes. A
        // published `textDocument/publishDiagnostics` carries a message, a
        // range, tags and related information — `to_proto::diagnostic`
        // sends no edit — and a quick fix reaches the client through
        // `textDocument/codeAction`, which recomputes it from the
        // diagnostics the client hands back. So every fix built here was
        // built to be dropped.
        //
        // `AssistResolveStrategy` is how rust-analyzer says that, and
        // `should_resolve` had no caller: the strategy was hard-coded to
        // `All` at the only place that chose one.
        let resolve = ide_db::assists::AssistResolveStrategy::None;
        ide_diagnostics::full_diagnostics(&self.db, config, &resolve, &sf, ft, Some(&ws_names))
    }

    /// Build cross-file name sets from all workspace files.
    ///
    /// Reads each file's `TopLevelEnv` via `top_level_env` (salsa-cached).
    /// Collects function, constructor, value, and record names across the
    /// workspace for cross-file false-positive suppression.
    ///
    /// Mirrors the information RA's `CrateDefMap` provides for cross-crate
    /// name resolution (`hir-def/src/nameres.rs:172`).
    pub fn build_workspace_names(&self) -> crate::Cancellable<ide_diagnostics::WorkspaceNames> {
        self.with_db(|a| a.build_workspace_names_inner(None))
    }

    fn build_workspace_names_inner(&self, from: Option<FileId>) -> ide_diagnostics::WorkspaceNames {
        let mut ws = ide_diagnostics::WorkspaceNames::default();
        // file name -> the file names it directly `$include`s, used to grow
        // the set of files responsible for an `$include_error`.
        let mut include_edges: Vec<(String, Vec<String>)> = Vec::new();
        // Every file's item tree, kept so the scattered groups can be closed
        // across files: `scattered function X` and `end X` need not share one.
        let mut trees: Vec<(FileId, std::sync::Arc<hir::ItemTree>)> = Vec::new();
        // Preprocessor symbols in effect for the workspace, so `$ifndef
        // _DEFAULT_ORDER_SET` resolves the same way it does when lowering.
        let mut base_symbols = syntax::preprocess::default_symbols();
        if let Some(wsf) = base_db::WorkspaceFiles::try_get(&self.db) {
            if let Some(order) = hir::Semantics::new(&self.db).workspace_default_order(wsf) {
                order.apply(&mut base_symbols);
            }
        }
        for (_url, _sf) in self.all_salsa_files() {
            let Some((_, view)) = _sf.file_text() else { continue };
            let fid = view.file_id(&self.db);
            if let Some(ft) = self.file_text(fid) {
                if let Some(tree) = hir::Semantics::new(&self.db).file_item_tree(ft) {
                    trees.push((fid, tree.clone()));
                }
                if from.is_some_and(|from| {
                    !base_db::module_graph::can_reference_file(&self.db, from, fid)
                }) {
                    continue;
                }
                let mut names = hir::Semantics::new(&self.db).top_level_names(ft);
                let visible = |name: &String| {
                    from.is_none_or(|from| {
                        hir::visibility::symbol_visible_from(&self.db, from, ft, name)
                    })
                };
                names.functions.retain(visible);
                names.constructors.retain(visible);
                names.values.retain(visible);
                names.records.retain(visible);
                ws.function_names.extend(names.functions);
                ws.constructor_names.extend(names.constructors);
                ws.value_names.extend(names.values);
                ws.record_names.extend(names.records);
                // Scattered heads: an `end X` may sit in a different file
                // from the `scattered function X` it closes.
                if ws.include_error.is_none() {
                    if let Some(msg) =
                        taken_include_error(&self.db as &dyn salsa::Database, ft, &base_symbols)
                    {
                        let name = file_name_of(&_url);
                        ws.include_error = Some((msg, std::collections::HashSet::from([name])));
                    }
                }
                collect_config_values(&self.db, ft, &_url, &mut ws.config_values);
                include_edges.push((
                    file_name_of(&_url),
                    hir::include_paths(&self.db, ft)
                        .iter()
                        .map(|p| p.rsplit('/').next().unwrap_or(p).to_string())
                        .collect(),
                ));
                if let Some(tree) = hir::Semantics::new(&self.db).file_item_tree(ft) {
                    let tree = &*tree;
                    for &id in tree.top_level_items() {
                        if !visible(&id.name(tree).to_string()) {
                            continue;
                        }
                        match id.def_kind(tree) {
                            DefKind::ValSpec => {
                                let name = id.name(tree).as_str().to_owned();
                                ws.value_spec_names.insert(name.clone());
                                if id.signature(tree).contains("<->") {
                                    ws.mapping_spec_names.insert(name);
                                }
                            }
                            DefKind::Mapping if id.signature(tree).contains("<->") => {
                                ws.mapping_spec_names.insert(id.name(tree).as_str().to_owned());
                            }
                            DefKind::ScatteredHead => {
                                ws.scattered_heads.insert(id.name(tree).as_str().to_owned());
                            }
                            DefKind::TypeAlias
                            | DefKind::Struct
                            | DefKind::Union
                            | DefKind::Enum
                            | DefKind::Bitfield
                            | DefKind::Newtype => {
                                ws.type_names.insert(id.name(tree).as_str().to_owned());
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        // Scattered completeness, over every file at once.
        let status =
            hir::scattered::workspace_scattered_status(trees.iter().map(|(f, t)| (*f, &**t)));
        ws.scattered_problems = hir::scattered::check_scattered_completeness(&status);

        // An `$include` cycle. Only a holder of the whole graph can see one;
        // `semantic_diagnostics` reports it on each file that is on it.
        // Physical dependency edges can form a cycle across two disjoint
        // include contexts. Only the ordered walk can prove a real recursion.
        let cycle =
            if let Some(input) = base_db::preprocess::WorkspaceSourceProject::try_get(&self.db) {
                let cycles = &hir::preprocess::workspace_plan(&self.db, input).cycles;
                (!cycles.is_empty()).then(|| cycles.clone())
            } else {
                self.include_graph().and_then(|graph| graph.find_cycle())
            };
        if let Some(cycle) = cycle {
            let names: std::collections::HashMap<FileId, String> = self
                .url_map
                .all_url_file_ids()
                .map(|(url, fid)| (fid, file_name_of(url)))
                .collect();
            let rendered: Vec<String> = cycle
                .iter()
                .map(|f| names.get(f).cloned().unwrap_or_else(|| format!("{f:?}")))
                .collect();
            let desc = if rendered.len() <= 5 {
                rendered.join(" → ")
            } else {
                format!("{} → ... ({} files in cycle)", rendered[0], rendered.len())
            };
            ws.include_cycle = Some((cycle.into_iter().collect(), desc));
        }

        // A file that includes a responsible file is itself responsible.
        if let Some((_, ref mut responsible)) = ws.include_error {
            let mut changed = true;
            while changed {
                changed = false;
                for (file, includes) in &include_edges {
                    if responsible.contains(file) {
                        continue;
                    }
                    if includes.iter().any(|i| responsible.contains(i)) {
                        responsible.insert(file.clone());
                        changed = true;
                    }
                }
            }
        }
        ws
    }

    /// Get inferred type text by querying each per-callable InferenceResult.
    /// Query per-item, not per-file merged result.
    pub fn expr_type_text(
        &self,
        file_id: FileId,
        span: parser::Span,
    ) -> crate::Cancellable<Option<String>> {
        self.with_db(|a| {
            a.expr_type_text_inner(
                a.instances_at_offset(file_id, span.start).first().copied().unwrap_or(file_id),
                span,
            )
        })
    }

    fn expr_type_text_inner(&self, file_id: FileId, span: parser::Span) -> Option<String> {
        let ft = self.file_text(file_id)?;
        let sf = SalsaFile::new(&self.db, ft);
        hir::Semantics::new(&self.db).expr_type_text(ft, span, sf.bodies())
    }

    /// Get inferred type text for a binding by querying per-callable.
    pub fn binding_type_text(
        &self,
        file_id: FileId,
        span: parser::Span,
    ) -> crate::Cancellable<Option<String>> {
        self.with_db(|a| {
            a.binding_type_text_inner(
                a.instances_at_offset(file_id, span.start).first().copied().unwrap_or(file_id),
                span,
            )
        })
    }

    fn binding_type_text_inner(&self, file_id: FileId, span: parser::Span) -> Option<String> {
        let ft = self.file_text(file_id)?;
        let sf = SalsaFile::new(&self.db, ft);
        hir::Semantics::new(&self.db).binding_type_text(ft, span, sf.bodies())
    }

    /// Workspace-level reference counts — computed on demand.
    pub fn ref_counts(&self) -> crate::Cancellable<HashMap<String, usize>> {
        self.with_db(|a| a.ref_counts_inner())
    }

    fn ref_counts_inner(&self) -> HashMap<String, usize> {
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        crate::annotations::collect_reference_counts(&all)
    }

    /// Workspace-level implementation counts — computed on demand.
    pub fn impl_counts(&self) -> crate::Cancellable<HashMap<String, usize>> {
        self.with_db(|a| a.impl_counts_inner())
    }

    fn impl_counts_inner(&self) -> HashMap<String, usize> {
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        crate::annotations::collect_implementation_counts(&all)
    }

    // Handlers call `analysis.method()` which internally
    // constructs the file adapter. This generic method enables
    // handlers to pass a closure that operates on &dyn FileDb.

    /// Signature help at position.
    fn signature_help_inner(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Option<ide_db::ide_types::SignatureHelp>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(None),
        };
        let url = match self.url_for_file_id(file_id) {
            Some(u) => u,
            None => return Ok(None),
        };
        let owned = self.visible_salsa_files(file_id);
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        Ok(crate::signature_help::signature_help(&all, url, &sf as &dyn FileDb, position))
    }
}

/// Pre-materialized workspace of SalsaFiles for WorkspaceDb compatibility.
/// Created by Analysis::workspace(). Stores all files so their references
/// remain valid for WorkspaceDb::get_file() and all_files().
pub struct SalsaWorkspace<'a> {
    files: Vec<(url::Url, SalsaFile<'a>)>,
}

impl<'a> ide_db::WorkspaceDb for SalsaWorkspace<'a> {
    fn get_file(&self, uri: &url::Url) -> Option<&dyn FileDb> {
        self.files.iter().find(|(u, _)| u == uri).map(|(_, sf)| sf as &dyn FileDb)
    }

    fn all_files(&self) -> Vec<(&url::Url, &dyn FileDb)> {
        self.files.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect()
    }
}

// D8: inference_diag_to_ide removed — replaced by
// ide_diagnostics::handlers::dispatch() (RA handler-per-diagnostic pattern).
// hir_diag_to_ide moved to ide-diagnostics::hir_diag_to_ide (RA two-pipeline).

//
// These wrap the existing query methods with salsa cancellation support.
// New code should prefer these over direct calls.

// `file_structure` was `document_symbols` a second time — same body, same
// call — with the server calling one of them. One feature, one door; the
// outline is built in `ide::file_structure`, where rust-analyzer builds
// it, and `Analysis::document_symbols` is the only way in.

#[cfg(test)]
mod tests {
    use super::*;

    /// Test helper: allocate FileId via VFS and set file text, through the
    /// one door.
    fn set_text(
        host: &mut AnalysisHost,
        files: &mut Files,
        vfs: &mut vfs::Vfs,
        url: &url::Url,
        text: &str,
    ) {
        let fid = vfs.file_id_for_url(url);
        let mut change = ide_db::apply_change::Change::new();
        change.change_file(fid, text);
        host.apply_change(change);
        *files = host.raw_database().files().clone();
    }

    /// Build a UrlMap from a Vfs snapshot (test helper).
    fn url_map(vfs: &vfs::Vfs) -> UrlMap {
        let (a, b) = vfs.url_map_snapshot();
        UrlMap::new(a, b)
    }

    #[test]
    fn module_privacy_members_and_scattered_rename_keep_definition_origins() {
        use base_db::module_graph::{ModuleData, ModuleGraph};
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let sources = [
            "private val secret : unit -> int\nenum color = { Red }\nscattered function execute : int -> int\n",
            "function secret() = 1\nfunction inside() = secret()\nfunction probe() = sec\n",
            "function outside() = secret()\nfunction color_use() = Red\nfunction clause execute(x) = x\nend execute\nfunction probe2() = sec\n",
            "function unrelated() = Red\n",
        ];
        let urls: Vec<_> =
            (0..4).map(|i| url::Url::parse(&format!("file:///scope{i}.sail")).unwrap()).collect();
        for (url, source) in urls.iter().zip(sources) {
            set_text(&mut host, &mut files, &mut vfs, url, source);
        }
        let ids: Vec<_> = urls.iter().map(|u| vfs.file_id_for_url(u)).collect();
        let mut change = ide_db::apply_change::Change::new();
        change.set_workspace_files(ids.clone());
        change.set_module_graph(ModuleGraph {
            modules: vec![
                ModuleData { files: vec![ids[0], ids[1]], requires: vec![] },
                ModuleData { files: vec![ids[2]], requires: vec![0] },
                ModuleData { files: vec![ids[3]], requires: vec![] },
            ],
            globals: vec![],
        });
        host.apply_change(change);
        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        let position = |i: usize, name: &str| {
            analysis.file(&urls[i]).unwrap().position_at(sources[i].find(name).unwrap())
        };
        assert!(!analysis.goto_definition(&urls[1], position(1, "secret")).unwrap().is_empty());
        assert!(analysis.goto_definition(&urls[2], position(2, "secret")).unwrap().is_empty());
        for (i, line, col, visible) in [(1, 2, 22, true), (2, 4, 23, false)] {
            let items = analysis.completions(ids[i], ide_db::LineCol { line, col }).unwrap();
            assert_eq!(items.iter().any(|item| item.label == "secret"), visible);
        }
        let renamed =
            analysis.rename(&urls[2], position(2, "Red"), "Blue").unwrap().unwrap().unwrap();
        assert!(renamed.source_file_edits.contains_key(&ids[0]));
        assert!(renamed.source_file_edits.contains_key(&ids[2]));
        assert!(!renamed.source_file_edits.contains_key(&ids[3]));
        let renamed =
            analysis.rename(&urls[2], position(2, "execute"), "run").unwrap().unwrap().unwrap();
        assert!(renamed.source_file_edits.contains_key(&ids[0]), "head is in a required module");
        assert!(renamed.source_file_edits.contains_key(&ids[2]));
    }

    #[test]
    fn module_visibility_is_shared_by_inference_navigation_completion_and_rename() {
        use base_db::module_graph::{ModuleData, ModuleGraph};
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let urls: Vec<_> = ["leaf", "bridge", "main", "unrelated"]
            .iter()
            .map(|n| url::Url::parse(&format!("file:///{n}.sail")).unwrap())
            .collect();
        let sources = [
            "val leaf : unit -> int\nfunction leaf() = 1\n",
            "val bridge : unit -> int\nfunction bridge() = leaf()\n",
            "val main : unit -> int\nfunction main() = leaf()\nfunction probe() = le\n",
            "val other : unit -> int\nfunction other() = leaf()\n",
        ];
        for (url, source) in urls.iter().zip(sources) {
            set_text(&mut host, &mut files, &mut vfs, url, source);
        }
        let ids: Vec<_> = urls.iter().map(|u| vfs.file_id_for_url(u)).collect();
        let mut offered = None;
        for direct in [false, true, false] {
            let mut change = ide_db::apply_change::Change::new();
            change.set_workspace_files(ids.clone());
            change.set_module_graph(ModuleGraph {
                modules: vec![
                    ModuleData { files: vec![ids[0]], requires: vec![] },
                    ModuleData { files: vec![ids[1]], requires: vec![0] },
                    ModuleData {
                        files: vec![ids[2]],
                        requires: if direct { vec![1, 0] } else { vec![1] },
                    },
                    ModuleData { files: vec![ids[3]], requires: vec![] },
                ],
                globals: vec![],
            });
            host.apply_change(change);
            let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
            let pos = ide_db::LineCol { line: 1, col: 19 };
            let nav = analysis.goto_definition(&urls[2], pos).unwrap();
            assert_eq!(!nav.is_empty(), direct, "{nav:?}");
            assert!(nav.iter().all(|n| n.url == urls[0]));
            let items = analysis.completions(ids[2], ide_db::LineCol { line: 2, col: 21 }).unwrap();
            assert_eq!(items.iter().any(|i| i.label == "leaf"), direct);
            assert!(items.iter().all(|item| item.scope_file == Some(ids[2])));
            // Resolving a previously offered item must use the current graph
            // and the requesting file, not the global declaration index.
            if let Some(item) = items.iter().find(|item| item.label == "leaf") {
                offered = Some(item.clone());
            }
            if let Some(mut item) = offered.clone() {
                item.detail = None;
                item.documentation = None;
                analysis.resolve_completion_item(&mut item).unwrap();
                assert_eq!(item.documentation.unwrap().contains("unit -> int"), direct);
            }
            let findings = format!(
                "{:?}",
                analysis
                    .file_diagnostics(ids[2], &ide_diagnostics::DiagnosticsConfig::new())
                    .unwrap()
            );
            assert_eq!(findings.contains("leaf"), !direct, "{findings}");
            let refs = analysis
                .find_all_refs(&urls[0], ide_db::LineCol { line: 0, col: 5 }, None)
                .unwrap()
                .unwrap();
            assert!(refs.iter().any(|r| r.references.contains_key(&ids[1])));
            assert_eq!(refs.iter().any(|r| r.references.contains_key(&ids[2])), direct);
            assert!(refs.iter().all(|r| !r.references.contains_key(&ids[3])));
            let change = analysis
                .rename(&urls[0], ide_db::LineCol { line: 0, col: 5 }, "renamed")
                .unwrap()
                .unwrap()
                .unwrap();
            assert!(change.source_file_edits.contains_key(&ids[1]));
            assert_eq!(change.source_file_edits.contains_key(&ids[2]), direct);
            assert!(!change.source_file_edits.contains_key(&ids[3]));
            assert_eq!(
                analysis.rename(&urls[2], pos, "renamed").unwrap().unwrap().is_some(),
                direct
            );
        }
    }

    #[test]
    fn analysis_host_basic() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let u = url::Url::parse("file:///test.sail").unwrap();
        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");

        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        let sf = analysis.file(&u);
        assert!(sf.is_some());

        assert!(sf.unwrap().text().contains("val x"));
    }

    #[test]
    fn configuration_switch_excludes_retained_files_from_ide_queries() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let main = url::Url::parse("file:///main.sail").unwrap();
        let a = url::Url::parse("file:///a.sail").unwrap();
        let b = url::Url::parse("file:///b.sail").unwrap();
        set_text(
            &mut host,
            &mut files,
            &mut vfs,
            &main,
            "val main : int -> int\nfunction main(x) = selected(x)\nfunction probe(x) = on\n",
        );
        for (url, unique) in [(&a, "only_a"), (&b, "only_b")] {
            set_text(
                &mut host,
                &mut files,
                &mut vfs,
                url,
                &format!(
                    "val selected : int -> int\nfunction selected(x) = x\nval {unique} : int\n"
                ),
            );
        }
        let main_id = vfs.file_id_for_url(&main);
        let a_id = vfs.file_id_for_url(&a);
        let b_id = vfs.file_id_for_url(&b);
        for (active, inactive, active_id, inactive_id, unique, hidden) in [
            (&a, &b, a_id, b_id, "only_a", "only_b"),
            (&b, &a, b_id, a_id, "only_b", "only_a"),
            (&a, &b, a_id, b_id, "only_a", "only_b"),
        ] {
            let mut change = ide_db::apply_change::Change::new();
            change.set_workspace_files(vec![active_id, main_id]);
            host.apply_change(change);
            let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
            assert!(analysis.file(inactive).is_some(), "inactive buffer stays addressable");
            let targets =
                analysis.goto_definition(&main, ide_db::LineCol { line: 1, col: 21 }).unwrap();
            assert!(!targets.is_empty());
            assert!(targets.iter().all(|target| target.url == *active), "{targets:?}");
            let items =
                analysis.completions(main_id, ide_db::LineCol { line: 2, col: 22 }).unwrap();
            assert!(items.iter().any(|item| item.label == unique), "missing {unique}");
            assert!(!items.iter().any(|item| item.label == hidden), "leaked {hidden}");
            let refs = analysis
                .find_all_refs(active, ide_db::LineCol { line: 0, col: 5 }, None)
                .unwrap()
                .unwrap();
            assert!(refs.iter().any(|r| r.references.contains_key(&main_id)));
            assert!(refs.iter().all(|r| !r.references.contains_key(&inactive_id)
                && r.declarations.iter().all(|d| d.file_id != inactive_id)));
            assert!(analysis.workspace_symbols(hidden).unwrap().is_empty());
            assert!(!analysis.workspace_symbols(unique).unwrap().is_empty());
        }
    }

    /// A cycle belongs to the graph, so it is found where the graph is and
    /// reported on each file that is on it. The check that used to do this
    /// lived in `ide-diagnostics` and had no caller.
    #[test]
    fn circular_include_is_reported_on_every_file_in_the_cycle() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let a = url::Url::parse("file:///a.sail").unwrap();
        let b = url::Url::parse("file:///b.sail").unwrap();
        set_text(&mut host, &mut files, &mut vfs, &a, "$include \"b.sail\"\n");
        set_text(&mut host, &mut files, &mut vfs, &b, "$include \"a.sail\"\n");
        let fid_a = vfs.file_id_for_url(&a);
        let fid_b = vfs.file_id_for_url(&b);

        let mut graph = hir::include_graph::IncludeGraph::new();
        graph.add_edge(fid_a, fid_b);
        graph.add_edge(fid_b, fid_a);
        let analysis = Analysis::with_include_graph(
            host.db_snapshot(),
            files.clone(),
            url_map(&vfs),
            std::sync::Arc::new(graph),
        );

        let config = ide_diagnostics::DiagnosticsConfig::new();
        for fid in [fid_a, fid_b] {
            let diags = analysis
                .file_diagnostics(fid, &config)
                .expect("the test database is never cancelled");
            let cycle = diags.iter().find(|d| d.code.as_str() == "circular-include");
            assert!(cycle.is_some(), "no circular-include for {fid:?}: {diags:?}");
            assert!(cycle.unwrap().message.contains("a.sail"));
        }
    }

    /// `end x` with no `scattered x` is a type error upstream, but the head
    /// may be in any file — so the check only means anything with every file
    /// in hand.
    #[test]
    fn scattered_completeness_is_judged_across_files() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let a = url::Url::parse("file:///a.sail").unwrap();
        let b = url::Url::parse("file:///b.sail").unwrap();
        set_text(&mut host, &mut files, &mut vfs, &a, "end foo\n");
        set_text(&mut host, &mut files, &mut vfs, &b, "val unrelated : int -> int\n");
        let fid_a = vfs.file_id_for_url(&a);

        let config = ide_diagnostics::DiagnosticsConfig::new();
        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        let diags = analysis
            .file_diagnostics(fid_a, &config)
            .expect("the test database is never cancelled");
        let orphan = diags.iter().find(|d| d.code.as_str() == "incomplete-scattered");
        assert!(orphan.is_some(), "no incomplete-scattered: {diags:?}");
        assert!(orphan.unwrap().message.contains("cannot be ended"));

        // With the head in the other file the `end` is not an orphan, and a
        // per-file view — the one this replaced — could not have known.
        // The snapshot holds the database; a write blocks until it is gone.
        drop(analysis);
        set_text(
            &mut host,
            &mut files,
            &mut vfs,
            &b,
            "scattered function foo : int -> int\nfunction clause foo(n) = n\n",
        );
        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        let diags = analysis
            .file_diagnostics(fid_a, &config)
            .expect("the test database is never cancelled");
        assert!(
            !diags.iter().any(|d| d.code.as_str() == "incomplete-scattered"),
            "called an `end` an orphan whose head is in another file: {diags:?}"
        );
    }

    #[test]
    fn analysis_snapshot_isolation() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let u = url::Url::parse("file:///test.sail").unwrap();
        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");

        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        let sf = analysis.file(&u).unwrap();

        assert!(sf.text().contains("val x : int"));
    }

    #[test]
    fn analysis_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Analysis>();
    }

    #[test]
    fn multi_file_workspace() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let prelude = url::Url::parse("file:///prelude.sail").unwrap();
        let model = url::Url::parse("file:///model.sail").unwrap();

        set_text(
            &mut host,
            &mut files,
            &mut vfs,
            &prelude,
            "val add : (int, int) -> int\nfunction add(x, y) = x + y\n",
        );
        set_text(&mut host, &mut files, &mut vfs, &model, "function main() = add(1, 2)\n");

        let analysis = host.analysis_with_url_map(&files, url_map(&vfs));
        assert!(analysis.file(&prelude).is_some());
        assert!(analysis.file(&model).is_some());

        let prelude_id = analysis.file_id(&prelude).unwrap();
        let symbols = analysis.document_symbols(prelude_id).unwrap();
        assert!(!symbols.is_empty(), "prelude should have symbols");

        // The workspace's own view: `main.sail` resolves `add`, which only
        // the prelude declares. This used to ask `workspace_def_map` — an
        // `Analysis` method whose only caller was this line, and the last
        // thing keeping a whole-workspace `DefMap` builder alive after the
        // include-scoped one was deleted.
        let model_id = analysis.file_id(&model).unwrap();
        assert!(
            analysis
                .file_diagnostics(model_id, &ide_diagnostics::DiagnosticsConfig::new())
                .expect("the test database is never cancelled")
                .iter()
                .all(|d| !d.message.contains("add")),
            "`add` is declared in the prelude and must resolve from the model"
        );
    }

    #[test]
    fn content_dedup() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let u = url::Url::parse("file:///test.sail").unwrap();

        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");
        let text1 = {
            let a1 = host.analysis_with_url_map(&files, url_map(&vfs));
            a1.file(&u).unwrap().text().to_string()
        };
        // The snapshot has to be gone before the next write: applying a change
        // cancels in-flight queries and salsa's write waits for every live
        // handle. Reading through a snapshot that outlives the read is what
        // blocked `benchmark_incremental_diagnostics` forever.

        // Set same content again — should be a no-op (content-hash dedup)
        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");
        let a2 = host.analysis_with_url_map(&files, url_map(&vfs));
        let text2 = a2.file(&u).unwrap().text().to_string();

        assert_eq!(text1, text2);
    }
}
