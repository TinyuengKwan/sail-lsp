//! Synchronous LSP backend using `lsp-server`.
//! Contains the `Task` enum and the main
//! event loop. `GlobalState` base methods are in `global_state.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use lsp_server::{Connection, Message, Notification, Request};
use lsp_types::*;

use crate::global_state::GlobalState;

/// A unit of background work that has completed.
/// unified enum for ALL background results (responses + tasks).
pub(crate) enum Task {
    /// Workspace scan discovered files on disk.
    WorkspaceScan { files: Vec<(lsp_types::Url, PathBuf, String)> },
    /// Async request handler completed — response ready to send.
    Response(lsp_server::Response),
    /// Async request handler was cancelled and should be retried.
    Retry(lsp_server::Request),
    /// Prime caches progress/completion.
    PrimeCaches(PrimeCachesProgress),
    /// Async diagnostics computation completed for a batch of files.
    /// DiagnosticsTaskKind carries `(generation, Vec<(FileId, diags)>)`.
    Diagnostics(DiagnosticsTaskKind),
}

/// Progress for prime_caches background task.
#[derive(Debug)]
pub(crate) enum PrimeCachesProgress {
    Begin,
    End { cancelled: bool },
}

pub(crate) use crate::diagnostics::NativeDiagnosticsFetchKind;

/// Wraps a batch of diagnostics with generation + kind.
/// ```text
/// pub(crate) enum DiagnosticsTaskKind {
///     Syntax(DiagnosticsGeneration, Vec<(FileId, Vec<Diagnostic>)>),
///     Semantic(DiagnosticsGeneration, Vec<(FileId, Vec<Diagnostic>)>),
/// }
/// ```
#[derive(Debug)]
pub(crate) enum DiagnosticsTaskKind {
    Syntax(
        crate::diagnostics::DiagnosticsGeneration,
        Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>,
    ),
    Semantic(
        crate::diagnostics::DiagnosticsGeneration,
        Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>,
    ),
}

impl GlobalState {
    /// receive proactive diagnostic pushes. Closed files get diagnostics
    /// on-demand via `textDocument/diagnostic` pull requests.
    /// Get files that should receive push diagnostics.
    /// Also includes open files that $include any subscribed file.
    /// When `types.sail` is edited, `main.sail` (which $includes it
    /// and is also open) should also get refreshed diagnostics.
    fn subscribed_files(&self) -> Vec<(Url, base_db::FileId)> {
        let open: Vec<(Url, base_db::FileId)> = self
            .mem_docs
            .iter()
            .filter_map(|url| {
                let fid = self.vfs.lookup_file_id_by_url(url)?;
                Some((url.clone(), fid))
            })
            .collect();

        let open_fids: std::collections::HashSet<base_db::FileId> =
            open.iter().map(|(_, fid)| *fid).collect();

        let mut result = open.clone();
        for (_, fid) in &open {
            for &dep_fid in self.include_graph.included_by(*fid) {
                if open_fids.contains(&dep_fid) {
                    continue; // already in result
                }
                // Only add if the dependent file is also open
                if let Some(dep_url) = self.vfs.lookup_url(dep_fid) {
                    if self.mem_docs.contains(dep_url) {
                        result.push((dep_url.clone(), dep_fid));
                    }
                }
            }
        }
        result
    }

    /// Syntax diagnostics (fast, parse-only) are computed first, then
    /// semantic diagnostics (slow, type inference) — spawned
    ///          to worker threads, results arrive via Task::Diagnostics
    ///        prime_caches.rs:185 `let db = db.clone()`).
    /// Bumps generation ONCE per call. Spawns a single worker task
    /// that computes syntax + semantic diagnostics for all subscribed
    /// files, sending batch results via `Task::Diagnostics(DiagnosticsTaskKind)`.
    #[allow(clippy::print_stderr)]
    fn update_diagnostics(&mut self) {
        let subscribed = self.subscribed_files();
        if subscribed.is_empty() {
            return;
        }

        let generation = self.diagnostics.next_generation();
        let analysis = self.analysis_snapshot();
        let diag_config = self.config.diagnostics.to_ide_config();

        // Spawn single task: syntax first, then semantic.
        //
        // (main_loop.rs:669, 685) — on panic, returns empty diagnostics.
        self.task_pool.spawn_with_sender(move |sender| {
            use std::panic::AssertUnwindSafe;

            // Syntax diagnostics (fast).
            let syntax_diags = std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::diagnostics::fetch_native_diagnostics(
                    &analysis,
                    &subscribed,
                    NativeDiagnosticsFetchKind::Syntax,
                    &diag_config,
                )
            }))
            .unwrap_or_else(|_| {
                eprintln!("[diag] SYNTAX PANIC caught");
                subscribed.iter().map(|(_, fid)| (*fid, Vec::new())).collect()
            });
            let _ = sender
                .send(Task::Diagnostics(DiagnosticsTaskKind::Syntax(generation, syntax_diags)));

