//! GlobalState and GlobalStateSnapshot — server state management.
//!
//! `global_state.rs` contains the GlobalState struct, its
//! constructor, and base methods (process_changes, snapshot, respond, etc.).
//! Event-loop-specific methods live in `main_loop.rs`.

use std::sync::Arc;

use crossbeam_channel::Sender;
use ide::analysis::Analysis;
use ide_db::root_database::SalsaFile;
use ide_db::workspace_index::SymbolIndex;
use lsp_server::{Message, Notification, Response};
use lsp_types::*;
use url::Url;

use crate::config::SailLspConfig;

pub(crate) struct GlobalState {
    /// MemDocs tracks open documents (text + version).
    pub mem_docs: crate::mem_docs::MemDocs,

    /// AnalysisHost { db } — sole owner of salsa database.
    pub analysis_host: ide::analysis::AnalysisHost,
    /// VFS — URL↔FileId mapping.
    pub vfs: vfs::Vfs,

    /// include dependency graph built during workspace scan.
    /// Tracks which files `$include` which other files.
    pub include_graph: hir_def::include_graph::IncludeGraph,

    /// SourceRoots — local vs library file classification.
    /// Library files (from `$include <lib>`) get HIGH durability and
    /// are never re-analyzed unless the workspace is reloaded.
    pub local_root: base_db::SourceRoot,
    pub library_root: base_db::SourceRoot,

    /// User configuration (from workspace/didChangeConfiguration).
    pub config: crate::config::SailLspConfig,

    /// Workspace-wide symbol index for O(1) name lookups.
    pub workspace_index: std::sync::Arc<ide_db::workspace_index::SymbolIndex>,
    /// Workspace folders for reload.
    pub workspace_folders: std::collections::HashSet<Url>,
    /// Files that changed since last index update.
    pub index_dirty_files: Vec<Url>,

    /// Whether client supports window/workDoneProgress.
    pub supports_progress: bool,

    /// Client capabilities received during initialization.
    /// with typed accessors.
    pub(crate) client_caps: crate::lsp::capabilities::ClientCapabilities,

    /// Task_pool is part of GlobalState.
    /// Generic `TaskPool<Task>` — Task defined in main_loop.rs.
    pub task_pool: crate::task_pool::TaskPool<crate::main_loop::Task>,

    /// Sender for pushing notifications back to the client.
    pub sender: Sender<Message>,

    /// F3: Last reported server status (for change detection).
    last_status: Option<crate::lsp_ext::ServerStatusParams>,

    /// Per-file diagnostic state with generation tracking.
    pub(crate) diagnostics: crate::diagnostics::DiagnosticCollection,
    /// Per-file semantic token cache for delta requests.
    pub(crate) semantic_tokens_cache: crate::lsp::semantic_tokens::SemanticTokensCache,

    /// Workspace scan/reload queue. Serializes workspace discovery operations.
    pub(crate) fetch_workspaces_queue: crate::op_queue::OpQueue,
    /// Prime caches queue. Serializes cache pre-warming operations.
    pub(crate) prime_caches_queue: crate::op_queue::OpQueue,
    // tracked query. salsa handles caching/invalidation automatically.
    // Diagnostics are triggered directly in the quiescent block when
    // `became_quiescent || state_changed || memdocs_changed`.
    pub(crate) last_gc_revision: u64,
    /// Shared cancellation token for prime_caches.
    /// (`global_state.rs:355 trigger_cancellation()`), which causes
    /// in-progress prime_caches to bail early. A new token is created
    /// when prime is re-queued.
    pub(crate) prime_caches_cancel: hir_ty::CancellationToken,

    /// Whether the workspace has been fully loaded (scan + index complete).
    /// has completed its initial load and is ready for feature requests.
    /// Set to `true` after the first `Task::WorkspaceScan` completes.
    #[allow(dead_code)]
    pub(crate) workspace_loaded: bool,

    /// Whether a shutdown has been requested.
    /// Set when `shutdown` request arrives; prevents new work from starting.
    #[allow(dead_code)]
    pub(crate) shutdown_requested: bool,

    /// Source root configuration: maps FileId → is_library.
    /// file should use HIGH durability (library) or LOW durability (local).
    /// Populated during workspace scan from `$SAIL_DIR` detection.
    #[allow(dead_code)]
    pub(crate) source_root_config: SourceRootConfig,

