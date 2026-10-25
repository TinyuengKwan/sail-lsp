//! GlobalState and GlobalStateSnapshot — server state management.
//!
//! `global_state.rs` contains the GlobalState struct, its
//! constructor, and base methods (process_changes, snapshot, respond, etc.).
//! Event-loop-specific methods live in `main_loop.rs`.

use std::sync::Arc;

use crossbeam_channel::Sender;
use ide::analysis::Analysis;
use lsp_server::{Message, Notification, Response};
use lsp_types::*;
use url::Url;

use crate::config::SailLspConfig;

/// What to do with a response the client sends back.
pub(crate) type ReqHandler = fn(&mut GlobalState, lsp_server::Response);
type ReqQueue = lsp_server::ReqQueue<(String, std::time::Instant), ReqHandler>;

pub(crate) struct GlobalState {
    /// MemDocs tracks open documents (text + version).
    pub mem_docs: crate::mem_docs::MemDocs,

    /// AnalysisHost { db } — sole owner of salsa database.
    pub analysis_host: ide::analysis::AnalysisHost,
    /// VFS — URL↔FileId mapping.
    pub vfs: vfs::Vfs,

    /// Edits queued since the last database revision. Applied as one batch by
    /// `process_changes`, so a handler never observes a half-updated database.
    pub pending_change: ide_db::apply_change::Change,

    /// include dependency graph built during workspace scan.
    /// Tracks which files `$include` which other files.
    pub include_graph: hir::include_graph::IncludeGraph,

    /// SourceRoots — local vs library file classification.
    /// Library files (from `$include <lib>`) get HIGH durability and
    /// are never re-analyzed unless the workspace is reloaded.
    pub local_root: base_db::SourceRoot,
    pub library_root: base_db::SourceRoot,

    /// User configuration (from workspace/didChangeConfiguration).
    pub config: crate::config::SailLspConfig,

    /// Workspace folders for reload.
    pub workspace_folders: std::collections::HashSet<Url>,

    /// Whether client supports window/workDoneProgress.
    pub supports_progress: bool,

    /// Client capabilities received during initialization.
    /// with typed accessors.
    pub(crate) client_caps: crate::lsp::capabilities::ClientCapabilities,
    /// Requests in flight, in both directions.
    ///
    /// rust-analyzer's `req_queue`. Without one, a response from the client
    /// has nowhere to go — the main loop dropped every `Message::Response`
    /// on the floor — so nothing the server asks the client can be
    /// answered: not `workspace/configuration`, not a progress token, not
    /// a capability registration. And an incoming request that is never
    /// registered cannot be cancelled: `$/cancelRequest` had nothing to
    /// look up.
    pub(crate) req_queue: ReqQueue,
    /// Whether the client has been asked to watch the workspace's files.
    pub(crate) registered_file_watchers: bool,
    /// Paths consulted by discovery, including absent include candidates.
    pub(crate) discovery_watch_files: Vec<std::path::PathBuf>,
    /// How the server reads a file it does not have, and — when the client
    /// registers no watchers — how it notices one changing.
    ///
    /// rust-analyzer always has one, and now so does this: it was created
    /// only in the fallback case, so on a client that watches for us
    /// `didClose` had no loader to ask and read the file by hand. `Option`
    /// remains for a document with no workspace folder behind it.
    pub(crate) loader: Option<Box<dyn vfs::loader::Handle>>,
    /// Changes the loader has seen.
    pub(crate) loader_receiver: Option<crossbeam_channel::Receiver<vfs::loader::Message>>,
    /// The scan's answer, waiting for the loader to have read it.
    ///
    /// The workspace is `(files in semantic order, count)` between
    /// `Task::WorkspaceScan` and the loader's `Progress { Finished }`. It is
    /// `Some` for exactly that window, which is also the window
    /// `fetch_workspaces_queue` stays in progress for — so the server is not
    /// quiescent, and nothing asks a question of a workspace with no text in
    /// it yet.
    pub(crate) pending_workspace: Option<(Vec<(Url, std::path::PathBuf)>, usize)>,
    /// Which load the barrier is waiting for.
    ///
    /// The loader answers `Progress { Finished }` for *every* config it is
    /// given, and it is given one when it is created. Taking the first one
    /// that arrives finished the workspace before a single file had been
    /// read — an environment with no text in it, which is the flood of
    /// "not found" this whole split exists to avoid. The version says which
    /// answer belongs to which question.
    pub(crate) workspace_load_version: u32,
    pub(crate) loading_project_config:
        Option<(crate::config::WorkspaceConfig, std::collections::HashSet<Url>, u64)>,
    pub(crate) loading_project_inputs: crate::reload::DiscoveryInputs,
    pub(crate) project_error: Option<String>,
    /// Settings the client sent that could not be read, last reported.
    pub(crate) config_errors: Vec<String>,
    pub(crate) project_manifests: Vec<std::path::PathBuf>,
    pub(crate) loading_module_graph: Option<base_db::module_graph::ModuleGraph>,
    pub(crate) loading_source_project: Option<base_db::preprocess::SourceProject>,

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
    /// The progress sequence a running cache-priming reports under.
    pub(crate) prime_caches_progress: Option<crate::progress::ProgressReporter>,
    /// The token of the one progress sequence the client may cancel.
    ///
    /// `window/workDoneProgress/cancel` names a token, and a server that
    /// cancels whatever it happens to be doing has not read the message.
    /// Only `prime_caches` is cancellable — it carries a `CancellationToken`
    /// and restarts itself — so there is one slot rather than a map.
    pub(crate) cancellable_progress: Option<lsp_types::ProgressToken>,
    /// Shared cancellation token for prime_caches.
    /// (`global_state.rs:355 trigger_cancellation()`), which causes
    /// in-progress prime_caches to bail early. A new token is created
    /// when prime is re-queued.
    pub(crate) prime_caches_cancel: hir::CancellationToken,