            // Semantic diagnostics (slow).
            let semantic_diags = std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::diagnostics::fetch_native_diagnostics(
                    &analysis,
                    &subscribed,
                    NativeDiagnosticsFetchKind::Semantic,
                    &diag_config,
                )
            }))
            .unwrap_or_else(|_| {
                eprintln!("[diag] SEMANTIC PANIC caught");
                subscribed.iter().map(|(_, fid)| (*fid, Vec::new())).collect()
            });
            let _ = sender
                .send(Task::Diagnostics(DiagnosticsTaskKind::Semantic(generation, semantic_diags)));
        });
    }
}

#[allow(clippy::print_stderr)]
pub fn main() {
    // CLI subcommand dispatch before starting the LSP server.
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && !args[1].starts_with('-') {
        let cmd = args[1].as_str();
        let path_arg = args.get(2).map(|s| std::path::Path::new(s.as_str()));
        let result = match cmd {
            "parse" => path_arg
                .ok_or_else(|| "Usage: sail-lsp parse <file>".to_string())
                .and_then(crate::cli::cmd_parse),
            "symbols" => path_arg
                .ok_or_else(|| "Usage: sail-lsp symbols <file>".to_string())
                .and_then(crate::cli::cmd_symbols),
            "check" => path_arg
                .ok_or_else(|| "Usage: sail-lsp check <file>".to_string())
                .and_then(crate::cli::cmd_check),
            "highlight" => path_arg
                .ok_or_else(|| "Usage: sail-lsp highlight <file>".to_string())
                .and_then(crate::cli::cmd_highlight),
            "scip" => path_arg
                .ok_or_else(|| "Usage: sail-lsp scip <dir>".to_string())
                .and_then(crate::cli::cmd_scip),
            "analysis-stats" => {
                let verbose = args.iter().any(|a| a == "--verbose" || a == "-v");
                let dir = args.iter()
                    .filter(|a| !a.starts_with('-') && *a != "analysis-stats")
                    .nth(1)
                    .map(|s| std::path::Path::new(s.as_str()));
                dir.ok_or_else(|| "Usage: sail-lsp analysis-stats [--verbose] <dir>".to_string())
                    .and_then(|d| crate::cli::cmd_analysis_stats(d, verbose))
            }
            _ => {
                // Not a known subcommand — fall through to LSP server.
                // This handles the case where the first arg is a flag
                // like --version.
                Err(String::new())
            }
        };
        match result {
            Ok(()) => return,
            Err(msg) if msg.is_empty() => {} // fall through to LSP
            Err(msg) => {
                eprintln!("Error: {msg}");
                std::process::exit(1);
            }
        }
    }

    // Controlled by SAIL_LOG (filter) and SAIL_TRACE_TREE (hierarchical spans).
    crate::tracing_setup::setup();

    tracing::info!("sail-lsp starting (lsp-server backend)");
    eprintln!("[sail-lsp] starting (lsp-server backend)");

    let (connection, io_threads) = Connection::stdio();

    let server_capabilities =
        serde_json::to_value(crate::lsp::capabilities::server_capabilities()).unwrap();
    let init_params = match connection.initialize(server_capabilities) {
        Ok(params) => params,
        Err(e) => {
            if e.channel_is_disconnected() {
                io_threads.join().ok();
            }
            return;
        }
    };
    let init_params: InitializeParams = serde_json::from_value(init_params).unwrap();

    eprintln!("[sail-lsp] initialized");

    // Background task channel
    let (task_sender, task_receiver) = crossbeam_channel::unbounded();
    let mut state = GlobalState::new(connection.sender.clone(), task_sender);

    // Check client progress capability
    let supports_progress = crate::progress::client_supports_progress(&init_params.capabilities);
    state.supports_progress = supports_progress;
    state.client_caps =
        crate::lsp::capabilities::ClientCapabilities(init_params.capabilities.clone());

    // Workspace scan: discover all .sail files from workspace folders.
    // Initial scan goes through the same fetch_workspaces() path
    // as reloads, ensuring consistent behavior.
    if let Some(folders) = init_params.workspace_folders {
        let mut disk_folders = std::collections::HashSet::new();
        for folder in folders {
            disk_folders.insert(folder.uri);
        }
        state.workspace_folders = disk_folders;
        // Request initial workspace scan via OpQueue.
        state.fetch_workspaces_queue.request_op("startup".into(), ());
        // Force start immediately — on first iteration, should_start_op() will fire.
        let _ = state.fetch_workspaces_queue.should_start_op();
        state.fetch_workspaces("startup".into());
    }

    // Async responses now flow through Task::Response,
    // no separate response channel needed.
    main_loop(&connection, &mut state, &task_receiver);

    io_threads.join().ok();
    eprintln!("[sail-lsp] shutdown");
}