    /// Effective `SAIL_DIR` path — either from `$SAIL_DIR` env var or from
    /// the embedded stdlib materialised to a temp directory.
    /// `$SAIL_DIR/lib/` contains the Sail standard library files.
    /// When the env var is not set, the embedded stdlib is written to disk
    /// at server start and this field points to that directory.
    pub(crate) effective_sail_dir: Option<std::path::PathBuf>,

    /// Cached formatting options from the last `textDocument/formatting`
    /// request. Used by code actions ("Format document") so they apply
    /// the same tab_size / insert_spaces settings the client sends for
    /// standard formatting. Without this, code actions use hardcoded
    /// defaults that may differ from the editor's configuration.
    pub(crate) last_format_options:
        std::sync::Arc<std::sync::Mutex<ide_db::ide_types::FormatOptions>>,
}

/// Source root classification for files.
/// and whether changes should trigger workspace rebuild.
#[derive(Clone, Debug, Default)]
pub(crate) struct SourceRootConfig {
    /// FileIds classified as library roots (HIGH durability, $SAIL_DIR).
    pub(crate) library_file_ids: std::collections::HashSet<base_db::FileId>,
}

impl SourceRootConfig {
    /// Check if a FileId belongs to a library source root.
    #[allow(dead_code)]
    pub(crate) fn is_library(&self, file_id: base_db::FileId) -> bool {
        self.library_file_ids.contains(&file_id)
    }
}

impl GlobalState {
    pub(crate) fn new(
        sender: Sender<Message>,
        task_sender: crossbeam_channel::Sender<crate::main_loop::Task>,
    ) -> Self {
        Self {
            mem_docs: crate::mem_docs::MemDocs::default(),
            analysis_host: ide::analysis::AnalysisHost::new(None),
            vfs: vfs::Vfs::default(),
            include_graph: hir_def::include_graph::IncludeGraph::new(),
            local_root: base_db::SourceRoot::new_local(base_db::FileSet::default()),
            library_root: base_db::SourceRoot::new_library(base_db::FileSet::default()),
            config: crate::config::SailLspConfig::default(),
            workspace_folders: std::collections::HashSet::new(),
            workspace_index: std::sync::Arc::new(ide_db::workspace_index::SymbolIndex::new()),
            index_dirty_files: Vec::new(),
            supports_progress: false,
            client_caps: crate::lsp::capabilities::ClientCapabilities::default(),
            task_pool: crate::task_pool::TaskPool::new_with_threads(task_sender, 2),
            sender,
            last_status: None,
            diagnostics: crate::diagnostics::DiagnosticCollection::default(),
            semantic_tokens_cache: crate::lsp::semantic_tokens::SemanticTokensCache::new(),
            fetch_workspaces_queue: crate::op_queue::OpQueue::default(),
            prime_caches_queue: crate::op_queue::OpQueue::default(),
            last_gc_revision: 0,
            prime_caches_cancel: hir_ty::CancellationToken::new(),
            workspace_loaded: false,
            shutdown_requested: false,
            source_root_config: SourceRootConfig::default(),
            effective_sail_dir: Self::resolve_sail_dir(),
            last_format_options: std::sync::Arc::new(std::sync::Mutex::new(
                ide_db::ide_types::FormatOptions::default(),
            )),
        }
    }

    /// Determine the effective SAIL_DIR: prefer `$SAIL_DIR` env var, fall
    /// back to materialising the embedded stdlib into a temp directory.
    fn resolve_sail_dir() -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("SAIL_DIR") {
            let p = std::path::PathBuf::from(&dir);
            if p.join("lib").is_dir() {
                log::info!("using $SAIL_DIR from environment: {dir}");
                return Some(p);
            }
            log::warn!("$SAIL_DIR={dir} has no lib/ subdirectory, falling back to embedded stdlib");
        }