    /// Whether the workspace has been fully loaded (scan + index complete).
    /// has completed its initial load and is ready for feature requests.
    /// Set to `true` after the first `Task::WorkspaceScan` completes.
    pub(crate) workspace_loaded: bool,

    /// Whether a shutdown has been requested.
    ///
    /// The loop keeps running after `shutdown` and answers anything that
    /// still arrives with `InvalidRequest`, until `exit`.
    pub(crate) shutdown_requested: bool,

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

impl GlobalState {
    pub(crate) fn new(
        sender: Sender<Message>,
        task_sender: crossbeam_channel::Sender<crate::main_loop::Task>,
    ) -> Self {
        Self {
            pending_change: ide_db::apply_change::Change::new(),
            mem_docs: crate::mem_docs::MemDocs::default(),
            analysis_host: ide::analysis::AnalysisHost::new(),
            vfs: vfs::Vfs::default(),
            include_graph: hir::include_graph::IncludeGraph::new(),
            local_root: base_db::SourceRoot::new_local(base_db::FileSet::default()),
            library_root: base_db::SourceRoot::new_library(base_db::FileSet::default()),
            config: crate::config::SailLspConfig::default(),
            workspace_folders: std::collections::HashSet::new(),
            supports_progress: false,
            client_caps: crate::lsp::capabilities::ClientCapabilities::default(),
            req_queue: ReqQueue::default(),
            registered_file_watchers: false,
            discovery_watch_files: Vec::new(),
            loader: None,
            loader_receiver: None,
            pending_workspace: None,
            workspace_load_version: 0,
            loading_project_config: None,
            loading_project_inputs: Default::default(),
            project_error: None,
            config_errors: Vec::new(),
            project_manifests: Vec::new(),
            loading_module_graph: None,
            loading_source_project: None,
            task_pool: crate::task_pool::TaskPool::new_with_threads(
                task_sender,
                crate::task_pool::TaskPool::<crate::main_loop::Task>::default_threads(),
            ),
            sender,
            last_status: None,
            diagnostics: crate::diagnostics::DiagnosticCollection::default(),
            semantic_tokens_cache: crate::lsp::semantic_tokens::SemanticTokensCache::new(),
            fetch_workspaces_queue: crate::op_queue::OpQueue::default(),
            prime_caches_queue: crate::op_queue::OpQueue::default(),
            last_gc_revision: 0,
            prime_caches_progress: None,
            cancellable_progress: None,
            prime_caches_cancel: hir::CancellationToken::new(),
            workspace_loaded: false,
            shutdown_requested: false,
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
        let tmp = crate::sail_stdlib::cache_dir();
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
    pub(crate) fn respond(&mut self, response: Response) {
        if let Some((method, started)) = self.req_queue.incoming.complete(&response.id) {
            let took = started.elapsed();
            if took > std::time::Duration::from_millis(200) {
                log::info!("{method} took {took:?}");
            }
        }
        self.sender.send(Message::Response(response)).ok();
    }

    /// Send a server→client request (e.g., refresh semantic tokens).
    /// like `workspace/semanticTokens/refresh` where the server asks
    /// the client to re-pull data.
    pub(crate) fn send_request<R: lsp_types::request::Request>(
        &mut self,
        params: R::Params,
        handler: ReqHandler,
    ) {
        let request = self.req_queue.outgoing.register(R::METHOD.to_owned(), params, handler);
        self.sender.send(Message::Request(request)).ok();
    }

    /// Hand a response from the client to whoever asked for it.
    pub(crate) fn complete_request(&mut self, response: lsp_server::Response) {
        let Some(handler) = self.req_queue.outgoing.complete(response.id.clone()) else {
            log::warn!("received a response for a request we did not send: {:?}", response.id);
            return;
        };
        handler(self, response);
    }

    /// A progress token the client has agreed to.
    ///
    /// The protocol requires `window/workDoneProgress/create` before a
    /// server-initiated `$/progress`; without it a client may drop every
    /// report, which is what happened to the workspace-load progress bar.
    pub(crate) fn progress_token(&mut self, title: &str) -> lsp_types::ProgressToken {
        let token =
            lsp_types::ProgressToken::String(format!("sail-lsp/{}", title.replace(' ', "-")));
        self.send_request::<lsp_types::request::WorkDoneProgressCreate>(
            lsp_types::WorkDoneProgressCreateParams { token: token.clone() },
            |_state, response| {
                if let Some(err) = response.error {
                    log::warn!("the client refused a progress token: {}", err.message);
                }
            },
        );
        token
    }

    /// Note that the client asked us for something.
    pub(crate) fn register_request(&mut self, request: &lsp_server::Request) {
        self.req_queue
            .incoming
            .register(request.id.clone(), (request.method.clone(), std::time::Instant::now()));
    }

    /// The client no longer wants an answer to `id`.
    ///
    /// Salsa's cancellation only fires when a new revision lands, so a
    /// request that is merely slow was never cancelled at all — the client
    /// waited for an answer it had already given up on.
    pub(crate) fn cancel(&mut self, id: lsp_server::RequestId) {
        if let Some(response) = self.req_queue.incoming.cancel(id) {
            self.respond(response);
        }
    }

    /// Whether the client has cancelled this request.
    pub(crate) fn is_completed(&self, id: &lsp_server::RequestId) -> bool {
        self.req_queue.incoming.is_completed(id)
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

    /// Queue new contents for a file, from wherever they came from.
    ///
    /// One method, because there is one rule: durability follows the file's
    /// source root and `FileChange::apply` derives it. There used to be two —
    /// `sync_to_salsa` naming LOW because an editor buffer changes on every
    /// keystroke, `sync_disk_file` naming HIGH because a file on disk does
    /// not — and the workspace scan went through the second, so every
    /// workspace `.sail` file became a HIGH-durability salsa input. Changing
    /// one bumps the high-durability revision and invalidates everything,
    /// and a `git checkout` under the editor changes many.
    /// Text lands in the VFS, and reaches salsa when changes are drained.
    ///
    /// rust-analyzer has one store for file contents and one drain into the
    /// database; this server had the VFS as a path/URL interner only, with
    /// text going straight into a pending change, so `Vfs::set_file_contents`
    /// and `Vfs::take_changes` — dedup, change kinds, the lot — had no caller
    /// outside their own tests. Two representations of "what this file says",
    /// and the loader could only speak to one of them.
    pub(crate) fn sync_to_salsa(&mut self, uri: &Url, text: &str) {
        let file_id = self.vfs.file_id_for_url(uri);
        let path = self.vfs.file_path(file_id).clone();
        self.vfs.set_file_contents(path, Some(text.as_bytes().to_vec()));
    }

    /// Move everything the VFS has been told into the pending change.
    ///
    /// The single point where "the file store changed" becomes "salsa is
    /// going to be written". A delete reads as empty text, which is what the
    /// rest of the server already understands as "nothing here".
    fn drain_vfs_changes(&mut self) {
        if !self.vfs.has_changes() {
            return;
        }
        for (file_id, changed) in self.vfs.take_changes() {
            let text = match &changed.change {
                vfs::Change::Create(bytes, _) | vfs::Change::Modify(bytes, _) => {
                    String::from_utf8_lossy(bytes).into_owned()
                }
                vfs::Change::Delete => String::new(),
            };
            let db = self.analysis_host.raw_database();
            if base_db::preprocess::WorkspaceSourceProject::try_get(db).is_some() {
                if let Some(old) = db.files().file_text(file_id) {
                    // This cheap trigger only schedules discovery. The CST
                    // event query decides actual include semantics.
                    let includes = |s: &str| {
                        s.lines()
                            .map(str::trim)
                            .filter(|l| l.starts_with("$include"))
                            .map(str::to_owned)
                            .collect::<Vec<_>>()
                    };
                    if includes(old.text(db)) != includes(&text) {
                        self.fetch_workspaces_queue.request_op("include paths changed".into(), ());
                    }
                }
            }
            self.pending_change.change_file(file_id, text);
        }
    }

    /// Queue contents that came from **disk**, which an open document wins.
    ///
    /// rust-analyzer's rule, in its own words at `main_loop.rs:930`: "if the
    /// file is in mem docs, it's managed by the client via notifications so
    /// only set it if it's not in there". Every path that reads bytes off
    /// the filesystem goes through here, because the rule was written at
    /// exactly one of the four such paths and the other three overwrote
    /// unsaved edits in silence — measured, not argued: with a buffer one
    /// edit ahead of disk, creating any `.sail` file in the workspace
    /// triggers a rescan and the edit's two diagnostics disappear.
    ///
    /// The client's own text does *not* come through here — `did_open` and
    /// `did_change` call [`Self::sync_to_salsa`], which is what makes the
    /// buffer authoritative in the first place.
    pub(crate) fn sync_disk_text(&mut self, uri: &Url, text: &str) {
        if self.mem_docs.contains(uri) {
            return;
        }
        self.sync_to_salsa(uri, text);
    }

    /// What the file says on disk right now, for a document being closed.
    ///
    /// A closed buffer stops being authoritative, so the text salsa holds
    /// has to go back to the file's. This is rust-analyzer's
    /// `loader.handle.invalidate` (`notification.rs:148`), and the loader is
    /// there to be asked now whether or not it watches — it was created only
    /// for clients that register no watchers, so on VS Code this had to be a
    /// bare `std::fs::read`. That read is the fallback for a document with
    /// no workspace behind it at all. A file that is gone reads as empty,
    /// which is what `handle_vfs_msg` already does for a delete.
    pub(crate) fn restore_disk_text(&mut self, uri: &Url) {
        let Ok(path) = uri.to_file_path() else { return };
        let Ok(abs) = paths::AbsPathBuf::try_from(path.to_string_lossy().as_ref()) else { return };
        let bytes = match &mut self.loader {
            Some(loader) => {
                loader.invalidate(abs.clone());
                // A loader that cannot read it is not a reason to guess:
                // the file is on disk either way.
                loader.load_sync(&abs).or_else(|| std::fs::read(&path).ok())
            }
            None => std::fs::read(&path).ok(),
        };
        let text = bytes.and_then(|b| String::from_utf8(b).ok()).unwrap_or_default();
        self.sync_to_salsa(uri, &text);
    }

    /// Tell the database which files are the standard library and which are
    /// the user's. Every later text write reads its durability off this.
    pub(crate) fn sync_source_roots(&mut self) {
        self.pending_change.set_roots(vec![self.local_root.clone(), self.library_root.clone()]);
    }

    /// Apply all buffered VFS changes to salsa in a single batch.
    /// Called once per main_loop iteration before dispatching requests.
    /// Returns true if any changes were applied.
    /// Triggers cancellation BEFORE applying changes so in-flight
    /// queries on the old revision will unwind via `salsa::Cancelled`.
    pub(crate) fn process_changes(&mut self) -> bool {
        self.drain_vfs_changes();
        if !self.pending_change.is_empty() {
            // Cancel in-progress prime_caches on edit.
            self.prime_caches_cancel.cancel();
            self.prime_caches_cancel = hir::CancellationToken::new();
            // NOTE: Do NOT re-queue prime_caches on every edit.
            // Salsa handles incremental updates automatically — editing a
            // file invalidates only the affected queries. Re-priming all
            // 158 files on every keystroke blocks the main thread 0.2-0.6s.
            // Prime is only needed after workspace scan (initial load).
        }
        let change = std::mem::take(&mut self.pending_change);
        let changed = self.analysis_host.raw_database_mut().apply_change(change);
        self.refresh_preprocessed_include_graph();
        changed
    }

    /// Ensure `WorkspaceFiles` lists all current files in semantic order.
    /// The incremental declaration index and inference projections depend on
    /// this singleton through Salsa. Called on scan completion and file
    /// add/remove events; content edits flow through per-file queries.
    pub(crate) fn ensure_workspace_files(&mut self, file_ids: Vec<base_db::FileId>) {
        self.drain_vfs_changes();
        self.pending_change.set_workspace_files(file_ids);
        let change = std::mem::take(&mut self.pending_change);
        self.analysis_host.raw_database_mut().apply_change(change);
        self.refresh_preprocessed_include_graph();
    }

    fn refresh_preprocessed_include_graph(&mut self) {
        let db = self.analysis_host.raw_database();
        if let Some(project) = base_db::preprocess::WorkspaceSourceProject::try_get(db) {
            let mut graph = hir::include_graph::IncludeGraph::new();
            for &(from, to) in &hir::preprocess::workspace_plan(db, project).edges {
                graph.add_edge(from, to);
            }
            self.include_graph = graph;
        }
    }

    /// Create Analysis snapshot from AnalysisHost + VFS.
    /// Includes the IncludeGraph for scope-aware analysis.
    pub(crate) fn analysis_snapshot(&mut self) -> Analysis {
        // A snapshot must not miss a queued edit — including one still
        // sitting in the file store.
        self.drain_vfs_changes();
        let change = std::mem::take(&mut self.pending_change);
        self.analysis_host.raw_database_mut().apply_change(change);
        self.refresh_preprocessed_include_graph();
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
            semantic_tokens_cache: self.semantic_tokens_cache.clone(),
            include_graph: Arc::new(self.include_graph.clone()),
            client_caps: self.client_caps.clone(),
            last_format_options: self.last_format_options.clone(),
            library_files: Arc::new(self.library_root.clone()),
            project_error: self.project_error.as_deref().map(Arc::from),
            document_versions: self
                .mem_docs
                .iter()
                .filter_map(|uri| self.mem_docs.get(uri).map(|doc| (uri.clone(), doc.version)))
                .collect(),
        }
    }

    /// Send a notification to the client.
    pub(crate) fn send_notification<N: lsp_types::notification::Notification>(
        &self,
        params: N::Params,
    ) {
        let notif = Notification::new(N::METHOD.to_string(), params);
        let _ = self.sender.send(Message::Notification(notif));
    }

    /// Log a message to the client.
    pub(crate) fn log_message(&self, typ: MessageType, msg: impl Into<String>) {
        self.send_notification::<lsp_types::notification::LogMessage>(LogMessageParams {
            typ,
            message: msg.into(),
        });
    }

    // No `all_salsa_files` and no `salsa_file`. Both read the store
    // *around* an `Analysis` snapshot — which is what a feature must not do,
    // because a snapshot is how a cancelled request stops reading a database
    // being written — and both said so in their own doc comments ("prefer
    // `state.snapshot().file(uri)`", "most handlers now use
    // `analysis.all_salsa_files()`") while having no caller at all. A second
    // door with a sign on it pointing at the first door is still a second
    // door.
}

/// Immutable snapshot of server state passed to request handlers.
/// Contains everything a handler needs to process a request without
/// mutating shared state. Clone is cheap (salsa Arc + index Arc).
#[derive(Clone)]
pub(crate) struct GlobalStateSnapshot {
    pub(crate) document_versions: std::collections::HashMap<Url, i32>,
    pub(crate) project_error: Option<Arc<str>>,
    /// Immutable Analysis snapshot (salsa db + Files).
    pub analysis: Analysis,
    /// Server configuration.
    pub config: SailLspConfig,
    /// Per-file semantic token cache (shared via Arc<Mutex>).
    pub semantic_tokens_cache: crate::lsp::semantic_tokens::SemanticTokensCache,
    /// Include dependency graph for cross-file diagnostic propagation.
    /// Sail-specific: explicit $include graph for cross-file diagnostics.
    pub include_graph: Arc<hir::include_graph::IncludeGraph>,
    /// Client capabilities for feature detection.
    pub client_caps: crate::lsp::capabilities::ClientCapabilities,
    /// Cached formatting options from the last `textDocument/formatting`
    /// request. Shared via Arc<Mutex> with GlobalState.
    pub last_format_options: std::sync::Arc<std::sync::Mutex<ide_db::ide_types::FormatOptions>>,
    /// Files belonging to `$SAIL_DIR` — the bundled library.
    ///
    /// rust-analyzer will not publish diagnostics for a library file, and
    /// says why: "theoretically these should never have errors, we have
    /// quite a few false positives particularly in the stdlib, and those
    /// diagnostics would stay around forever if we emitted them here"
    /// (`main_loop.rs`). The classification was being computed during the
    /// workspace scan and then left on `GlobalState`, where no handler could
    /// reach it.
    pub library_files: Arc<base_db::SourceRoot>,
}

impl GlobalStateSnapshot {
    /// Whether `file_id` is part of the bundled library rather than the
    /// user's own sources.
    pub(crate) fn is_library(&self, file_id: base_db::FileId) -> bool {
        self.library_files.contains(file_id)
    }
}