/// Event-driven main loop using `crossbeam_channel::select!`.
/// Two channels (LSP messages + background tasks/responses).
/// Async handler responses flow through Task::Response (unified Task enum).
/// process_changes() called AFTER event handling.
fn main_loop(
    connection: &Connection,
    state: &mut GlobalState,
    task_receiver: &crossbeam_channel::Receiver<Task>,
) {
    loop {
        let was_quiescent = state.is_quiescent();
        let loop_start = std::time::Instant::now();
        // stdx::defer — ensures timing is logged even on early return.
        let _loop_guard = stdx::defer(|| {
            let elapsed = loop_start.elapsed();
            if elapsed > std::time::Duration::from_secs(1) {
                log::warn!("extremely long loop turn: {elapsed:?}");
            }
        });

        crossbeam_channel::select! {
            recv(connection.receiver) -> msg => {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(crossbeam_channel::RecvError) => return,
                };
                match msg {
                    Message::Request(req) => {
                        if connection.handle_shutdown(&req).unwrap_or(true) {
                            state.shutdown_requested = true;
                            return;
                        }
                        // Reject requests after shutdown has been requested.
                        if state.shutdown_requested {
                            state.respond(lsp_server::Response::new_err(
                                req.id,
                                lsp_server::ErrorCode::InvalidRequest as i32,
                                "shutdown already requested".to_owned(),
                            ));
                            continue;
                        }
                        on_request(state, req);
                    }
                    Message::Notification(notif) => {
                        on_notification(state, notif);
                    }
                    Message::Response(_) => {}
                }
            }
            recv(task_receiver) -> task => {
                if let Ok(task) = task {
                    on_task(state, task);
                }
            }
        }

        while let Ok(task) = task_receiver.try_recv() {
            on_task(state, task);
        }

        // Sail: fetch_workspaces_queue completion is equivalent to vfs_done.
        let (state_changed, memdocs_added_or_removed) =
            if !state.fetch_workspaces_queue.op_in_progress() {
                (state.process_changes(), state.mem_docs.take_changes())
            } else {
                (false, false)
            };

        if state.is_quiescent() {
            let became_quiescent = !was_quiescent;

            // Sail-specific: drain dirty files for workspace index update.
            // Inside quiescent block to avoid blocking the event loop
            // while prime_caches worker is running (no salsa contention).
            // Sail-specific: drain dirty files for workspace index update.
            if !state.index_dirty_files.is_empty() {
                let dirty: Vec<Url> = state.index_dirty_files.drain(..).collect();
                let db = state.analysis_host.raw_database();
                let mut index = (*state.workspace_index).clone();
                for url in &dirty {
                    if let Some(fid) = state.vfs.lookup_file_id_by_url(url) {
                        if let Some(ft) = db.files().file_text(fid) {
                            let sf = ide_db::root_database::SalsaFile::new(db, ft);
                            let result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    index.add_file(url, &sf as &dyn ide_db::FileDb);
                                }));
                            if result.is_err() {
                                log::error!("panic re-indexing {url}, skipping");
                            }
                        }
                    }
                }
                state.workspace_index = std::sync::Arc::new(index);
            }

            let project_or_mem_docs_changed =
                became_quiescent || state_changed || memdocs_added_or_removed;

            if became_quiescent {
                state.prime_caches_queue.request_op("became quiescent".to_owned(), ());
            }

            let client_refresh = became_quiescent || state_changed;
            if client_refresh {
                state.send_request::<lsp_types::request::SemanticTokensRefresh>(());
                state.send_request::<lsp_types::request::CodeLensRefresh>(());
                state.send_request::<lsp_types::request::InlayHintRefreshRequest>(());
            }

            if project_or_mem_docs_changed {
                state.update_diagnostics();
            }

            let current_nonce = state.analysis_host.raw_database().nonce();
            if task_receiver.is_empty() && current_nonce != state.last_gc_revision {
                state.analysis_host.trigger_garbage_collection();
                state.last_gc_revision = current_nonce;
            }
        }

        if let Some((cause, ())) = state.fetch_workspaces_queue.should_start_op() {
            state.fetch_workspaces(cause);
        }

        if let Some((_cause, ())) = state.prime_caches_queue.should_start_op() {
            let analysis = state.analysis_snapshot();
            let files = state.analysis_host.raw_database().files().clone();
            let cancel = state.prime_caches_cancel.clone();
            state.task_pool.spawn_with_sender(move |sender| {
                let _ = sender.send(Task::PrimeCaches(PrimeCachesProgress::Begin));
                ide_db::prime_caches::parallel_prime_caches(
                    analysis.db(),
                    &files,
                    2,
                    &|_progress| {},
                    &cancel,
                );
                let _ = sender.send(Task::PrimeCaches(PrimeCachesProgress::End {
                    cancelled: cancel.is_cancelled(),
                }));
            });
        }

        if let Some(diagnostic_changes) = state.diagnostics.take_changes() {
            for file_id in diagnostic_changes {
                if let Some(url) = state.vfs.lookup_url(file_id) {
                    let diagnostics: Vec<lsp_types::Diagnostic> =
                        state.diagnostics.diagnostics_for(file_id).cloned().collect();
                    state.send_notification::<lsp_types::notification::PublishDiagnostics>(
                        lsp_types::PublishDiagnosticsParams {
                            uri: url.clone(),
                            diagnostics,
                            version: None,
                        },
                    );
                }
            }
        }

        state.update_status_or_notify();

        let loop_duration = loop_start.elapsed();
        if loop_duration > std::time::Duration::from_millis(100) && was_quiescent {
            log::warn!("overly long loop turn: {loop_duration:?}");
        }
    }
}