        // Materialise embedded stdlib to a deterministic temp path so it
        // survives across scan_workspace_folders calls.
        let tmp = std::env::temp_dir().join("sail-lsp-stdlib");
        match crate::sail_stdlib::materialise_stdlib(&tmp) {
            Ok(sail_dir) => {
                log::info!("materialised embedded Sail stdlib to {}", sail_dir.display());
                Some(sail_dir)
            }
            Err(e) => {
                log::error!("failed to materialise embedded stdlib: {e}");
                None
            }
        }
    }

    /// Send a response back to the client.
    pub(crate) fn respond(&self, response: Response) {
        self.sender.send(Message::Response(response)).ok();
    }

    /// Send a server→client request (e.g., refresh semantic tokens).
    /// like `workspace/semanticTokens/refresh` where the server asks
    /// the client to re-pull data.
    pub(crate) fn send_request<R: lsp_types::request::Request>(&self, params: R::Params) {
        use std::sync::atomic::{AtomicI32, Ordering};
        static REQ_ID: AtomicI32 = AtomicI32::new(1);
        let id = REQ_ID.fetch_add(1, Ordering::SeqCst);
        let request = lsp_server::Request::new(
            lsp_server::RequestId::from(id),
            R::METHOD.to_string(),
            params,
        );
        self.sender.send(Message::Request(request)).ok();
    }

    /// Compute current server health status.
    /// Send status notification if changed.
    pub(crate) fn update_status_or_notify(&mut self) {
        let status = self.current_status();
        if self.last_status.as_ref() != Some(&status) {
            self.last_status = Some(status.clone());
            self.send_notification::<crate::lsp_ext::ServerStatusNotification>(status);
        }
    }

    /// Buffer file change to VFS pending queue (LOW durability).
    /// Changes are flushed to salsa in process_changes().
    /// Notifications only buffer; main loop applies batch.
    pub(crate) fn sync_to_salsa(&mut self, uri: &Url, text: &str) {
        let fid = self.vfs.file_id_for_url(uri);
        self.analysis_host.raw_database_mut().files_mut().set_file_contents(
            fid,
            text,
            base_db::Durability::LOW,
        );
    }

    /// Apply all buffered VFS changes to salsa in a single batch.
    /// Called once per main_loop iteration before dispatching requests.
    /// Returns true if any changes were applied.
    /// Triggers cancellation BEFORE applying changes so in-flight
    /// queries on the old revision will unwind via `salsa::Cancelled`.
    pub(crate) fn process_changes(&mut self) -> bool {
        if self.analysis_host.raw_database().files().has_pending() {
            // Trigger cancellation of any in-flight queries before mutating the db.
            self.analysis_host.trigger_cancellation();

            // Cancel in-progress prime_caches on edit.
            self.prime_caches_cancel.cancel();
            self.prime_caches_cancel = hir_ty::CancellationToken::new();
            // NOTE: Do NOT re-queue prime_caches on every edit.
            // Salsa handles incremental updates automatically — editing a
            // file invalidates only the affected queries. Re-priming all
            // 158 files on every keystroke blocks the main thread 0.2-0.6s.
            // Prime is only needed after workspace scan (initial load).
        }
        self.analysis_host.raw_database_mut().apply_pending_file_changes()
    }

    /// Rebuild workspace context for cross-file type inference.
    /// Ensure the `WorkspaceFiles` salsa input lists all current files.
    /// - `WorkspaceFiles` is a salsa singleton listing all `FileText` handles
    /// - `workspace_context(db, ws)` is a salsa tracked query that
    ///   aggregates cross-file data from all files' `top_level_env`
    /// - `infer` depends on it automatically via salsa
    ///
    /// Called on workspace scan completion and file add/remove events.
    /// NOT called on every edit — content changes flow through `FileText`
    /// → `top_level_env` → `workspace_context` automatically.
    pub(crate) fn ensure_workspace_files(&mut self) {
        let db = self.analysis_host.raw_database();
        let file_texts: Vec<base_db::FileText> = db
            .files()
            .all_file_ids()
            .into_iter()
            .filter_map(|fid| db.files().file_text(fid))
            .collect();
        let db = self.analysis_host.raw_database_mut();
        if let Some(existing) = base_db::WorkspaceFiles::try_get(db) {
            // Only update if the file list actually changed (file add/remove).
            let current = existing.file_texts(db);
            if current.len() != file_texts.len() || *current != file_texts {
                use salsa::Setter;
                existing.set_file_texts(db).to(file_texts);
            }
        } else {
            base_db::WorkspaceFiles::new(db, file_texts);
        }
    }

    /// Create Analysis snapshot from AnalysisHost + VFS.
    /// Includes the IncludeGraph for scope-aware analysis.
    pub(crate) fn analysis_snapshot(&mut self) -> Analysis {
        // Flush pending VFS changes before snapshot
        self.analysis_host.raw_database_mut().apply_pending_file_changes();
        let files = self.analysis_host.raw_database().files().clone();
        let db = self.analysis_host.db_snapshot();
        let (url_to_fid, fid_to_url) = self.vfs.url_map_snapshot();
        let url_map = ide::analysis::UrlMap::new(url_to_fid, fid_to_url);
        Analysis::with_include_graph(
            db,
            files,
            url_map,
            std::sync::Arc::new(self.include_graph.clone()),
        )
    }

    /// Create GlobalStateSnapshot for request handlers.
    /// Immutable snapshot passed to handler (sync or async).
    pub(crate) fn snapshot(&mut self) -> GlobalStateSnapshot {
        GlobalStateSnapshot {
            analysis: self.analysis_snapshot(),
            config: self.config.clone(),
            workspace_index: self.workspace_index.clone(),
            semantic_tokens_cache: self.semantic_tokens_cache.clone(),
            include_graph: Arc::new(self.include_graph.clone()),
            client_caps: self.client_caps.clone(),
            last_format_options: self.last_format_options.clone(),
        }
    }

    /// Buffer disk file change to VFS pending queue (HIGH durability).
    pub(crate) fn sync_disk_file(&mut self, uri: &Url, text: &str) {
        let fid = self.vfs.file_id_for_url(uri);
        self.analysis_host.raw_database_mut().files_mut().set_file_contents(
            fid,
            text,
            base_db::Durability::HIGH,
        );
    }

    /// Send a notification to the client.
    pub(crate) fn send_notification<N: lsp_types::notification::Notification>(
        &self,
        params: N::Params,
    ) {
        let notif = Notification::new(N::METHOD.to_string(), params);
        let _ = self.sender.send(Message::Notification(notif));
    }

    /// Return all salsa-tracked files as `Vec<(&Url, SalsaFile)>`.
    /// Kept for `file_locations_to_lsp`; most handlers now use
    /// `analysis.all_salsa_files()` via the snapshot pattern.
    #[allow(dead_code)]
    pub(crate) fn all_salsa_files(&self) -> Vec<(&Url, SalsaFile<'_>)> {
        let db = self.analysis_host.raw_database();
        self.vfs
            .all_url_file_ids()
            .filter_map(|(url, fid)| {
                let ft = db.files().file_text(fid)?;
                Some((url, SalsaFile::new(db, ft)))
            })
            .collect()
    }

    /// Log a message to the client.
    pub(crate) fn log_message(&self, typ: MessageType, msg: impl Into<String>) {
        self.send_notification::<lsp_types::notification::LogMessage>(LogMessageParams {
            typ,
            message: msg.into(),
        });
    }

    /// Get a SalsaFile for the given Url.
    /// Prefer `state.snapshot().file(uri)` for new handlers (:
    /// handlers operate on Analysis snapshots for cancellation isolation).
    /// This method is equivalent but bypasses the snapshot.
    #[allow(dead_code)] // TODO: convenience method, prefer snapshot pattern
    pub(crate) fn salsa_file(&self, uri: &Url) -> Option<SalsaFile<'_>> {
        let db = self.analysis_host.raw_database();
        let id = self.vfs.lookup_file_id_by_url(uri)?;
        let ft = db.files().file_text(id)?;
        Some(SalsaFile::new(db, ft))
    }
}

