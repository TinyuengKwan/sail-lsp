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
use hir_def::callgraph::SourceFileInfo;
use ide_db::root_database::{RootDatabase, SalsaFile};
use ide_db::FileDb;

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

    pub fn all_urls(&self) -> impl Iterator<Item = &url::Url> + '_ {
        self.url_to_file_id.keys()
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

impl AnalysisHost {
    /// Create a new AnalysisHost with optional LRU capacity hint.
    ///
    /// In salsa 0.25, LRU is configured per-query via `#[salsa::tracked(lru=N)]`,
    /// not at the database level. This parameter is accepted for API
    /// compatibility but currently unused.
    pub fn new(lru_capacity: Option<u16>) -> Self {
        let _ = lru_capacity; // TODO: wire to per-query LRU config if needed
        Self { db: RootDatabase::default() }
    }

    /// Update the LRU capacity on the underlying database.
    ///
    /// Currently a no-op in salsa 0.25 (LRU is per-query, not global).
    pub fn update_lru_capacity(&mut self, _lru_capacity: Option<u16>) {
        // TODO: In RA's salsa fork, this adjusts per-query LRU sizes.
        // salsa 0.25.2 configures LRU at compile-time via tracked(lru=N).
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

    /// Convenience: set file text through VFS + salsa db.
    /// Wraps VFS file_id allocation + durability-aware salsa write.
    ///
    /// The caller provides the FileId (obtained from `vfs::Vfs`).
    pub fn set_file_text(
        &mut self,
        files: &mut Files,
        file_id: FileId,
        text: &str,
        durability: base_db::Durability,
    ) {
        files.set_file_text_with_durability(&mut self.db, file_id, text, durability);
    }

    /// Apply a batch of file changes to the database.
    ///
    /// All changes are applied in a single salsa revision bump.
    pub fn apply_change(&mut self, change: base_db::FileChange) {
        change.apply(&mut self.db);
    }
}

impl Default for AnalysisHost {
    fn default() -> Self {
        Self::new(None)
    }
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
    /// Include graph for scope-aware analysis.
    include_graph: Option<std::sync::Arc<hir_def::include_graph::IncludeGraph>>,
}

impl Analysis {
    /// Create from an AnalysisHost db snapshot + Files (VFS) snapshot.
    pub fn new(db: RootDatabase, files: Files) -> Self {
        Self { db, files, url_map: UrlMap::default(), include_graph: None }
    }

    /// Create with URL map (from VFS snapshot).
    pub fn with_url_map(db: RootDatabase, files: Files, url_map: UrlMap) -> Self {
        Self { db, files, url_map, include_graph: None }
    }

    /// Create with include graph for scope-aware analysis.
    pub fn with_include_graph(
        db: RootDatabase,
        files: Files,
        url_map: UrlMap,
        graph: std::sync::Arc<hir_def::include_graph::IncludeGraph>,
    ) -> Self {
        Self { db, files, url_map, include_graph: Some(graph) }
    }

    /// Get the include graph, if available.
    pub fn include_graph(&self) -> Option<&hir_def::include_graph::IncludeGraph> {
        self.include_graph.as_deref()
    }
}

impl Analysis {
    /// Get a SalsaFile by FileId.: Analysis methods take FileId.
    pub fn file_by_id(&self, id: FileId) -> Option<SalsaFile<'_>> {
        let ft = self.files.file_text(id)?;
        Some(SalsaFile::new(&self.db, ft))
    }

    /// Resolve URL to FileId. Called at handler layer (not inside Analysis methods).
    /// Analogous to RA's `GlobalStateSnapshot::file_id()`.
    pub fn file_id(&self, url: &url::Url) -> Option<FileId> {
        self.url_map.lookup_file_id(url)
    }

    /// Get the URL for a FileId.
    pub fn url_for_file_id(&self, id: FileId) -> Option<&url::Url> {
        self.url_map.lookup_url(id)
    }