/// Process a completed background task.
fn on_task(state: &mut GlobalState, task: Task) {
    match task {
        Task::WorkspaceScan { files } => {
            let count = files.len();

            // Workspace scan is salsa-only.
            // Only register files as salsa inputs — no File struct, no eager parse.
            // Salsa queries handle all derived data lazily on demand.
            let mut url_to_fid = HashMap::new();
            for (uri, _path, text) in &files {
                // Salsa-only — just set input, no File struct.
                state.sync_disk_file(uri, text);
                let fid = state.vfs.file_id_for_url(uri);
                url_to_fid.insert(uri.clone(), fid);
            }

            // Flush all buffered disk files to salsa before building index.
            state.process_changes();

            // Create WorkspaceFiles salsa input listing all FileText handles.
            // workspace_context(db, ws) depends on this — salsa handles
            // the rest (caching, invalidation) automatically.
            state.ensure_workspace_files();

            // "Building index" progress (include graph + SourceRoots)
            let build_progress = if state.supports_progress {
                Some(crate::progress::ProgressReporter::begin(&state.sender, "Building index"))
            } else {
                None
            };
            if let Some(ref p) = build_progress {
                p.report("resolving includes...", Some(30));
            }

            // Extract include paths and build graph.
            let sail_dir = state.effective_sail_dir.clone();

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

            for (uri, path, _text) in &files {
                if let Some(&from_fid) = url_to_fid.get(uri) {
                    let db = state.analysis_host.raw_database();
                    let include_paths = hir_def::def_query::typed_include_paths(
                        db,
                        db.files().file_text(from_fid).unwrap(),
                    );
                    let parent_dir = path.parent();

                    for inc in include_paths {
                        let resolved = match inc {
                            hir_def::def_query::IncludePath::Relative(ref p) => {
                                parent_dir.map(|d| d.join(p))
                            }
                            hir_def::def_query::IncludePath::Library(ref p) => {
                                sail_dir.as_ref().map(|d| d.join("lib").join(p))
                            }
                        };

                        // O(1) lookup via canonical_map instead of O(n) scan.
                        if let Some(resolved_path) = resolved {
                            let found = resolved_path
                                .canonicalize()
                                .ok()
                                .and_then(|canon| canonical_map.get(&canon).copied());

                            if let Some(target_fid) = found {
                                state.include_graph.add_edge(from_fid, target_fid);
                            } else {
                                // Fallback: suffix matching for unresolved paths
                                let target_name = match inc {
                                    hir_def::def_query::IncludePath::Relative(ref p)
                                    | hir_def::def_query::IncludePath::Library(ref p) => p.clone(),
                                };
                                // Try suffix map first (O(1)), then full path suffix scan
                                let filename =
                                    target_name.rsplit('/').next().unwrap_or(&target_name);
                                let fallback = suffix_map.get(filename).copied().or_else(|| {
                                    url_to_fid
                                        .iter()
                                        .find(|(u, _)| u.path().ends_with(&target_name))
                                        .map(|(_, &fid)| fid)
                                });
                                if let Some(target_fid) = fallback {
                                    state.include_graph.add_edge(from_fid, target_fid);
                                }
                            }
                        }
                    }
                }
            }

            // Classify files into local vs library SourceRoots.
            // Workspace files are local; $SAIL_DIR files are library.
            for (uri, path, _) in &files {
                if let Some(&fid) = url_to_fid.get(uri) {
                    let abs = paths::AbsPathBuf::assert_utf8(path.clone());
                    let vfs_path = base_db::VfsPath::new(abs.clone());
                    // Use AbsPath::strip_prefix to determine if file is under $SAIL_DIR.
                    let is_library = sail_dir.as_ref().map_or(false, |sd| {
                        let sd_abs = paths::AbsPathBuf::assert_utf8(sd.clone());
                        abs.strip_prefix(&sd_abs).is_some()
                    });
                    if is_library {
                        state.library_root.insert(fid, vfs_path);
                    } else {
                        state.local_root.insert(fid, vfs_path);
                    }
                }
            }

            if let Some(p) = build_progress {
                p.end("include graph built");
            }

            let lib_count = state.library_root.len();
            let local_count = state.local_root.len();
            state.log_message(
                MessageType::INFO,
                format!("background scan: {count} files ({local_count} local, {lib_count} library), include graph ({} nodes)",
                    state.include_graph.all_files().len()),
            );

            // "Indexing" progress during diagnostics computation.
            // This is the most time-consuming startup phase — report progress.
            let indexing_progress = if state.supports_progress {
                Some(crate::progress::ProgressReporter::begin(&state.sender, "Indexing"))
            } else {
                None
            };

            // Queue prime_caches via OpQueue instead of running inline.
            //
            // (main_loop.rs:600-617): prime_caches is launched
            // from the idle section after workspace scan completes, not
            // inline in the scan callback. This prevents blocking the
            // main thread during workspace scan processing.
            state.prime_caches_queue.request_op("workspace loaded".into(), ());

            // Deferred diagnostics — don't block workspace scan.
            //
            // (main_loop.rs:619-706): diagnostics are computed
            // asynchronously after workspace is quiescent, not inline in
            // the scan callback. This makes the editor usable immediately.
            //
            // Diagnostics will be computed on-demand when files are opened
            // (via textDocument/diagnostic pull request) or when the client
            // triggers a workspace diagnostic refresh.
            //
            // For files that are already open (in MemDocs), publish empty
            // diagnostics to clear any stale state, then queue a refresh.
            for (url, _fid) in state.vfs.all_url_file_ids() {
                state.send_notification::<lsp_types::notification::PublishDiagnostics>(
                    lsp_types::PublishDiagnosticsParams {
                        uri: url.clone(),
                        diagnostics: Vec::new(),
                        version: None,
                    },
                );
            }
            // Diagnostics will refresh naturally when the quiescent block
            // fires with `became_quiescent = true` after workspace scan.

            if let Some(p) = indexing_progress {
                let total = state.vfs.all_url_file_ids().count();
                p.end(&format!("{total} files indexed"));
            }

            // Build workspace symbol index from all files.
            // This enables O(1) name lookups in handlers instead of O(n) file scans.
            //
            // Wrap add_file in catch_unwind: a parse panic in one file
            // so a parse panic in one file doesn't take down the server.
            {
                let analysis = state.analysis_snapshot();
                let all = analysis.all_salsa_files();
                let mut index = ide_db::workspace_index::SymbolIndex::new();
                for (url, sf) in &all {
                    let url_clone = url.clone();
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        index.add_file(url, sf as &dyn ide_db::FileDb);
                    }));
                    if result.is_err() {
                        log::error!("panic indexing {url_clone}, skipping");
                    }
                }
                state.workspace_index = std::sync::Arc::new(index);
            }
            state.log_message(
                MessageType::INFO,
                format!("workspace index: {} symbols", state.workspace_index.all_entries().count()),
            );

            // Mark workspace as fully loaded.
            state.workspace_loaded = true;

            // "server ready" notification
            state.log_message(MessageType::INFO, "sail-lsp: ready");

            // Mark workspace scan op as completed.
            if state.fetch_workspaces_queue.op_in_progress() {
                state.fetch_workspaces_queue.op_completed(());
            }
        }
        Task::Response(response) => {
            // Async handler completed → forward response to client.
            state.sender.send(Message::Response(response)).ok();
        }
        Task::Retry(req) => {
            // (dispatch.rs:268): request was cancelled, re-dispatch.
            // This happens after process_changes() has applied the new revision,
            // so the handler will see the latest state.
            on_request(state, req);
        }
        Task::PrimeCaches(progress) => {
            match progress {
                PrimeCachesProgress::Begin => {}
                PrimeCachesProgress::End { cancelled } => {
                    state.analysis_host.trigger_garbage_collection();
                    state.prime_caches_queue.op_completed(());
                    if cancelled {
                        state
                            .prime_caches_queue
                            .request_op("restart after cancellation".into(), ());
                    }
                }
            }
        }
        Task::Diagnostics(task_kind) => {
            // Batch of (FileId, diagnostics) stored in DiagnosticCollection.
            // Actual publish happens via take_changes() at end of loop.
            let (kind, generation, diags) = match task_kind {
                DiagnosticsTaskKind::Syntax(gen, diags) => {
                    (NativeDiagnosticsFetchKind::Syntax, gen, diags)
                }
                DiagnosticsTaskKind::Semantic(gen, diags) => {
                    (NativeDiagnosticsFetchKind::Semantic, gen, diags)
                }
            };
            state.diagnostics.set_native_diagnostics(kind, generation, diags);
        }
    }
}