/// Immutable snapshot of server state passed to request handlers.
/// Contains everything a handler needs to process a request without
/// mutating shared state. Clone is cheap (salsa Arc + index Arc).
#[derive(Clone)]
pub(crate) struct GlobalStateSnapshot {
    /// Immutable Analysis snapshot (salsa db + Files).
    pub analysis: Analysis,
    /// Server configuration.
    pub config: SailLspConfig,
    /// Workspace-wide symbol index for O(1) name lookups.
    pub workspace_index: Arc<SymbolIndex>,
    /// Per-file semantic token cache (shared via Arc<Mutex>).
    pub semantic_tokens_cache: crate::lsp::semantic_tokens::SemanticTokensCache,
    /// Include dependency graph for cross-file diagnostic propagation.
    /// Sail-specific: explicit $include graph for cross-file diagnostics.
    pub include_graph: Arc<hir_def::include_graph::IncludeGraph>,
    /// Client capabilities for feature detection.
    pub client_caps: crate::lsp::capabilities::ClientCapabilities,
    /// Cached formatting options from the last `textDocument/formatting`
    /// request. Shared via Arc<Mutex> with GlobalState.
    pub last_format_options: std::sync::Arc<std::sync::Mutex<ide_db::ide_types::FormatOptions>>,
}