    /// Iterate all known file URLs.
    pub fn all_file_urls(&self) -> impl Iterator<Item = &url::Url> + '_ {
        self.url_map.all_urls()
    }

    /// Get the underlying salsa database (for advanced queries).
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

    /// Build a workspace-wide definition map from all files' ItemTrees.
    pub fn workspace_def_map(&self) -> hir_def::WorkspaceDefMap {
        let mut file_trees: Vec<(usize, hir_def::ItemTree)> = Vec::new();
        for (idx, (_url, fid)) in self.url_map.all_url_file_ids().enumerate() {
            if let Some(ft) = self.files.file_text(fid) {
                let sf = SalsaFile::new(&self.db, ft);
                if let Some(tree) = sf.item_tree() {
                    file_trees.push((idx, tree.clone()));
                }
            }
        }
        let refs: Vec<(usize, &hir_def::ItemTree)> =
            file_trees.iter().map(|(idx, tree)| (*idx, tree)).collect();
        hir_def::WorkspaceDefMap::build(&refs)
    }

    /// Build a scope-aware workspace definition map.
    ///
    /// If the include graph is available, only includes definitions
    /// from files within the AnalysisScope of `root_file`. Otherwise
    /// falls back to the full workspace def map.
    pub fn workspace_def_map_for_file(&self, root_url: &url::Url) -> hir_def::WorkspaceDefMap {
        let root_fid = match self.file_id(root_url) {
            Some(id) => id,
            None => return self.workspace_def_map(),
        };

        let graph = match self.include_graph() {
            Some(g) => g,
            None => return self.workspace_def_map(),
        };

        let scope = hir_def::analysis_scope::AnalysisScope::from_include_graph(graph, root_fid);

        // Collect (FileId, file_index, &ItemTree) for all files
        let mut file_trees: Vec<(base_db::FileId, usize, hir_def::ItemTree)> = Vec::new();
        for (idx, (_url, fid)) in self.url_map.all_url_file_ids().enumerate() {
            if let Some(ft) = self.files.file_text(fid) {
                let sf = SalsaFile::new(&self.db, ft);
                if let Some(tree) = sf.item_tree() {
                    file_trees.push((fid, idx, tree.clone()));
                }
            }
        }
        let refs: Vec<(base_db::FileId, usize, &hir_def::ItemTree)> =
            file_trees.iter().map(|(fid, idx, tree)| (*fid, *idx, tree)).collect();
        hir_def::WorkspaceDefMap::build_for_scope(&scope, &refs)
    }

    /// Create a SalsaWorkspace from this Analysis. The workspace
    /// stores pre-created SalsaFiles so they can be referenced by
    /// WorkspaceDb::get_file() and all_files().
    pub fn workspace(&self) -> SalsaWorkspace<'_> {
        let files: Vec<(url::Url, SalsaFile<'_>)> = self
            .url_map
            .all_url_file_ids()
            .filter_map(|(url, id)| {
                let ft = self.files.file_text(id)?;
                Some((url.clone(), SalsaFile::new(&self.db, ft)))
            })
            .collect();
        SalsaWorkspace { files }
    }

    /// Build a list of all files as `(Url, SalsaFile)` pairs.
    /// Callers can iterate with `.iter().map(|(u, s)| (u, s))` to get
    /// `(&Url, &SalsaFile)` for generic `F: FileDb` functions.
    pub fn all_salsa_files(&self) -> Vec<(url::Url, SalsaFile<'_>)> {
        self.url_map
            .all_url_file_ids()
            .filter_map(|(url, id)| {
                let ft = self.files.file_text(id)?;
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
    pub fn document_symbols(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::NavigationTarget>> {
        match self.file_by_id(file_id) {
            Some(f) => Ok(ide_db::symbol_index::document_symbols_ide(&f)),
            None => Ok(Vec::new()),
        }
    }

    /// Diagnostics for a file — salsa-backed, Cancellable wrapper.
    ///
    /// `ide_diagnostics::full_diagnostics()` (two-pipeline architecture).
    pub fn diagnostics(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_diagnostics::Diagnostic>> {
        let config = ide_diagnostics::DiagnosticsConfig::new();
        Ok(self.file_diagnostics(file_id, &config))
    }

    /// Signature index for a file.: takes FileId.
    pub fn signature_index(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<
        Option<std::sync::Arc<HashMap<String, ide_db::CallableSignature>>>,
    > {
        let ft = match self.files.file_text(file_id) {
            Some(ft) => ft,
            None => return Ok(None),
        };
        Ok(Some(ide_db::db_query::signature_index(&self.db, ft).clone()))
    }

    /// Hover information for a symbol at position.
    pub fn hover(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Option<ide_db::ide_types::HoverResult>> {
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
        let owned = self.all_salsa_files_vec();
        let refs: Vec<(&url::Url, &SalsaFile<'_>)> = owned.iter().map(|(u, sf)| (u, sf)).collect();
        Ok(crate::hover::hover_for_symbol(
            refs.iter().map(|(u, f)| (*u, *f)),
            url,
            &sf as &dyn FileDb,
            position,
            range,
            &symbol_key,
        ))
    }

    /// Completions at position.
    ///
    /// `ide_completion::completions()`.
    pub fn completions(
        &self,
        file_id: FileId,
        position: ide_db::LineCol,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::CompletionItem>> {
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
        let file_text = self.files.file_text(file_id);
        let owned = self.all_salsa_files_vec();
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
    pub fn inlay_hints(
        &self,
        file_id: FileId,
        range: ide_db::line_index::TextRange,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::InlayHint>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(Vec::new()),
        };
        let url = match self.url_for_file_id(file_id) {
            Some(u) => u,
            None => return Ok(Vec::new()),
        };
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();

        // B6-1: Thread TypeCheckResult from salsa for type hints.
        let ft = self.files.file_text(file_id);
        let type_check = ft.map(|ft| hir_ty::query::infer_body(&self.db, ft));

        // Thread salsa-cached transitive effects for effect hints.
        let transitive_effects = ft.map(|ft| hir_ty::query::transitive_effects(&self.db, ft));

        Ok(crate::inlay_hints::inlay_hints_ide_with_types(
            &all,
            url,
            &sf as &dyn FileDb,
            range,
            type_check.map(|tc| &*tc.0),
            transitive_effects.map(|te| &*te.0),
        ))
    }

    /// Code lenses for a file.
    pub fn code_lenses(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<Vec<ide_db::ide_types::Annotation>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(Vec::new()),
        };
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        let ref_counts = crate::annotations::collect_reference_counts(&all);
        let impl_counts = crate::annotations::collect_implementation_counts(&all);
        Ok(crate::annotations::code_lenses_ide(&sf, &ref_counts, &impl_counts))
    }

    /// Semantic tokens for a file.: takes FileId.
    pub fn semantic_tokens(
        &self,
        file_id: FileId,
    ) -> ide_db::ide_types::Cancellable<ide_db::ide_types::HlRanges> {
        match self.file_by_id(file_id) {
            Some(f) => Ok(crate::syntax_highlighting::compute_semantic_tokens(&f)),
            None => Ok(ide_db::ide_types::HlRanges { result_id: None, data: Vec::new() }),
        }
    }

    /// Format a document.: takes FileId.
    pub fn format_document(
        &self,
        file_id: FileId,
        tab_size: u32,
        insert_spaces: bool,
    ) -> ide_db::ide_types::Cancellable<Option<Vec<ide_db::ide_types::IdeTextEdit>>> {
        let sf = match self.file_by_id(file_id) {
            Some(f) => f,
            None => return Ok(None),
        };
        let opts =
            ide_db::ide_types::FormatOptions { tab_size, insert_spaces, ..Default::default() };
        Ok(crate::formatting::format_document_edits(&sf, &opts))
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
    ) -> Vec<ide_diagnostics::Diagnostic> {
        let ft = match self.files.file_text(file_id) {
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
    ) -> Vec<ide_diagnostics::Diagnostic> {
        let ft = match self.files.file_text(file_id) {
            Some(ft) => ft,
            None => return Vec::new(),
        };
        let sf = SalsaFile::new(&self.db, ft);

        // Build workspace cross-file name sets from all files'
        // TopLevelEnv (salsa-cached, no recomputation cost).
        let ws_names = self.build_workspace_names();

        let resolve = ide_db::assists::AssistResolveStrategy::All;
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
    pub fn build_workspace_names(&self) -> ide_diagnostics::WorkspaceNames {
        let mut ws = ide_diagnostics::WorkspaceNames::default();
        for (_url, fid) in self.url_map.all_url_file_ids() {
            if let Some(ft) = self.files.file_text(fid) {
                let env_data = hir_ty::query::top_level_env(&self.db, ft);
                let env = &env_data.0.env;
                ws.function_names.extend(env.function_names().cloned());
                ws.constructor_names.extend(env.constructor_names().cloned());
                ws.value_names.extend(env.value_names().cloned());
                ws.value_names.extend(env.register_names().cloned());
                ws.record_names.extend(env.record_names().cloned());
            }
        }
        ws
    }

    /// Get inferred type text by querying each per-callable InferenceResult.
    /// Query per-item, not per-file merged result.
    pub fn expr_type_text(&self, file_id: FileId, span: parser::Span) -> Option<String> {
        let ft = self.files.file_text(file_id)?;
        let sf = SalsaFile::new(&self.db, ft);
        let callable_ids = hir_def::def_query::file_def_with_body_ids(&self.db, ft);
        let bodies = sf.bodies();
        for &id in callable_ids {
            let result = hir_ty::query::infer(&self.db, id);
            if let Some(ty_text) = result.0.expr_type_text(span, bodies) {
                return Some(ty_text);
            }
        }
        None
    }

    /// Get inferred type text for a binding by querying per-callable.
    pub fn binding_type_text(&self, file_id: FileId, span: parser::Span) -> Option<String> {
        let ft = self.files.file_text(file_id)?;
        let sf = SalsaFile::new(&self.db, ft);
        let callable_ids = hir_def::def_query::file_def_with_body_ids(&self.db, ft);
        let bodies = sf.bodies();
        for &id in callable_ids {
            let result = hir_ty::query::infer(&self.db, id);
            if let Some(ty_text) = result.0.binding_type_text(span, bodies) {
                return Some(ty_text);
            }
        }
        None
    }

    /// Workspace-level reference counts — computed on demand.
    pub fn ref_counts(&self) -> HashMap<String, usize> {
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        crate::annotations::collect_reference_counts(&all)
    }

    /// Workspace-level implementation counts — computed on demand.
    pub fn impl_counts(&self) -> HashMap<String, usize> {
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        crate::annotations::collect_implementation_counts(&all)
    }

    // Handlers call `analysis.method()` which internally
    // constructs the file adapter. This generic method enables
    // handlers to pass a closure that operates on &dyn FileDb.

    /// Run a closure with a SalsaFile for the given URL.
    /// Returns `None` if the file is not known.
    /// This is the primary entry point for migrating handlers from
    /// `state.salsa_file()` to `analysis.with_file()`.
    pub fn with_file<T>(
        &self,
        url: &url::Url,
        f: impl FnOnce(&SalsaFile<'_>, &url::Url) -> T,
    ) -> Option<T> {
        let sf = self.file(url)?;
        Some(f(&sf, url))
    }

    /// Run a closure with a SalsaFile + all workspace files.
    /// For handlers that need workspace-wide context.
    pub fn with_file_and_workspace<T>(
        &self,
        url: &url::Url,
        f: impl FnOnce(&SalsaFile<'_>, &url::Url, &[(url::Url, SalsaFile<'_>)]) -> T,
    ) -> Option<T> {
        let sf = self.file(url)?;
        let all = self.all_salsa_files();
        Some(f(&sf, url, &all))
    }

    /// Signature help at position.
    pub fn signature_help(
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
        let owned = self.all_salsa_files_vec();
        let all: Vec<(&url::Url, &dyn FileDb)> =
            owned.iter().map(|(u, sf)| (u, sf as &dyn FileDb)).collect();
        Ok(crate::calls::signature_help_ide(&all, url, &sf as &dyn FileDb, position))
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

impl Analysis {
    /// File structure (outline / document symbols).
    pub fn file_structure(
        &self,
        file_id: FileId,
    ) -> crate::Cancellable<Vec<ide_db::ide_types::NavigationTarget>> {
        let sf = match self.file_by_id(file_id) {
            Some(sf) => sf,
            None => return Ok(Vec::new()),
        };
        Ok(ide_db::symbol_index::document_symbols_ide(&sf))
    }

    // diagnostics() already defined above at line 271 with Cancellable return.
}

#[cfg(test)]
mod tests {
    use super::*;
    use base_db::Durability;

    /// Test helper: allocate FileId via VFS and set file text.
    fn set_text(
        host: &mut AnalysisHost,
        files: &mut Files,
        vfs: &mut vfs::Vfs,
        url: &url::Url,
        text: &str,
    ) {
        let fid = vfs.file_id_for_url(url);
        host.set_file_text(files, fid, text, Durability::LOW);
    }

    /// Build a UrlMap from a Vfs snapshot (test helper).
    fn url_map(vfs: &vfs::Vfs) -> UrlMap {
        let (a, b) = vfs.url_map_snapshot();
        UrlMap::new(a, b)
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

        let wdm = analysis.workspace_def_map();
        assert!(wdm.contains("add"), "add should be in workspace");
        assert!(wdm.contains("main"), "main should be in workspace");
    }

    #[test]
    fn content_dedup() {
        let mut host = AnalysisHost::new(None);
        let mut files = Files::default();
        let mut vfs = vfs::Vfs::default();
        let u = url::Url::parse("file:///test.sail").unwrap();

        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");
        let a1 = host.analysis_with_url_map(&files, url_map(&vfs));
        let text1 = a1.file(&u).unwrap().text().to_string();

        // Set same content again — should be a no-op (content-hash dedup)
        set_text(&mut host, &mut files, &mut vfs, &u, "val x : int\n");
        let a2 = host.analysis_with_url_map(&files, url_map(&vfs));
        let text2 = a2.file(&u).unwrap().text().to_string();

        assert_eq!(text1, text2);
    }
}