/// Dispatch request via RequestDispatcher.
fn on_request(state: &mut GlobalState, req: Request) {
    use crate::dispatch::RequestDispatcher;
    use crate::handlers::request as handlers;

    RequestDispatcher { req: Some(req), global_state: state }
        .on_sync::<request::DocumentSymbolRequest>(handlers::handle_document_symbol)
        .on_sync::<request::SelectionRangeRequest>(handlers::handle_selection_range)
        .on_sync::<request::Formatting>(handlers::handle_formatting)
        .on_sync::<request::SemanticTokensRangeRequest>(handlers::handle_semantic_tokens_range)
        .on_sync::<request::DocumentHighlightRequest>(handlers::handle_document_highlight)
        .on_sync::<request::SemanticTokensFullRequest>(handlers::handle_semantic_tokens_full)
        .on_sync::<request::DocumentDiagnosticRequest>(handlers::handle_document_diagnostic)
        .on_sync::<request::RangeFormatting>(handlers::handle_range_formatting)
        .on_sync::<request::LinkedEditingRange>(handlers::handle_linked_editing_range)
        .on_sync::<request::DocumentLinkRequest>(handlers::handle_document_link)
        .on_sync::<request::DocumentLinkResolve>(handlers::handle_document_link_resolve)
        .on_sync::<request::PrepareRenameRequest>(handlers::handle_prepare_rename)
        .on_sync::<request::SemanticTokensFullDeltaRequest>(
            handlers::handle_semantic_tokens_full_delta,
        )
        .on_sync::<request::FoldingRangeRequest>(handlers::handle_folding_range)
        .on_sync::<request::CodeLensResolve>(handlers::handle_code_lens_resolve)
        .on_sync::<request::OnTypeFormatting>(handlers::handle_on_type_formatting)
        .on_sync::<request::InlayHintResolveRequest>(handlers::handle_inlay_hint_resolve)
        .on_sync::<request::CodeActionRequest>(handlers::handle_code_action)
        .on_sync::<request::CodeActionResolveRequest>(handlers::handle_code_action_resolve)
        // ALLOW_RETRYING = true: latency-sensitive, client will re-request on cancel
        // ALLOW_RETRYING = false: user-triggered, show error on cancel
        .on::<true, request::Completion>(handlers::handle_completion)
        .on::<true, request::ResolveCompletionItem>(handlers::handle_resolve_completion_item)
        .on::<true, request::InlayHintRequest>(handlers::handle_inlay_hint)
        .on::<true, request::SignatureHelpRequest>(handlers::handle_signature_help)
        .on::<true, request::HoverRequest>(handlers::handle_hover)
        .on::<false, request::GotoDefinition>(handlers::handle_goto_definition)
        .on::<false, request::GotoDeclaration>(handlers::handle_goto_declaration)
        .on::<false, request::References>(handlers::handle_references)
        .on::<false, request::Rename>(handlers::handle_rename)
        .on::<false, request::CodeLensRequest>(handlers::handle_code_lens)
        .on::<false, request::CallHierarchyPrepare>(handlers::handle_call_hierarchy_prepare)
        .on::<false, request::CallHierarchyIncomingCalls>(handlers::handle_call_hierarchy_incoming)
        .on::<false, request::CallHierarchyOutgoingCalls>(handlers::handle_call_hierarchy_outgoing)
        .on::<false, request::WorkspaceSymbolRequest>(handlers::handle_workspace_symbol)
        .on::<false, request::GotoImplementation>(handlers::handle_goto_implementation)
        .on::<false, request::WillRenameFiles>(handlers::handle_will_rename_files)
        .on::<false, request::GotoTypeDefinition>(handlers::handle_goto_type_definition)
        .on::<false, request::TypeHierarchyPrepare>(handlers::handle_type_hierarchy_prepare)
        .on::<false, request::TypeHierarchySupertypes>(handlers::handle_type_hierarchy_supertypes)
        .on::<false, request::TypeHierarchySubtypes>(handlers::handle_type_hierarchy_subtypes)
        .on_sync::<crate::lsp_ext::ViewSyntaxTree>(handlers::handle_view_syntax_tree)
        .on_sync::<crate::lsp_ext::ViewHir>(handlers::handle_view_hir)
        .on_sync::<crate::lsp_ext::ViewItemTree>(handlers::handle_view_item_tree)
        .on::<false, crate::lsp_ext::ExpandInclude>(handlers::handle_expand_include)
        .on::<false, crate::lsp_ext::EffectAnnotations>(handlers::handle_effect_annotations)
        .on_sync::<crate::lsp_ext::SailLspStatus>(handlers::handle_sail_lsp_status)
        .on_sync::<crate::lsp_ext::ViewIncludeGraph>(handlers::handle_view_include_graph)
        .on::<false, crate::lsp_ext::Ssr>(handlers::handle_ssr)
        .finish();
}

