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
    /// Workspace scan discovered files on disk — which ones, in the order
    /// the project declares them. Not what they say: the loader reads them.
    WorkspaceScan(crate::reload::WorkspaceScan),
    /// Async request handler completed — response ready to send.
    Response(lsp_server::Response),
    /// Async request handler was cancelled and should be retried.
    Retry(lsp_server::Request),
    /// Prime caches progress/completion.
    PrimeCaches(PrimeCachesProgress),
    /// Async diagnostics computation completed for a batch of files.
    /// DiagnosticsTaskKind carries `(generation, Vec<(FileId, diags)>)`.
    Diagnostics(DiagnosticsTaskKind),
    /// What `sail --just-check` said, by the path it named.
    Flycheck(crate::flycheck::FlycheckResult),
}

/// Progress for prime_caches background task.
#[derive(Debug)]
pub(crate) enum PrimeCachesProgress {
    Begin,
    Report(ide_db::prime_caches::ParallelPrimeCachesProgress),
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
        self.task_pool.spawn_with_sender(
            stdx::thread::ThreadIntent::LatencySensitive,
            move |sender| {
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
                let _ = sender.send(Task::Diagnostics(DiagnosticsTaskKind::Semantic(
                    generation,
                    semantic_diags,
                )));
            },
        );
    }

    /// Everything about the workspace that needs the files to have been
    /// *read*: the ordered `WorkspaceFiles` input, the include graph, cache
    /// priming, and telling the client the server is ready.
    ///
    /// The scan produces paths; the loader produces contents. Splitting
    /// here is what lets the second be asynchronous — rust-analyzer waits
    /// on `vfs::loader::Message::Progress { Finished }` the same way. It
    /// matters more here than there: `WorkspaceFiles` is *the* environment
    /// every cross-file query resolves against, so assembling it before the
    /// text lands would mean a workspace that briefly knows nothing — and a
    /// transient flood of "not found" is the false positive this project
    /// pays most for.
    pub(crate) fn finish_workspace_load(&mut self, files: &[(Url, PathBuf)], count: usize) {
        let module_graph = self.loading_module_graph.take();
        let source_project = self.loading_source_project.take();
        let inputs = std::mem::take(&mut self.loading_project_inputs);
        if !inputs.matches_disk() {
            self.fetch_workspaces_queue.request_op("manifest changed during loading".into(), ());
        }
        if let Some((config, folders, revision)) = self.loading_project_config.take() {
            if revision != self.fetch_workspaces_queue.revision()
                || config != self.config.workspace
                || folders != self.workspace_folders
            {
                if self.fetch_workspaces_queue.op_in_progress() {
                    self.fetch_workspaces_queue.op_completed(());
                }
                return;
            }
        }
        self.project_error = None;
        let url_to_fid: HashMap<Url, base_db::FileId> = files
            .iter()
            .filter_map(|(uri, _)| self.vfs.lookup_file_id_by_url(uri).map(|f| (uri.clone(), f)))
            .collect();
        // Create WorkspaceFiles salsa input listing all FileText handles.
        // The declaration directory depends on this — salsa handles
        // the rest (caching, invalidation) automatically. Keep the scan's
        // semantic order rather than reconstructing it from DashMap or
        // FileId order: an early didOpen may allocate a project FileId
        // before the library is discovered.
        let workspace_file_ids =
            files.iter().filter_map(|(uri, _)| url_to_fid.get(uri).copied()).collect();
        if let Some(graph) = module_graph {
            self.pending_change.set_module_graph(graph);
        }
        if let Some(project) = source_project {
            self.pending_change.set_source_project(project);
        }
        self.ensure_workspace_files(workspace_file_ids);

        // "Building index" progress (include graph + SourceRoots)
        let build_progress = self.supports_progress.then(|| {
            let token = self.progress_token("Building index");
            crate::progress::ProgressReporter::begin(&self.sender, token, "Building index", false)
        });
        if let Some(ref p) = build_progress {
            p.report("resolving includes...", Some(30));
        }

        // Extract include paths and build graph.
        // One builder, in `ide`, because an `Analysis` without a graph
        // answers differently: `$include` cycles are reported only by a
        // holder of the whole graph, and no probe held one.
        self.include_graph = self.analysis_host.include_graph(
            files,
            &url_to_fid,
            self.effective_sail_dir.as_ref().map(|d| d.join("lib")).as_deref(),
        );

        if let Some(p) = build_progress {
            p.end("include graph built");
        }

        let lib_count = self.library_root.len();
        let local_count = self.local_root.len();
        self.log_message(
            MessageType::INFO,
            format!("background scan: {count} files ({local_count} local, {lib_count} library), include graph ({} nodes)",
                self.include_graph.all_files().len()),
        );

        // "Indexing" progress during diagnostics computation.
        // This is the most time-consuming startup phase — report progress.
        let indexing_progress = self.supports_progress.then(|| {
            let token = self.progress_token("Indexing");
            crate::progress::ProgressReporter::begin(&self.sender, token, "Indexing", false)
        });

        // Queue prime_caches via OpQueue instead of running inline.
        //
        // (main_loop.rs:600-617): prime_caches is launched
        // from the idle section after workspace scan completes, not
        // inline in the scan callback. This prevents blocking the
        // main thread during workspace scan processing.
        self.prime_caches_queue.request_op("workspace loaded".into(), ());

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
        // Nothing is cleared here. This used to publish an empty list
        // for *every* file in the workspace "to clear any stale state",
        // without telling `DiagnosticCollection` — so the collection
        // still held the previous findings, the wave that followed
        // computed the same ones, `set_native_diagnostics` compared them
        // equal and recorded no change, and nothing was republished. The
        // client kept the empty list. A rescan (any `.sail` file
        // created, any project file saved) therefore *erased* the
        // problems panel until the file was edited again.
        //
        // rust-analyzer has no such sweep: a reload recomputes, and each
        // file's diagnostics are replaced when its wave lands. The
        // quiescent block below asks for that wave with
        // `became_quiescent`.

        if let Some(p) = indexing_progress {
            let total = self.vfs.all_url_file_ids().count();
            p.end(&format!("{total} files indexed"));
        }

        // Mark workspace as fully loaded.
        self.workspace_loaded = true;

        // "server ready" notification
        self.log_message(MessageType::INFO, "sail-lsp: ready");

        // Mark workspace scan op as completed.
        if self.fetch_workspaces_queue.op_in_progress() {
            self.fetch_workspaces_queue.op_completed(());
        }
    }
    /// Ask upstream Sail what it thinks, if the user turned it on.
    ///
    /// rust-analyzer runs this on save, and so does this: `--just-check`
    /// re-reads whole files from disk, so a run against a buffer the editor
    /// has not written yet would report the previous save.
    pub(crate) fn request_flycheck(&mut self) {
        if !self.config.flycheck.enable {
            return;
        }
        let files: Vec<std::path::PathBuf> =
            self.subscribed_files().iter().filter_map(|(url, _)| url.to_file_path().ok()).collect();
        if files.is_empty() {
            return;
        }
        // Upstream prints the path it was given; relative ones resolve
        // against the first workspace folder.
        let cwd = self
            .workspace_folders
            .iter()
            .next()
            .and_then(|u| u.to_file_path().ok())
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        let command = self.config.flycheck.command.clone();
        let extra_args = self.config.flycheck.extra_args.clone();
        self.task_pool.spawn_with_sender(stdx::thread::ThreadIntent::Worker, move |sender| {
            let found = crate::flycheck::run(&command, &extra_args, &files, &cwd);
            let _ = sender.send(Task::Flycheck(found));
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
                let dir = args
                    .iter()
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
    run_server(connection);
    io_threads.join().ok();
    eprintln!("[sail-lsp] shutdown");
}

/// The server itself, on a connection it is handed.
///
/// Split from [`main`] so something other than a terminal can be the client:
/// `server_check` drives this over `Connection::memory()`. Every probe in
/// this repo before it entered at `Analysis`, and the loop — where an
/// eviction cancelled the diagnostics wave, where a rescan replaced open
/// buffers, where a scan told the client its problems panel was empty — is
/// below none of them and above all of them. rust-analyzer splits at the
/// same seam (`rust_analyzer::main_loop(config, connection)`).
#[allow(clippy::print_stderr)]
pub fn run_server(connection: Connection) {
    // The client's capabilities first, then ours — rust-analyzer's
    // `initialize_start` / `initialize_finish`. `initialize()` does both at
    // once and hands the server's capabilities over before it has seen the
    // client's, so nothing the server advertises could depend on what the
    // client said it understands.
    let (initialize_id, init_params) = match connection.initialize_start() {
        Ok(it) => it,
        Err(_) => return,
    };
    let init_params: InitializeParams = serde_json::from_value(init_params).unwrap();
    let client_caps =
        crate::lsp::capabilities::ClientCapabilities(init_params.capabilities.clone());
    let initialize_result = lsp_types::InitializeResult {
        capabilities: crate::lsp::capabilities::server_capabilities(&client_caps),
        server_info: Some(lsp_types::ServerInfo {
            name: "sail-lsp".to_owned(),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
        }),
    };
    if connection
        .initialize_finish(initialize_id, serde_json::to_value(initialize_result).unwrap())
        .is_err()
    {
        return;
    }

    eprintln!("[sail-lsp] initialized");

    // Background task channel
    let (task_sender, task_receiver) = crossbeam_channel::unbounded();
    let mut state = GlobalState::new(connection.sender.clone(), task_sender);

    state.client_caps = client_caps;
    // Asked of the capabilities the client just sent, which is why it reads
    // them after they are stored.
    let supports_progress = state.client_caps.work_done_progress_support();
    state.supports_progress = supports_progress;

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
                    // `shutdown` does not end the loop. The protocol says
                    // the server keeps running until `exit`, answering
                    // anything that still arrives with `InvalidRequest` —
                    // which is what `shutdown_requested` is for, and it had
                    // no reader: `Connection::handle_shutdown` was called
                    // instead and the loop returned on the spot, so between
                    // the two messages a client got no answer at all, to
                    // anything. `on_sync_mut` — rust-analyzer's dispatcher
                    // method for a request that mutates state, and the one
                    // RA uses for exactly this — had no caller for the same
                    // reason.
                    Message::Request(req) => {
                        state.register_request(&req);
                        on_request(state, req);
                    }
                    Message::Notification(notif) if notif.method == "exit" => {
                        if !state.shutdown_requested {
                            log::warn!("exit without a preceding shutdown");
                        }
                        return;
                    }
                    Message::Notification(notif) => {
                        on_notification(state, notif);
                    }
                    Message::Response(resp) => state.complete_request(resp),
                }
            }
            recv(task_receiver) -> task => {
                if let Ok(task) = task {
                    on_task(state, task);
                }
            }
            // The watcher this server runs when the client will not.
            // `never()` when there is none, so the arm is simply inert —
            // crossbeam needs a receiver either way.
            recv(state.loader_receiver.clone().unwrap_or_else(crossbeam_channel::never)) -> msg => {
                if let Ok(msg) = msg {
                    state.handle_vfs_msg(msg);
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

            // The workspace symbol index used to be rebuilt here, from a
            // dirty-file list, by cloning the whole map. It is
            // `ide_db::db_query::workspace_symbol_index` now — a salsa
            // query keyed by the workspace's files, so an unchanged file's
            // symbols are reused and nothing is cloned per edit.

            let project_or_mem_docs_changed =
                became_quiescent || state_changed || memdocs_added_or_removed;

            if became_quiescent {
                state.prime_caches_queue.request_op("became quiescent".to_owned(), ());
            }

            let client_refresh = became_quiescent || state_changed;
            if client_refresh {
                // A refresh has nothing to come back, so its response is
                // logged and dropped — but it goes through the queue, so an
                // answer to something else is never mistaken for it.
                // The cached baselines are what a delta is computed against,
                // and asking the client to re-pull says they no longer
                // describe anything. rust-analyzer clears here too
                // (`main_loop.rs:505`); this cache had no invalidation at
                // all — `invalidate` and `clear` were written, waived as
                // dead code, and never called.
                state.semantic_tokens_cache.clear();
                state.send_request::<lsp_types::request::SemanticTokensRefresh>((), noop_response);
                state.send_request::<lsp_types::request::CodeLensRefresh>((), noop_response);
                state
                    .send_request::<lsp_types::request::InlayHintRefreshRequest>((), noop_response);
            }

            // Eviction takes `&mut` on the database and cancels every live
            // snapshot, so it may only run when nothing holds one — which is
            // what `task_pool.is_empty()` answers, as rust-analyzer's
            // `task_pool.handle.is_empty() && fmt_pool.handle.is_empty()`
            // does (`main_loop.rs:541-546`).
            //
            // The guard used to read `task_receiver`, the channel results
            // arrive on. That is empty *precisely while* a task is still
            // working, so eviction fired at the one moment it must not: the
            // diagnostics wave spawned a turn earlier was cancelled, a
            // cancelled wave is dropped file by file in
            // `fetch_native_diagnostics`, and eviction is not a *change*, so
            // no later turn asked again. An opened file got no diagnostics
            // at all until the next edit happened to schedule one.
            //
            // It also runs *before* the wave, so the turn that bumps the
            // nonce cannot cancel the snapshot it is about to take.
            let current_nonce = state.analysis_host.raw_database().nonce();
            if state.task_pool.is_empty() && current_nonce != state.last_gc_revision {
                state.analysis_host.trigger_garbage_collection();
                state.last_gc_revision = current_nonce;
            }

            if project_or_mem_docs_changed {
                state.update_diagnostics();
            }
        }

        if let Some((cause, ())) = state.fetch_workspaces_queue.should_start_op() {
            state.fetch_workspaces(cause);
        }

        if let Some((_cause, ())) = state.prime_caches_queue.should_start_op() {
            let analysis = state.analysis_snapshot();
            let cancel = state.prime_caches_cancel.clone();
            let num_worker_threads = state.config.prime_caches_num_threads();
            state.task_pool.spawn_with_sender(stdx::thread::ThreadIntent::Worker, move |sender| {
                let _ = sender.send(Task::PrimeCaches(PrimeCachesProgress::Begin));
                // Through `Analysis`, with the worker count the client set
                // and a progress callback that reaches the client — a
                // literal `2` and `&|_| {}` stood here, and the phase that
                // dominates startup reported nothing while it ran.
                let report = sender.clone();
                let _ = analysis.parallel_prime_caches(num_worker_threads, &cancel, move |p| {
                    let _ = report.send(Task::PrimeCaches(PrimeCachesProgress::Report(p)));
                });
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
                    // The version of the buffer these are about, so a client
                    // can drop what it already knows is stale. `None` says
                    // "no idea", which is the one answer it cannot act on —
                    // and `MemDocs::get`, the only way to answer, carried an
                    // `#[expect(dead_code)]`.
                    let version = state.mem_docs.get(url).map(|doc| doc.version);
                    state.send_notification::<lsp_types::notification::PublishDiagnostics>(
                        lsp_types::PublishDiagnosticsParams {
                            uri: url.clone(),
                            diagnostics,
                            version,
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
        Task::WorkspaceScan(scan) => {
            if !scan.inputs.matches_disk() {
                state
                    .fetch_workspaces_queue
                    .request_op("manifest changed during discovery".into(), ());
            }
            if scan.revision != state.fetch_workspaces_queue.revision()
                || scan.config != state.config.workspace
                || scan.folders != state.workspace_folders
            {
                if state.fetch_workspaces_queue.op_in_progress() {
                    state.fetch_workspaces_queue.op_completed(());
                }
                return;
            }
            let files = match scan.files {
                Ok(files) => files,
                Err(error) => {
                    state.discovery_watch_files.extend(scan.inputs.watch_files);
                    state.discovery_watch_files.sort();
                    state.discovery_watch_files.dedup();
                    state.request_workspace_load(&[]);
                    state.log_message(MessageType::ERROR, error.clone());
                    state.project_error = Some(error);
                    if state.fetch_workspaces_queue.op_in_progress() {
                        state.fetch_workspaces_queue.op_completed(());
                    }
                    return;
                }
            };
            state.loading_project_inputs = scan.inputs.clone();
            state.discovery_watch_files = scan.inputs.watch_files;
            state.loading_project_config = Some((scan.config, scan.folders, scan.revision));
            let count = files.len();

            // Workspace scan is salsa-only.
            // Only register files as salsa inputs — no File struct, no eager parse.
            // Salsa queries handle all derived data lazily on demand.
            // Classify into local vs library source roots *first*: a file's
            // durability comes from the root it is in, and `sync_disk_file`
            // reads `library_root` to decide. Classifying afterwards — which
            // is where this used to be — meant every file, prelude included,
            // was synced against an empty library root.
            let sail_dir = state.effective_sail_dir.clone();
            let mut url_to_fid = HashMap::new();
            for (uri, path) in &files {
                let fid = state.vfs.file_id_for_url(uri);
                url_to_fid.insert(uri.clone(), fid);
                let abs = paths::AbsPathBuf::assert_utf8(path.clone());
                let vfs_path = base_db::VfsPath::new(abs.clone());
                let is_library = sail_dir.as_ref().is_some_and(|sd| {
                    let sd_abs = paths::AbsPathBuf::assert_utf8(sd.clone());
                    abs.strip_prefix(&sd_abs).is_some()
                });
                if is_library {
                    state.library_root.insert(fid, vfs_path);
                } else {
                    state.local_root.insert(fid, vfs_path);
                }
            }
            let modules: Vec<_> = scan
                .modules
                .into_iter()
                .map(|(urls, requires)| base_db::module_graph::ModuleData {
                    files: urls.iter().filter_map(|u| url_to_fid.get(u).copied()).collect(),
                    requires,
                })
                .collect();
            let owned: std::collections::HashSet<_> =
                modules.iter().flat_map(|m| m.files.iter().copied()).collect();
            let globals = files
                .iter()
                .filter_map(|(u, _)| url_to_fid.get(u).copied())
                .filter(|id| !owned.contains(id))
                .collect();
            let graph = base_db::module_graph::ModuleGraph { modules, globals };
            state.loading_source_project =
                scan.source_roots.map(|roots| base_db::preprocess::SourceProject {
                    roots: roots.iter().filter_map(|(u, _)| url_to_fid.get(u).copied()).collect(),
                    root_modules: roots
                        .iter()
                        .filter(|(u, _)| url_to_fid.contains_key(u))
                        .map(|(_, owner)| *owner)
                        .collect(),
                    paths: files
                        .iter()
                        .filter_map(|(u, p)| url_to_fid.get(u).map(|id| (p.clone(), *id)))
                        .collect(),
                    library: sail_dir.as_ref().map(|p| p.join("lib")),
                    include_paths: crate::reload::include_search_paths(
                        &state.workspace_folders,
                        &state.config.workspace,
                    ),
                    target: state.config.workspace.target.clone(),
                    modules: graph.clone(),
                });
            state.loading_module_graph = Some(graph);
            state.sync_source_roots();

            // The scan says *which* files and in what order; reading them is
            // the loader's job, and it answers asynchronously. Until it has,
            // there is nothing to assemble a workspace out of.
            if state.request_workspace_load(&files) {
                state.pending_workspace = Some((files, count));
                return;
            }

            // No loader — no workspace folder to watch, or it could not be
            // started. Read them here, which is what the scan itself used to
            // do for every file.
            for (uri, path) in &files {
                if let Ok(text) = std::fs::read_to_string(path) {
                    // Disk text, so an open document keeps its own.
                    state.sync_disk_text(uri, &text);
                }
            }
            state.process_changes();
            state.finish_workspace_load(&files, count);
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
        Task::Flycheck(found) => {
            // Group by file. A path the workspace does not know is dropped
            // rather than guessed at — upstream may name a prelude file.
            let mut by_file: HashMap<base_db::FileId, Vec<lsp_types::Diagnostic>> = HashMap::new();
            for (path, diag) in found {
                let Ok(url) = lsp_types::Url::from_file_path(&path) else { continue };
                let Some(fid) = state.vfs.lookup_file_id_by_url(&url) else { continue };
                by_file.entry(fid).or_default().push(diag);
            }
            state.diagnostics.set_check_diagnostics(by_file.into_iter().collect());
        }
        Task::PrimeCaches(progress) => match progress {
            PrimeCachesProgress::Begin => {
                if state.supports_progress {
                    let token = state.progress_token("Priming caches");
                    state.cancellable_progress = Some(token.clone());
                    state.prime_caches_progress = Some(crate::progress::ProgressReporter::begin(
                        &state.sender,
                        token,
                        "Priming caches",
                        true,
                    ));
                }
            }
            PrimeCachesProgress::Report(report) => {
                if let Some(reporter) = state.prime_caches_progress.as_ref() {
                    let pct = (report.files_done * 100).checked_div(report.files_total).unwrap_or(0)
                        as u32;
                    reporter.report(
                        &format!(
                            "{} {}/{}",
                            report.work_type, report.files_done, report.files_total
                        ),
                        Some(pct),
                    );
                }
            }
            PrimeCachesProgress::End { cancelled } => {
                if let Some(reporter) = state.prime_caches_progress.take() {
                    reporter.end(if cancelled { "cancelled" } else { "done" });
                }
                state.cancellable_progress = None;
                // No eviction here. It was the *second* call site, and the
                // unguarded one: priming ends while the diagnostics wave
                // spawned by the same settling change is still computing, so
                // this cancelled it every time a file was opened. The loop's
                // one guarded call is enough — it fires as soon as the pool
                // is idle and the nonce has moved.
                state.prime_caches_queue.op_completed(());
                if cancelled {
                    state.prime_caches_queue.request_op("restart after cancellation".into(), ());
                }
            }
        },
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

    let mut dispatcher = RequestDispatcher { req: Some(req), global_state: state };
    dispatcher.on_sync_mut::<request::Shutdown>(|state, ()| {
        state.shutdown_requested = true;
        Ok(())
    });
    // Anything that arrives after `shutdown` is answered, and answered
    // with the error the protocol names.
    if let RequestDispatcher { req: Some(req), global_state } = &mut dispatcher {
        if global_state.shutdown_requested {
            let id = req.id.clone();
            global_state.respond(lsp_server::Response::new_err(
                id,
                lsp_server::ErrorCode::InvalidRequest as i32,
                "Shutdown already requested.".to_owned(),
            ));
            return;
        }
    }

    dispatcher
        // On the main thread, and only these: rust-analyzer keeps
        // `SelectionRange` and `OnTypeFormatting` sync too, because both are
        // pure syntax, both are wanted within a keystroke, and neither can
        // block on inference. Everything else takes a snapshot and goes to
        // the pool — while a sync handler runs, the loop reads no event at
        // all, not even the cancellation for the request it is running.
        .on_sync::<request::SelectionRangeRequest>(handlers::handle_selection_range)
        .on_sync::<request::OnTypeFormatting>(handlers::handle_on_type_formatting)
        .on_sync::<crate::lsp_ext::MatchingBrace>(handlers::handle_matching_brace)
        .on_sync::<crate::lsp_ext::JoinLines>(handlers::handle_join_lines)
        .on_sync::<crate::lsp_ext::OnEnter>(handlers::handle_on_enter)
        // ALLOW_RETRYING = true: latency-sensitive, client will re-request on cancel
        // ALLOW_RETRYING = false: user-triggered, show error on cancel
        .on::<true, request::Completion>(handlers::handle_completion)
        .on::<true, request::ResolveCompletionItem>(handlers::handle_resolve_completion_item)
        .on::<true, request::InlayHintRequest>(handlers::handle_inlay_hint)
        .on::<true, request::SignatureHelpRequest>(handlers::handle_signature_help)
        .on::<true, request::HoverRequest>(handlers::handle_hover)
        // Semantic tokens are redrawn on every edit, so a cancelled one is
        // worth retrying rather than reporting — RA marks the same three
        // latency-sensitive, and the range request no-retry.
        .on::<true, request::SemanticTokensFullRequest>(handlers::handle_semantic_tokens_full)
        .on::<true, request::SemanticTokensFullDeltaRequest>(
            handlers::handle_semantic_tokens_full_delta,
        )
        .on::<false, request::SemanticTokensRangeRequest>(handlers::handle_semantic_tokens_range)
        .on::<true, crate::lsp::ext::CodeActionResolveRequest>(handlers::handle_code_action_resolve)
        .on::<false, request::DocumentSymbolRequest>(handlers::handle_document_symbol)
        .on::<false, request::DocumentDiagnosticRequest>(handlers::handle_document_diagnostic)
        .on::<false, request::DocumentHighlightRequest>(handlers::handle_document_highlight)
        .on::<false, crate::lsp::ext::CodeActionRequest>(handlers::handle_code_action)
        .on::<false, request::Formatting>(handlers::handle_formatting)
        .on::<false, request::RangeFormatting>(handlers::handle_range_formatting)
        .on::<false, request::FoldingRangeRequest>(handlers::handle_folding_range)
        .on::<false, request::LinkedEditingRange>(handlers::handle_linked_editing_range)
        .on::<false, request::DocumentLinkRequest>(handlers::handle_document_link)
        .on::<false, request::DocumentLinkResolve>(handlers::handle_document_link_resolve)
        .on::<false, request::PrepareRenameRequest>(handlers::handle_prepare_rename)
        .on::<false, request::CodeLensResolve>(handlers::handle_code_lens_resolve)
        .on::<false, request::InlayHintResolveRequest>(handlers::handle_inlay_hint_resolve)
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
        .on::<false, crate::lsp_ext::ViewSyntaxTree>(handlers::handle_view_syntax_tree)
        .on::<false, crate::lsp_ext::ViewHir>(handlers::handle_view_hir)
        .on::<false, crate::lsp_ext::ViewItemTree>(handlers::handle_view_item_tree)
        .on::<false, crate::lsp_ext::ExpandInclude>(handlers::handle_expand_include)
        .on::<false, crate::lsp_ext::EffectAnnotations>(handlers::handle_effect_annotations)
        .on::<false, crate::lsp_ext::SailLspStatus>(handlers::handle_sail_lsp_status)
        .on::<false, crate::lsp_ext::ViewIncludeGraph>(handlers::handle_view_include_graph)
        .on::<false, crate::lsp_ext::MoveItem>(handlers::handle_move_item)
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
        .on_sync_mut::<lsp_types::notification::WorkDoneProgressCancel>(
            handlers::handle_work_done_progress_cancel,
        )
        .on_sync_mut::<crate::lsp_ext::ReloadWorkspace>(handlers::handle_reload_workspace)
        .finish();
}

/// A response we asked for but have nothing to do with.
fn noop_response(_state: &mut GlobalState, response: lsp_server::Response) {
    if let Some(err) = response.error {
        log::debug!("client refused a refresh: {}", err.message);
    }
}

#[cfg(test)]
mod project_reload_tests {
    use super::*;

    #[track_caller]
    fn pump_until(
        state: &mut GlobalState,
        tasks: &crossbeam_channel::Receiver<Task>,
        done: impl Fn(&GlobalState) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some((cause, ())) = state.fetch_workspaces_queue.should_start_op() {
                state.fetch_workspaces(cause);
            }
            while let Ok(task) = tasks.try_recv() {
                on_task(state, task);
            }
            if let Some(receiver) = state.loader_receiver.clone() {
                while let Ok(message) = receiver.try_recv() {
                    state.handle_vfs_msg(message);
                }
            }
            state.process_changes();
            if !state.fetch_workspaces_queue.op_in_progress()
                && !state.fetch_workspaces_queue.op_requested()
                && done(state)
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "workspace did not converge: {:?}, error {:?}, loaded {}",
                state.fetch_workspaces_queue,
                state.project_error,
                state.workspace_loaded
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn external_include_recovers_without_reload_and_preserves_editor_overlays() {
        for dynamic_registration in [false, true] {
            check_external_include_recovery(dynamic_registration);
        }
    }

    fn check_external_include_recovery(dynamic_registration: bool) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "sail-external-watch-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let workspace = root.join("workspace");
        let external = root.join("external/nested/lib.sail");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(workspace.join("model.sail_project"), "M { files main.sail }").unwrap();
        std::fs::write(workspace.join("main.sail"), "$include \"../external/nested/lib.sail\"\n")
            .unwrap();
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.client_caps.0 = serde_json::from_value(serde_json::json!({
            "workspace": { "didChangeWatchedFiles": { "dynamicRegistration": dynamic_registration } }
        })).unwrap();
        state.effective_sail_dir = None;
        state.workspace_folders.insert(Url::from_directory_path(&workspace).unwrap());
        state.fetch_workspaces_queue.request_op("startup".into(), ());
        pump_until(&mut state, &tasks, |s| s.workspace_loaded);
        let external_url = Url::from_file_path(&external).unwrap();
        let contains_external = |s: &GlobalState| {
            let db = s.analysis_host.raw_database();
            base_db::preprocess::WorkspaceSourceProject::try_get(db)
                .is_some_and(|p| p.project(db).paths.iter().any(|(p, _)| p == &external))
        };
        assert!(!contains_external(&state));
        std::fs::create_dir_all(external.parent().unwrap()).unwrap();
        std::fs::write(&external, "val external : int\n").unwrap();
        pump_until(&mut state, &tasks, contains_external);
        let source_is = |s: &GlobalState, expected: &str| {
            let db = s.analysis_host.raw_database();
            s.vfs
                .lookup_file_id_by_url(&external_url)
                .and_then(|id| db.files().file_text(id))
                .is_some_and(|ft| ft.text(db).as_ref() == expected)
        };
        assert!(source_is(&state, "val external : int\n"));
        std::fs::write(&external, "val external : bool\n").unwrap();
        pump_until(&mut state, &tasks, |s| source_is(s, "val external : bool\n"));

        let overlay = "val external : string\n";
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: external_url.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: overlay.into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        std::fs::write(&external, "val external : unit\n").unwrap();
        let receiver = state.loader_receiver.clone().unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let message = receiver
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .unwrap();
            let observed = matches!(&message, vfs::loader::Message::Changed { files }
                if files.iter().any(|(p, text)| p.as_ref() as &std::path::Path == external
                    && text.as_deref() == Some(b"val external : unit\n")));
            state.handle_vfs_msg(message);
            if observed {
                break;
            }
        }
        state.process_changes();
        // Closing asks the same loader to restore the current disk contents.
        assert!(source_is(&state, overlay));
        state.mem_docs.remove(&external_url);
        state.restore_disk_text(&external_url);
        pump_until(&mut state, &tasks, |s| source_is(s, "val external : unit\n"));

        std::fs::remove_file(&external).unwrap();
        pump_until(&mut state, &tasks, |s| !contains_external(s));
        std::fs::write(&external, "val recovered : int\n").unwrap();
        pump_until(&mut state, &tasks, |s| {
            contains_external(s) && source_is(s, "val recovered : int\n")
        });
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    fn state_with_active_file() -> (GlobalState, Url, base_db::FileId) {
        let (sender, _) = crossbeam_channel::unbounded();
        let (tasks, _) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let url = Url::parse("file:///previous.sail").unwrap();
        state.sync_disk_text(&url, "val previous : int\n");
        state.process_changes();
        let id = state.vfs.lookup_file_id_by_url(&url).unwrap();
        state.ensure_workspace_files(vec![id]);
        (state, url, id)
    }

    fn active_ids(state: &GlobalState) -> Vec<base_db::FileId> {
        let db = state.analysis_host.raw_database();
        base_db::WorkspaceFiles::try_get(db)
            .unwrap()
            .file_texts(db)
            .iter()
            .map(|ft| ft.file_id(db))
            .collect()
    }

    #[test]
    fn loading_installs_module_edges_even_when_file_membership_is_unchanged() {
        let (mut state, a, a_id) = state_with_active_file();
        let b = Url::parse("file:///consumer.sail").unwrap();
        state.sync_disk_text(&b, "val consumer : int\n");
        state.process_changes();
        let b_id = state.vfs.lookup_file_id_by_url(&b).unwrap();
        for requires in [vec![], vec![0], vec![]] {
            let scan = crate::reload::WorkspaceScan {
                revision: state.fetch_workspaces_queue.revision(),
                inputs: Default::default(),
                files: Ok(vec![
                    (a.clone(), a.to_file_path().unwrap()),
                    (b.clone(), b.to_file_path().unwrap()),
                ]),
                modules: vec![(vec![a.clone()], vec![]), (vec![b.clone()], requires.clone())],
                source_roots: None,
                config: state.config.workspace.clone(),
                folders: state.workspace_folders.clone(),
            };
            on_task(&mut state, Task::WorkspaceScan(scan));
            assert_eq!(active_ids(&state), [a_id, b_id]);
            assert_eq!(
                base_db::module_graph::can_reference_file(
                    state.analysis_host.raw_database(),
                    b_id,
                    a_id
                ),
                !requires.is_empty()
            );
        }
    }

    #[test]
    fn invalid_project_preserves_last_good_workspace_and_reports_error() {
        let (mut state, _, id) = state_with_active_file();
        let scan = crate::reload::WorkspaceScan {
            revision: state.fetch_workspaces_queue.revision() + 1,
            inputs: Default::default(),
            files: Err("Unknown project variable RMEM".into()),
            modules: Vec::new(),
            source_roots: None,
            config: state.config.workspace.clone(),
            folders: state.workspace_folders.clone(),
        };
        state.fetch_workspaces_queue.request_op("test".into(), ());
        state.fetch_workspaces_queue.should_start_op().unwrap();
        on_task(&mut state, Task::WorkspaceScan(scan));
        assert_eq!(active_ids(&state), [id]);
        assert!(!state.fetch_workspaces_queue.op_in_progress());
        assert!(matches!(state.current_status().health, crate::lsp_ext::Health::Error));
        assert!(state.current_status().message.unwrap().contains("RMEM"));
        state.finish_workspace_load(&[], 0);
        assert!(state.project_error.is_none());
        assert!(active_ids(&state).is_empty());
    }

    #[test]
    fn configuration_changes_discard_stale_scan_and_load_results() {
        let (mut state, _, id) = state_with_active_file();
        let config = state.config.workspace.clone();
        let folders = state.workspace_folders.clone();
        state.config.workspace.project_modules = vec!["New".into()];
        let revision = state.fetch_workspaces_queue.revision();
        on_task(
            &mut state,
            Task::WorkspaceScan(crate::reload::WorkspaceScan {
                revision,
                inputs: Default::default(),
                files: Ok(Vec::new()),
                modules: Vec::new(),
                source_roots: None,
                config: config.clone(),
                folders: folders.clone(),
            }),
        );
        assert_eq!(active_ids(&state), [id]);
        state.loading_project_config =
            Some((config, folders, state.fetch_workspaces_queue.revision()));
        state.finish_workspace_load(&[], 0);
        assert_eq!(active_ids(&state), [id]);
        assert!(state.loading_project_config.is_none());
    }

    #[test]
    fn manifest_changes_discard_scan_success_and_failure_before_publication() {
        for files in [Ok(Vec::new()), Err("obsolete parse error".into())] {
            let (mut state, _, id) = state_with_active_file();
            state.fetch_workspaces_queue.request_op("initial scan".into(), ());
            state.fetch_workspaces_queue.should_start_op().unwrap();
            let scan = crate::reload::WorkspaceScan {
                revision: state.fetch_workspaces_queue.revision(),
                inputs: Default::default(),
                files,
                modules: Vec::new(),
                source_roots: None,
                config: state.config.workspace.clone(),
                folders: state.workspace_folders.clone(),
            };
            crate::handlers::notification::handle_did_change_watched_files(
                &mut state,
                lsp_types::DidChangeWatchedFilesParams {
                    changes: vec![lsp_types::FileEvent {
                        uri: Url::parse("file:///model.sail_project").unwrap(),
                        typ: lsp_types::FileChangeType::CHANGED,
                    }],
                },
            )
            .unwrap();
            on_task(&mut state, Task::WorkspaceScan(scan));
            assert_eq!(active_ids(&state), [id]);
            assert!(state.project_error.is_none());
            assert!(!state.fetch_workspaces_queue.op_in_progress());
            assert!(state.fetch_workspaces_queue.should_start_op().is_some());
        }
    }

    #[test]
    fn manifest_changes_during_loading_preserve_the_installed_workspace() {
        let (mut state, _, id) = state_with_active_file();
        state.fetch_workspaces_queue.request_op("scan".into(), ());
        state.fetch_workspaces_queue.should_start_op().unwrap();
        state.loading_project_config = Some((
            state.config.workspace.clone(),
            state.workspace_folders.clone(),
            state.fetch_workspaces_queue.revision(),
        ));
        state.loading_module_graph = Some(Default::default());
        state.loading_source_project = Some(Default::default());
        state.pending_workspace = Some((Vec::new(), 0));
        state.handle_vfs_msg(vfs::loader::Message::Changed {
            files: vec![(
                paths::AbsPathBuf::assert_utf8(std::env::temp_dir().join("model.sail_project")),
                Some(b"M {}".to_vec()),
            )],
        });
        state.handle_vfs_msg(vfs::loader::Message::Progress {
            n_total: 0,
            n_done: vfs::loader::LoadingProgress::Finished,
            dir: None,
            config_version: state.workspace_load_version,
        });
        assert_eq!(active_ids(&state), [id]);
        assert!(state.loading_source_project.is_none());
        assert!(state.loading_module_graph.is_none());
        assert!(state.pending_workspace.is_none());
        assert!(state.fetch_workspaces_queue.should_start_op().is_some());
    }

    #[test]
    fn delayed_watch_notifications_cannot_publish_stale_manifest_results() {
        let path = std::env::temp_dir()
            .join(format!("sail-manifest-revision-{}.sail_project", std::process::id()));
        for during_loading in [false, true] {
            let (mut state, _, id) = state_with_active_file();
            state.fetch_workspaces_queue.request_op("scan".into(), ());
            state.fetch_workspaces_queue.should_start_op().unwrap();
            std::fs::write(&path, "A {}").unwrap();
            let inputs = crate::reload::DiscoveryInputs {
                watch_files: vec![path.clone()],
                manifests: std::collections::BTreeMap::from([(path.clone(), Some("A {}".into()))]),
            };
            std::fs::write(&path, "B {}").unwrap();
            // No watcher notification has been dispatched yet.
            if during_loading {
                state.loading_project_config = Some((
                    state.config.workspace.clone(),
                    state.workspace_folders.clone(),
                    state.fetch_workspaces_queue.revision(),
                ));
                state.loading_project_inputs = inputs;
                state.finish_workspace_load(&[], 0);
            } else {
                let scan = crate::reload::WorkspaceScan {
                    revision: state.fetch_workspaces_queue.revision(),
                    inputs,
                    files: Ok(Vec::new()),
                    modules: Vec::new(),
                    source_roots: None,
                    config: state.config.workspace.clone(),
                    folders: state.workspace_folders.clone(),
                };
                on_task(&mut state, Task::WorkspaceScan(scan));
            }
            assert_eq!(active_ids(&state), [id]);
            assert!(state.fetch_workspaces_queue.should_start_op().is_some());
        }
        std::fs::remove_file(path).unwrap();
    }
}