/// Dispatch notification via NotificationDispatcher.
fn on_notification(state: &mut GlobalState, notif: Notification) {
    use crate::dispatch::NotificationDispatcher;
    use crate::handlers::notification as handlers;

    NotificationDispatcher { not: Some(notif), global_state: state }
        .on_sync_mut::<crate::lsp_ext::CancelRequest>(handlers::handle_cancel)
        .on_sync_mut::<lsp_types::notification::DidOpenTextDocument>(
            handlers::handle_did_open_text_document,
        )
        .on_sync_mut::<lsp_types::notification::DidChangeTextDocument>(
            handlers::handle_did_change_text_document,
        )
        .on_sync_mut::<lsp_types::notification::DidCloseTextDocument>(
            handlers::handle_did_close_text_document,
        )
        .on_sync_mut::<lsp_types::notification::DidSaveTextDocument>(
            handlers::handle_did_save_text_document,
        )
        .on_sync_mut::<lsp_types::notification::DidChangeWatchedFiles>(
            handlers::handle_did_change_watched_files,
        )
        .on_sync_mut::<lsp_types::notification::DidChangeWorkspaceFolders>(
            handlers::handle_did_change_workspace_folders,
        )
        .on_sync_mut::<lsp_types::notification::DidChangeConfiguration>(
            handlers::handle_did_change_configuration,
        )
        .on_sync_mut::<crate::lsp_ext::ReloadWorkspace>(handlers::handle_reload_workspace)
        .finish();
}
