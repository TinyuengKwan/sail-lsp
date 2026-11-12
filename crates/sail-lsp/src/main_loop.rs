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
#[allow(clippy::large_enum_variant)]
pub(crate) enum Task {
    EditorDocument {
        source: Option<crate::editor_commands::SourceStamp>,
        result: Result<crate::editor_commands::Document, String>,
    },
    /// Workspace scan discovered files on disk — which ones, in the order
    /// the project declares them. Not what they say: the loader reads them.
    WorkspaceScan(crate::reload::WorkspaceScan),
    WorkspacePreprocessed {
        revision: u64,
        load_version: u32,
        result: Result<hir::preprocess::PreparedWorkspaceData, String>,
    },
    /// Async request handler completed — response ready to send.
    Response(lsp_server::Response),
    /// What a `textDocument/diagnostic` answer said about a file's health.
    PullDiagnosticsHealth(crate::analysis_health::PullReport),
    /// Async request handler was cancelled and should be retried.
    Retry(lsp_server::Request),
    /// Prime caches progress/completion.
    PrimeCaches(PrimeCachesProgress),
    /// Async diagnostics computation completed for a batch of files.
    /// DiagnosticsTaskKind carries `(generation, Vec<(FileId, diags)>)`.
    Diagnostics(DiagnosticsTaskKind),
    /// What `sail --just-check` said, by the path it named.
    FlycheckStarted {
        target: crate::flycheck::CheckTarget,
        generation: u64,
        cache: crate::flycheck::cache::CacheInfo,
    },
    Flycheck {
        target: crate::flycheck::CheckTarget,
        /// Only the latest run of a target is believed.
        generation: u64,
        found: crate::flycheck::FlycheckResult,
        status: crate::flycheck::Status,
        run: crate::flycheck::RunReport,
    },
}

pub(crate) struct PendingPreprocessing {
    revision: u64,
    load_version: u32,
    files: Vec<(Url, PathBuf)>,
    count: usize,
}

pub(crate) struct PreprocessingCancellation(pub hir::CancellationToken);
impl Drop for PreprocessingCancellation {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn preprocessing_database(cancel: hir::CancellationToken) -> ide_db::root_database::RootDatabase {
    ide_db::root_database::RootDatabase::with_event_callback(Some(Box::new(move |event| {
        if matches!(event.kind, salsa::EventKind::WillCheckCancellation)
            && cancel.is_cancelled()
            && !std::thread::panicking()
        {
            // This database has no shared consumers. Cancel at normal
            // checkpoints and leave unwinding/discard cleanup uninterrupted.
            std::panic::resume_unwind(Box::new(stdx::cancellation::Cancelled));
        }
    })))
}

/// Progress for prime_caches background task.
#[derive(Debug)]
pub(crate) enum PrimeCachesProgress {
    Begin,
    Report(ide_db::prime_caches::ParallelPrimeCachesProgress),
    End { cancelled: bool, failed: bool },
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
    Unavailable {
        kind: NativeDiagnosticsFetchKind,
        generation: crate::diagnostics::DiagnosticsGeneration,
        files: Vec<base_db::FileId>,
        /// None means normal cancellation, Some means internal failure.
        failure: Option<String>,
    },

    Syntax(
        crate::diagnostics::DiagnosticsGeneration,
        Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>,
    ),
    Semantic(
        crate::diagnostics::DiagnosticsGeneration,
        Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>,
    ),
}

impl DiagnosticsTaskKind {
    fn compute(
        kind: NativeDiagnosticsFetchKind,
        generation: crate::diagnostics::DiagnosticsGeneration,
        files: Vec<base_db::FileId>,
        compute: impl FnOnce() -> ide::Cancellable<Vec<(base_db::FileId, Vec<lsp_types::Diagnostic>)>>,
    ) -> Self {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            salsa::Cancelled::catch(std::panic::AssertUnwindSafe(compute))
        }));
        let unavailable = |failure| Self::Unavailable { kind, generation, files, failure };
        match result {
            Ok(Ok(Ok(diags))) => match kind {
                NativeDiagnosticsFetchKind::Syntax => Self::Syntax(generation, diags),
                NativeDiagnosticsFetchKind::Semantic => Self::Semantic(generation, diags),
            },
            Ok(Ok(Err(ide_db::ide_types::CancelledError::PendingWrite))) => unavailable(None),
            Ok(Ok(Err(ide_db::ide_types::CancelledError::PropagatedPanic))) => {
                unavailable(Some("dependent query panicked".into()))
            }
            Ok(Err(salsa::Cancelled::PropagatedPanic { .. })) => {
                unavailable(Some("dependent query panicked".into()))
            }
            Ok(Err(_)) => unavailable(None),
            Err(payload) => {
                unavailable(Some(crate::analysis_health::panic_message(payload.as_ref())))
            }
        }
    }
}

impl GlobalState {
    fn compiler_policy_for(
        &self,
        target: &crate::flycheck::CheckTarget,
    ) -> crate::flycheck::CompilerDiagnosticPolicy {
        let path = match target {
            crate::flycheck::CheckTarget::Project(manifest) => self
                .projects
                .values()
                .find(|project| &project.manifest == manifest)
                .map(|project| &project.workspace_root)
                .unwrap_or(manifest),
            crate::flycheck::CheckTarget::File(path) => path,
        };
        let uri = Url::from_file_path(path).ok();
        let config = uri.as_ref().map(|uri| self.config_for_uri(uri)).unwrap_or(&self.config);
        (&config.flycheck).into()
    }

    pub(crate) fn apply_compiler_diagnostic_policies(&mut self) {
        for target in self.diagnostics.check_targets() {
            let policy = self.compiler_policy_for(&target);
            self.diagnostics.set_check_policy(&target, policy);
            let (reported, visible) = self.diagnostics.check_counts(&target);
            if let Some(run) =
                self.flycheck_history.get_mut(&target).and_then(|h| h.last_run.as_mut())
            {
                run.diagnostics_ignored = reported.saturating_sub(visible);
            }
        }
    }
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
    fn update_diagnostics(&mut self) {
        let generation = self.diagnostics.next_generation();
        let subscribed = self.subscribed_files();
        if subscribed.is_empty() {
            let health = std::sync::Arc::make_mut(&mut self.analysis_health);
            health.recovered("syntaxDiagnostics");
            health.recovered("semanticDiagnostics");
            health.reported_unknowns.clear();
            return;
        }
        let analysis = self.analysis_snapshot();
        let diag_configs: std::collections::HashMap<_, _> = subscribed
            .iter()
            .map(|(uri, _)| (uri.clone(), self.config_for_uri(uri).diagnostics.to_ide_config()))
            .collect();
        self.task_pool.spawn_with_sender(
            stdx::thread::ThreadIntent::LatencySensitive,
            move |sender| {
                for kind in
                    [NativeDiagnosticsFetchKind::Syntax, NativeDiagnosticsFetchKind::Semantic]
                {
                    let task = DiagnosticsTaskKind::compute(
                        kind,
                        generation,
                        subscribed.iter().map(|(_, id)| *id).collect(),
                        || {
                            crate::diagnostics::fetch_native_diagnostics(
                                &analysis,
                                &subscribed,
                                kind,
                                |uri| diag_configs[uri].clone(),
                            )
                        },
                    );
                    if sender.send(Task::Diagnostics(task)).is_err() {
                        break;
                    }
                }
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
        let db = self.analysis_host.raw_database();
        let bytes: usize = self
            .loading_source_project
            .as_ref()
            .map(|project| {
                project
                    .paths
                    .iter()
                    .filter_map(|(_, id)| db.files().file_text(*id))
                    .map(|file| file.text(db).len())
                    .sum()
            })
            .unwrap_or(0);
        // Tiny fixtures and projects need no extra scheduling round. Large
        // projects derive their model without borrowing the host database.
        if self.loading_source_project.is_some() && (files.len() > 64 || bytes > 256 * 1024) {
            self.pending_preprocessing = Some(PendingPreprocessing {
                revision: self.fetch_workspaces_queue.revision(),
                load_version: self.workspace_load_version,
                files: files.to_vec(),
                count,
            });
            self.start_workspace_preprocessing(None);
            return;
        }
        self.finish_workspace_load_inner(files, count);
    }

    fn start_workspace_preprocessing(
        &mut self,
        seed: Option<hir::preprocess::PreparedWorkspaceData>,
    ) {
        if self.preprocessing_running || self.pending_preprocessing.is_none() {
            return;
        }
        let pending = self.pending_preprocessing.as_ref().unwrap();
        if pending.revision != self.fetch_workspaces_queue.revision()
            || pending.load_version != self.workspace_load_version
            || self.loading_project_config.as_ref().is_some_and(|(config, folders, revision)| {
                *revision != self.fetch_workspaces_queue.revision()
                    || *config != self.config.workspace
                    || *folders != self.workspace_folders
            })
        {
            // A reload can supersede discovery while VFS is still reading.
            // No preprocessing worker was started, so no completion message
            // will release this operation for its queued replacement.
            self.pending_preprocessing = None;
            self.loading_source_project = None;
            self.loading_module_graph = None;
            self.loading_project_config = None;
            if self.fetch_workspaces_queue.op_in_progress() {
                self.fetch_workspaces_queue.op_completed(());
            }
            return;
        }
        let Some(project) = self.loading_source_project.clone() else {
            return;
        };
        let db = self.analysis_host.raw_database();
        let sources: Vec<_> = project
            .paths
            .iter()
            .filter_map(|(_, id)| {
                db.files().file_text(*id).map(|file| (*id, file.text(db).clone()))
            })
            .collect();
        let strict_bitvector = base_db::strict_bitvector(db);
        let revision = self.fetch_workspaces_queue.revision();
        let load_version = self.workspace_load_version;
        self.preprocessing_running = true;
        self.preprocessing_cancel = PreprocessingCancellation(hir::CancellationToken::new());
        let cancel = self.preprocessing_cancel.0.clone();
        self.task_pool.spawn(stdx::thread::ThreadIntent::Worker, move || {
            let started = std::time::Instant::now();
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                use salsa::Setter;
                let mut db = preprocessing_database(cancel);
                base_db::LanguageOptions::try_get(&db).unwrap().set_strict_bitvector(&mut db).to(strict_bitvector);
                // Preserve compact results for unchanged sources when an
                // overlay edit superseded the preceding plan. Repeating a
                // whole-project cold derivation on each keystroke would
                // otherwise prevent startup from catching up with typing.
                if let Some(seed) = seed {
                    hir::preprocess::PreparedWorkspace::try_get(&db).unwrap().set_data(&mut db).to(Some(std::sync::Arc::new(seed)));
                }
                let files = sources.into_iter().map(|(id, text)| (id, base_db::FileText::new(&db, text, id))).collect();
                let input = base_db::preprocess::WorkspaceSourceProject::new(&db, project, files);
                hir::preprocess::prepare_workspace(&db, input)
            })).map_err(|payload| format!("Workspace preprocessing failed: {}", crate::analysis_health::panic_message(payload.as_ref())));
            tracing::debug!(target: "sail_lsp::latency", "workspace_preprocess revision={revision} load_version={load_version} duration_us={}", started.elapsed().as_micros());
            Task::WorkspacePreprocessed { revision, load_version, result }
        });
    }

    fn finish_workspace_load_inner(&mut self, files: &[(Url, PathBuf)], count: usize) {
        let module_graph = self.loading_module_graph.take();
        let source_project = self.loading_source_project.take();
        let inputs = std::mem::take(&mut self.loading_project_inputs);
        if !inputs.matches_disk() {
            self.request_workspace_reload(
                "manifest changed during loading",
                crate::reload::ReloadScope::All,
            );
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
        let mut problems: Vec<_> = inputs
            .project_errors
            .iter()
            .map(|error| format!(
                "Failed to load Sail project: {error}\nFix the manifest or workspace.projectFile/projectVariables/projectModules settings and reload."
            ))
            .collect();
        if !inputs.source_errors.is_empty() {
            problems.push(format!(
                "Sail project loaded with source file errors; analysis is incomplete.\n{}\nCheck the manifest paths or restore the missing files. The project will reload when they change.",
                inputs.source_errors.join("\n"),
            ));
        }
        self.report_project_error((!problems.is_empty()).then(|| problems.join("\n")));
        self.publish_manifest_diagnostics(inputs.manifest_diagnostics);
        self.log_message(
            MessageType::INFO,
            format!(
                "project reload: {} scanned, {} reused",
                inputs.scanned_projects, inputs.reused_projects
            ),
        );
        self.projects = std::sync::Arc::new(inputs.projects);
        // The model now says what it reads — include paths, files reached
        // outside the folders — so the watch set can follow it.
        self.register_file_watchers();
        self.send_notification::<crate::lsp_ext::ProjectGraphChanged>(());
        self.module_contexts =
            std::sync::Arc::new(crate::reload::loaded_module_contexts(&self.projects));
        // An unloaded project's last check no longer describes anything, and
        // a file a project now lists is checked as part of that project.
        let manifests: std::collections::HashSet<_> =
            self.projects.values().map(|p| p.manifest.clone()).collect();
        let listed: std::collections::HashSet<_> = self
            .projects
            .values()
            .flat_map(|p| p.files.iter().map(|(_, f)| syntax::preprocess::normalize_path(f)))
            .collect();
        let keep = |t: &crate::flycheck::CheckTarget| match t {
            crate::flycheck::CheckTarget::Project(manifest) => manifests.contains(manifest),
            crate::flycheck::CheckTarget::File(path) => {
                !listed.contains(&syntax::preprocess::normalize_path(path))
            }
        };
        self.invalidate_flycheck_targets(|t| !keep(t));
        self.flycheck_history.retain(|t, _| keep(t));
        if self.projects.is_empty() {
            self.log_message(MessageType::WARNING,
                "No Sail project loaded. Set sail-lsp.workspace.projectFile; unlinked files receive syntax diagnostics only.");
        } else {
            let mut manifests: Vec<_> =
                self.projects.values().map(|p| p.manifest.clone()).collect();
            manifests.sort();
            for manifest in manifests {
                self.log_message(
                    MessageType::INFO,
                    format!("Loaded Sail project: {}", manifest.display()),
                );
            }
        }
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
            crate::progress::ProgressReporter::begin(
                &self.sender,
                token,
                "Building index",
                false,
                Some(0),
            )
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
            crate::progress::ProgressReporter::begin(
                &self.sender,
                token,
                "Indexing",
                false,
                Some(0),
            )
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
        self.recheck_flycheck();
    }
    /// Record what a pull report said about its file, the way the push
    /// path's `Task::Diagnostics` arm records a wave. `update_diagnostics`
    /// does not run for a pulling client, so without this the health report
    /// and the exported diagnostic report showed no undecided proofs and no
    /// failures in VS Code, whatever the analysis had met.
    pub(crate) fn record_pull_report(&mut self, report: crate::analysis_health::PullReport) {
        let generation = self.diagnostics.generation();
        if let Some(message) = &report.failure {
            self.log_message(MessageType::ERROR, format!("semanticDiagnostics failed: {message}"));
        }
        // Reasons on the latest report of each file, summed: a pulling
        // client has no wave to read them off.
        if report.reported_unknowns.is_empty() {
            self.pull_unknowns.remove(&report.uri);
        } else {
            self.pull_unknowns.insert(report.uri.clone(), report.reported_unknowns);
        }
        match report.failure {
            Some(message) => {
                self.pull_failures
                    .insert(report.uri, crate::analysis_health::Failure { generation, message });
            }
            None => {
                self.pull_failures.remove(&report.uri);
            }
        }
        let health = std::sync::Arc::make_mut(&mut self.analysis_health);
        if let Some(failure) = self.pull_failures.values().max_by_key(|failure| failure.generation)
        {
            health.failures.insert("semanticDiagnostics".into(), failure.clone());
        } else {
            health.recovered("semanticDiagnostics");
        }
        health.reported_unknowns.clear();
        for (reason, count) in self.pull_unknowns.values().flatten() {
            *health.reported_unknowns.entry(reason.clone()).or_default() += count;
        }
    }

    /// Forget published results and tickets; generations remain monotonic.
    pub(crate) fn invalidate_flycheck(&mut self) {
        self.invalidate_flycheck_targets(|_| true);
    }

    /// Forget the published results and tickets of the targets `drop`
    /// selects, and stop their runs; the others keep running and their
    /// answers are still believed.
    pub(crate) fn invalidate_flycheck_targets(
        &mut self,
        drop: impl Fn(&crate::flycheck::CheckTarget) -> bool,
    ) {
        let dropped: std::collections::BTreeSet<_> = self
            .flycheck_workers
            .keys()
            .chain(self.flycheck_status.keys())
            .chain(self.flycheck_history.keys())
            .chain(self.flycheck_latest.keys())
            .filter(|target| drop(target))
            .cloned()
            .collect();
        for target in &dropped {
            if let Some(worker) = self.flycheck_workers.remove(target) {
                worker.cancel();
            }
            self.flycheck_status.remove(target);
            if let Some(history) = self.flycheck_history.get_mut(target) {
                history.invalidate();
            }
            self.flycheck_latest.remove(target);
            self.end_flycheck_progress(target, "cancelled");
        }
        self.diagnostics.retain_check_targets(|target| !drop(target));
    }

    /// The progress token a run reports under; the generation is what the
    /// client's `window/workDoneProgress/cancel` hands back, and what
    /// [`Self::flycheck_target_for_progress`] resolves to a target.
    fn flycheck_progress_name(generation: u64) -> String {
        format!("flycheck/{generation}")
    }

    /// The target whose run reports under `token`, if it is one of ours and
    /// the run is still the one believed.
    pub(crate) fn flycheck_target_for_progress(
        &self,
        token: &lsp_types::ProgressToken,
    ) -> Option<crate::flycheck::CheckTarget> {
        let lsp_types::ProgressToken::String(token) = token else { return None };
        let generation: u64 = token.strip_prefix("sail-lsp/flycheck/")?.parse().ok()?;
        self.flycheck_latest
            .iter()
            .find(|(_, &g)| g == generation)
            .map(|(target, _)| target.clone())
    }

    /// `target` as the user knows it: relative to its workspace folder.
    fn flycheck_target_label(&self, target: &crate::flycheck::CheckTarget) -> String {
        let path = match target {
            crate::flycheck::CheckTarget::Project(path)
            | crate::flycheck::CheckTarget::File(path) => path,
        };
        self.workspace_folders
            .iter()
            .filter_map(|folder| folder.to_file_path().ok())
            .find_map(|folder| path.strip_prefix(&folder).ok().map(|rel| rel.display().to_string()))
            .unwrap_or_else(|| path.display().to_string())
    }

    /// Start the progress sequence for the run `generation` of `target` —
    /// rust-analyzer's `Checking` progress, titled with what is checked and
    /// cancellable from the client's progress UI. Only for a client that
    /// declared `window.workDoneProgress`; a run that supersedes an earlier
    /// one ends that one's sequence first.
    fn begin_flycheck_progress(&mut self, target: &crate::flycheck::CheckTarget, generation: u64) {
        if !self.supports_progress {
            return;
        }
        self.end_flycheck_progress(target, "superseded");
        let token = self.progress_token(&Self::flycheck_progress_name(generation));
        let title = format!("Checking {}", self.flycheck_target_label(target));
        let reporter =
            crate::progress::ProgressReporter::begin(&self.sender, token, &title, true, None);
        self.flycheck_progress.insert(target.clone(), reporter);
    }

    /// End `target`'s progress sequence, if one is open, saying why.
    pub(crate) fn end_flycheck_progress(
        &mut self,
        target: &crate::flycheck::CheckTarget,
        message: &str,
    ) {
        if let Some(reporter) = self.flycheck_progress.remove(target) {
            reporter.end(message);
        }
    }

    /// Tell the client how long each running check has taken, on the same
    /// tick that refreshes the status line's "Running for Ns".
    pub(crate) fn report_flycheck_progress(&self) {
        for (target, reporter) in &self.flycheck_progress {
            if let Some(running) =
                self.flycheck_history.get(target).and_then(|history| history.running_for())
            {
                reporter.report(&running, None);
            }
        }
    }

    /// Ask upstream Sail what it thinks of the program `saved` belongs to,
    /// if the user turned it on.
    ///
    /// rust-analyzer runs this on save, and so does this: `--just-check`
    /// re-reads whole files from disk, so a run against a buffer the editor
    /// has not written yet would report the previous save. Like its
    /// per-workspace `cargo check`, a run checks the project that lists the
    /// file, with that project's manifest, variables and roots; a file no
    /// project knows is checked alone.
    pub(crate) fn request_flycheck(&mut self, saved: &std::path::Path) {
        self.request_flycheck_targets(self.flycheck_targets_for_file(saved), false);
    }

    fn flycheck_targets_for_file(
        &self,
        saved: &std::path::Path,
    ) -> Vec<crate::flycheck::CheckTarget> {
        use crate::flycheck::CheckTarget;
        let saved = syntax::preprocess::normalize_path(saved);
        if let Some(project) = self
            .projects
            .values()
            .find(|project| syntax::preprocess::normalize_path(&project.manifest) == saved)
        {
            return vec![CheckTarget::Project(project.manifest.clone())];
        }
        let lists = |p: &crate::reload::FolderProject| {
            p.files.iter().any(|(_, f)| syntax::preprocess::normalize_path(f) == saved)
        };
        let mut targets: Vec<_> = self
            .projects
            .values()
            .filter(|p| lists(p))
            .map(|p| CheckTarget::Project(p.manifest.clone()))
            .collect();
        if targets.is_empty() {
            // An `$include`d file belongs to the project that includes it,
            // but no manifest lists it.
            let member = lsp_types::Url::from_file_path(&saved)
                .ok()
                .and_then(|url| self.vfs.lookup_file_id_by_url(&url))
                .is_some_and(|file_id| {
                    base_db::preprocess::project_membership(
                        self.analysis_host.raw_database(),
                        file_id,
                    ) == Some(true)
                });
            targets = if member {
                self.projects.values().map(|p| CheckTarget::Project(p.manifest.clone())).collect()
            } else if crate::reload::is_sail_file(&saved) {
                vec![CheckTarget::File(saved)]
            } else {
                Vec::new()
            };
        }
        targets
    }

    pub(crate) fn recheck_flycheck(&mut self) {
        let mut targets: Vec<_> = self
            .projects
            .values()
            .map(|p| crate::flycheck::CheckTarget::Project(p.manifest.clone()))
            .collect();
        for path in self.mem_docs.iter().filter_map(|uri| uri.to_file_path().ok()) {
            for target in self.flycheck_targets_for_file(&path) {
                if !targets.contains(&target) {
                    targets.push(target);
                }
            }
        }
        // What was asked for while the scan ran — a closed file's save has
        // no other way back in.
        for target in std::mem::take(&mut self.deferred_flycheck_targets) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
        // A run still in flight with a ticket is left to finish: a reload
        // that did not touch a target must not start its check over. The
        // targets a reload or a settings change did touch were invalidated
        // first and have no ticket here.
        targets.retain(|target| {
            !(self.flycheck_latest.contains_key(target)
                && matches!(
                    self.flycheck_status.get(target),
                    Some(crate::flycheck::Status::Queued | crate::flycheck::Status::Running)
                ))
        });
        self.request_flycheck_targets(targets, false);
    }

    fn flycheck_control_targets(
        &self,
        document: Option<&lsp_types::TextDocumentIdentifier>,
    ) -> Vec<crate::flycheck::CheckTarget> {
        let Some(document) = document else {
            let mut targets: Vec<_> = self
                .projects
                .values()
                .map(|project| crate::flycheck::CheckTarget::Project(project.manifest.clone()))
                .collect();
            for path in self.mem_docs.iter().filter_map(|uri| uri.to_file_path().ok()) {
                for target in self.flycheck_targets_for_file(&path) {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
            for target in self
                .flycheck_workers
                .keys()
                .chain(self.flycheck_status.keys())
                .chain(self.flycheck_latest.keys())
            {
                if !targets.contains(target) {
                    targets.push(target.clone());
                }
            }
            return targets;
        };
        document
            .uri
            .to_file_path()
            .map(|path| self.flycheck_targets_for_file(&path))
            .unwrap_or_default()
    }

    pub(crate) fn run_flycheck(&mut self, document: Option<&lsp_types::TextDocumentIdentifier>) {
        let targets = self.flycheck_control_targets(document);
        self.request_flycheck_targets(targets, true);
    }

    pub(crate) fn cancel_flycheck(&mut self, document: Option<&lsp_types::TextDocumentIdentifier>) {
        for target in self.flycheck_control_targets(document) {
            self.cancel_flycheck_target(&target);
        }
    }

    /// Stop `target`'s run and keep what it last published. The scoped
    /// `sail-lsp/cancelFlycheck` and the cancel button on the progress
    /// notification both come here.
    pub(crate) fn cancel_flycheck_target(&mut self, target: &crate::flycheck::CheckTarget) {
        if let Some(history) = self.flycheck_history.get_mut(target) {
            history.invalidate();
        }
        if let Some(worker) = self.flycheck_workers.get(target) {
            worker.cancel();
        }
        // A cancelled process can still finish and enqueue its answer.
        // Removing the ticket makes that answer unobservable.
        if self.flycheck_latest.remove(target).is_some()
            || self.flycheck_status.contains_key(target)
        {
            self.flycheck_status.insert(target.clone(), crate::flycheck::Status::Cancelled);
        }
        self.end_flycheck_progress(target, "cancelled");
    }

    pub(crate) fn clear_flycheck(&mut self, document: Option<&lsp_types::TextDocumentIdentifier>) {
        if document.is_none() {
            self.invalidate_flycheck();
            return;
        }
        let targets = self.flycheck_control_targets(document);
        for target in &targets {
            if let Some(history) = self.flycheck_history.get_mut(target) {
                history.invalidate();
            }
            if let Some(worker) = self.flycheck_workers.remove(target) {
                worker.cancel();
            }
            self.flycheck_latest.remove(target);
            self.flycheck_status.remove(target);
            self.end_flycheck_progress(target, "cleared");
        }
        self.diagnostics.retain_check_targets(|target| !targets.contains(target));
    }

    /// Compiler findings describe disk contents at the last check. Once an
    /// input changes, neither those findings nor an in-flight result describe
    /// the current buffer. Invalidate affected projects, preserving other runs.
    pub(crate) fn invalidate_flycheck_for_source(&mut self, uri: &Url) {
        if self.flycheck_latest.is_empty() && self.diagnostics.check_targets().is_empty() {
            return;
        }
        let Ok(path) = uri.to_file_path() else { return };
        let targets = self.flycheck_targets_for_file(&path);
        for target in &targets {
            if let Some(history) = self.flycheck_history.get_mut(target) {
                history.invalidate();
            }
            if let Some(worker) = self.flycheck_workers.get(target) {
                worker.cancel();
            }
            if self.flycheck_latest.remove(target).is_some()
                || self.flycheck_status.contains_key(target)
            {
                self.flycheck_status.insert(target.clone(), crate::flycheck::Status::Stale);
            }
            self.end_flycheck_progress(target, "source changed");
        }
        self.diagnostics.retain_check_targets(|target| !targets.contains(target));
    }

    fn request_flycheck_targets(
        &mut self,
        targets: Vec<crate::flycheck::CheckTarget>,
        manual: bool,
    ) {
        // A scan is reshaping the projects a check would run against. The
        // request waits for it: `recheck_flycheck` at the scan's end asks
        // for these too. It used to be dropped, so a save racing the scan
        // its own watcher event triggers — every save of an unlisted file on
        // the native watcher path — was checked only if the file was open.
        if self.fetch_workspaces_queue.op_requested()
            || self.fetch_workspaces_queue.op_in_progress()
            || self.loading_project_config.is_some()
        {
            self.deferred_flycheck_targets.extend(targets);
            return;
        }
        // A document is dirty when its buffer is not what is on disk, decided
        // against disk now, when a compiler run is actually being requested.
        // Editors can restore unsaved buffers directly in didOpen, without a
        // didChange, and a disk compiler must not report those as checked;
        // and an edit that was undone leaves a buffer equal to disk, which it
        // may check. `didChange` only ever added to this set, so one undone
        // keystroke held the project's checks back until the file was saved.
        for uri in self.mem_docs.iter() {
            let Some(document) = self.mem_docs.get(uri) else { continue };
            let dirty = uri
                .to_file_path()
                .ok()
                .and_then(|path| std::fs::read_to_string(path).ok())
                .is_none_or(|disk| disk != document.data);
            if dirty {
                self.dirty_documents.insert(uri.clone());
            } else {
                self.dirty_documents.remove(uri);
            }
        }
        for target in targets {
            let config_path = match &target {
                crate::flycheck::CheckTarget::Project(manifest) => self
                    .projects
                    .values()
                    .find(|p| &p.manifest == manifest)
                    .map(|p| &p.workspace_root)
                    .unwrap_or(manifest),
                crate::flycheck::CheckTarget::File(path) => path,
            };
            let effective = Url::from_file_path(config_path)
                .ok()
                .map(|uri| self.config_for_uri(&uri).clone())
                .unwrap_or_else(|| self.config.clone());
            if !manual && !effective.flycheck.enable {
                continue;
            }
            if self.dirty_documents.iter().any(|uri| {
                uri.to_file_path()
                    .ok()
                    .is_some_and(|path| self.flycheck_targets_for_file(&path).contains(&target))
            }) {
                self.flycheck_status.insert(target, crate::flycheck::Status::Stale);
                continue;
            }
            self.flycheck_generation += 1;
            let generation = self.flycheck_generation;
            self.flycheck_latest.insert(target.clone(), generation);
            let workspace = match &target {
                crate::flycheck::CheckTarget::Project(manifest) => self
                    .projects
                    .values()
                    .find(|p| &p.manifest == manifest)
                    .map(|p| &p.config)
                    .unwrap_or(&effective.workspace),
                crate::flycheck::CheckTarget::File(_) => &effective.workspace,
            };
            let root = match &target {
                crate::flycheck::CheckTarget::Project(manifest) => self
                    .projects
                    .values()
                    .find(|p| &p.manifest == manifest)
                    .map(|p| p.workspace_root.clone())
                    .filter(|root| root.is_absolute())
                    .unwrap_or_else(|| manifest.parent().unwrap().to_path_buf()),
                crate::flycheck::CheckTarget::File(file) => file.parent().unwrap().to_path_buf(),
            };
            let includes = crate::reload::include_search_paths(
                &[Url::from_directory_path(&root).unwrap()].into_iter().collect(),
                workspace,
            );
            let invocation = crate::flycheck::command_line_with_context(
                &target,
                workspace,
                &effective.flycheck.extra_args,
                &root,
                &includes,
            );
            let (args, cwd) = match invocation {
                Ok(invocation) => invocation,
                Err(reason) => {
                    self.flycheck_workers.remove(&target);
                    self.flycheck_status.insert(
                        target.clone(),
                        crate::flycheck::Status::Skipped { message: reason.clone() },
                    );
                    self.flycheck_latest.remove(&target);
                    self.end_flycheck_progress(&target, "skipped");
                    self.diagnostics.retain_check_targets(|t| t != &target);
                    self.log_message(
                        lsp_types::MessageType::WARNING,
                        format!("Flycheck skipped for {target:?}: {reason}"),
                    );
                    continue;
                }
            };
            if !self.flycheck_workers.contains_key(&target) {
                match crate::flycheck::Worker::new(
                    target.clone(),
                    self.task_pool.sender(),
                    self.effective_sail_dir.clone(),
                ) {
                    Ok(worker) => {
                        self.flycheck_workers.insert(target.clone(), worker);
                    }
                    Err(error) => {
                        self.flycheck_status.insert(
                            target.clone(),
                            crate::flycheck::Status::Failed { message: error.to_string() },
                        );
                        self.flycheck_latest.remove(&target);
                        self.end_flycheck_progress(&target, "failed");
                        self.diagnostics.retain_check_targets(|t| t != &target);
                        continue;
                    }
                }
            }
            self.flycheck_status.insert(target.clone(), crate::flycheck::Status::Queued);
            self.begin_flycheck_progress(&target, generation);
            self.flycheck_workers.get_mut(&target).unwrap().submit(
                generation,
                &effective.flycheck,
                args,
                cwd,
            );
        }
    }
}

#[allow(clippy::print_stdout, clippy::print_stderr)]
pub fn main() {
    // CLI subcommand dispatch before starting the LSP server.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--version" || arg == "-V") {
        if args.iter().any(|a| a == "--json") {
            println!("{}", crate::version::metadata());
        } else {
            crate::version::print_version();
        }
        return;
    }
    if args.len() >= 2 && !args[1].starts_with('-') {
        let cmd = args[1].as_str();
        let path_arg = args.get(2).map(|s| std::path::Path::new(s.as_str()));
        let result = match cmd {
            "install" => crate::install::run(&args[2..]),
            "doctor" => crate::doctor::run(&args[2..]),
            "fmt" => crate::formatting::cli(&args[2..]),
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
    // Not on the process's main thread: that one has whatever stack the OS
    // gives it — 1 MiB on Windows — and the handlers that run on the loop
    // thread (selection ranges, on-type formatting, join lines, on-enter)
    // parse and recurse like the ones on the 8 MiB pool threads do.
    // rust-analyzer's `main.rs` does the same (`with_extra_thread`).
    with_extra_thread("LspServer", move || run_server(connection));
    io_threads.join().ok();
    eprintln!("[sail-lsp] shutdown");
}

/// Run `f` to completion on a latency-sensitive thread with
/// [`stdx::thread::DEFAULT_STACK_SIZE`] of stack, and wait for it.
fn with_extra_thread<T: Send + 'static>(name: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    stdx::thread::Builder::new(stdx::thread::ThreadIntent::LatencySensitive, name)
        .stack_size(stdx::thread::DEFAULT_STACK_SIZE)
        .spawn(f)
        .expect("failed to spawn the server thread")
        .join()
}

#[cfg(test)]
mod server_thread_tests {
    /// Ten MiB of stack, in frames a debug build cannot shrink: more than
    /// the pool's 8 MiB, less than the 16 MiB the server thread asks for.
    fn recurse(levels: u32) -> u32 {
        let frame = [levels as u8; 64 * 1024];
        std::hint::black_box(&frame);
        if levels == 0 {
            return frame[0] as u32;
        }
        recurse(levels - 1) + 1
    }

    #[test]
    fn the_server_thread_is_named_and_has_a_stack_deep_recursion_fits_in() {
        let (sender, receiver) = crossbeam_channel::bounded(1);
        super::with_extra_thread("LspServer", move || {
            let name = std::thread::current().name().map(str::to_owned);
            sender.send((name, recurse(160))).unwrap();
        });
        assert_eq!(receiver.recv().unwrap(), (Some("LspServer".to_owned()), 160));
    }
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
            version: Some(crate::version::version()),
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
    state.log_message(
        MessageType::INFO,
        format!(
            "sail-lsp {} — executable: {}",
            crate::version::version(),
            std::env::current_exe()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|e| format!("unavailable: {e}"))
        ),
    );
    // Like RA, apply initialization settings before discovering projects.
    // Clients need not send a later didChangeConfiguration notification.
    if let Some(options) = init_params.initialization_options {
        state.apply_client_settings(&options);
    }

    // A running editor always has an explicit project model, even while it
    // is empty/loading. Otherwise detached files enter legacy whole-program
    // analysis before discovery has established any membership.
    state.pending_change.set_source_project(Default::default());
    state.process_changes();

    state.client_caps = client_caps;
    // Asked of the capabilities the client just sent, which is why it reads
    // them after they are stored.
    let supports_progress = state.client_caps.work_done_progress_support();
    state.supports_progress = supports_progress;

    // Discover projects from the client's workspace roots.
    // Initial scan goes through the same fetch_workspaces() path
    // as reloads, ensuring consistent behavior.
    // Zed may initialize with an empty folder list and a valid rootUri.
    // As in RA, an empty list must not discard that explicit root.
    let folders =
        init_params.workspace_folders.filter(|folders| !folders.is_empty()).unwrap_or_else(|| {
            init_params
                .root_uri
                .into_iter()
                .map(|uri| WorkspaceFolder { uri, name: "workspace".into() })
                .collect()
        });
    state.log_message(
        MessageType::INFO,
        format!(
            "Workspace roots from initialize: {}",
            if folders.is_empty() {
                "<none>".to_owned()
            } else {
                folders.iter().map(|folder| folder.uri.as_str()).collect::<Vec<_>>().join(", ")
            },
        ),
    );
    if !folders.is_empty() {
        let mut disk_folders = std::collections::HashSet::new();
        for folder in folders {
            disk_folders.insert(folder.uri);
        }
        state.workspace_folders = disk_folders;
        // Request initial workspace scan via OpQueue.
        state.request_workspace_reload("startup", crate::reload::ReloadScope::All);
        state.request_configuration();
        if state.pending_configuration.is_none() {
            let _ = state.fetch_workspaces_queue.should_start_op();
            let scope = state.take_reload_scope();
            state.fetch_workspaces("startup".into(), scope);
        }
    } else {
        state.finish_workspace_load(&[], 0);
    }

    // Async responses now flow through Task::Response,
    // no separate response channel needed.
    main_loop(&connection, &mut state, &task_receiver);
}

/// Preserve notification/command order while coalescing edits separated by
/// reads that a later edit in this bounded batch has already superseded.
fn on_lsp_batch(state: &mut GlobalState, messages: Vec<Message>) -> (bool, bool) {
    let started = std::time::Instant::now();
    let count = messages.len();
    let mut changed = false;
    let mut skipped = 0;
    let mut messages = messages.into_iter();
    while let Some(message) = messages.next() {
        match message {
            Message::Request(request) => {
                state.register_request(&request);
                let version = crate::request_batch::document_uri(&request)
                    .and_then(|uri| state.document_version(&uri));
                let obsolete = (!state.shutdown_requested)
                    .then(|| {
                        crate::request_batch::superseded(&request, version, messages.as_slice())
                    })
                    .flatten();
                if let Some(code) = obsolete {
                    skipped += 1;
                    state.respond(lsp_server::Response::new_err(
                        request.id,
                        code as i32,
                        "Request superseded by a queued edit or cancellation".into(),
                    ));
                } else {
                    if !state.fetch_workspaces_queue.op_in_progress() {
                        changed |= state.process_changes();
                    }
                    on_request(state, request);
                }
            }
            Message::Notification(notification) if notification.method == "exit" => {
                if !state.shutdown_requested {
                    log::warn!("exit without a preceding shutdown");
                }
                return (false, changed);
            }
            Message::Notification(notification) => {
                if !matches!(
                    notification.method.as_str(),
                    "textDocument/didChange" | "$/cancelRequest"
                ) && !state.fetch_workspaces_queue.op_in_progress()
                {
                    changed |= state.process_changes();
                }
                on_notification(state, notification);
            }
            Message::Response(response) => state.complete_request(response),
        }
    }
    tracing::debug!(target: "sail_lsp::latency",
        "request_batch count={count} skipped={skipped} duration_us={}", started.elapsed().as_micros());
    (true, changed)
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
    let input = crate::request_batch::BufferedMessages::new(connection.receiver.clone());
    let check_status_tick = crossbeam_channel::tick(std::time::Duration::from_secs(1));
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

        let mut batch_changed = false;
        crossbeam_channel::select! {
            // A silent compiler still needs elapsed-time updates. This timer
            // is selected only while a check is queued or running; it starts
            // no analysis work and does not wake an idle server.
            recv(if state.flycheck_status.values().any(|status| status.busy()) {
                check_status_tick.clone()
            } else { crossbeam_channel::never() }) -> _ => {
                state.report_flycheck_progress();
            },
            recv(state.pending_configuration.as_ref().map(|pending| crossbeam_channel::after(pending.deadline.saturating_duration_since(std::time::Instant::now()))).unwrap_or_else(crossbeam_channel::never)) -> _ => {
                state.configuration_timed_out();
            }
            recv(input.receiver) -> msg => {
                let msg = match msg {
                    Ok(msg) => msg,
                    Err(crossbeam_channel::RecvError) => return,
                };
                let mut batch = vec![msg];
                batch.extend(input.receiver.try_iter().take(crate::request_batch::LIMIT - 1));
                let (keep_running, changed) = on_lsp_batch(state, batch);
                batch_changed = changed;
                if !keep_running {
                    return;
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

        let drain_started = std::time::Instant::now();
        let mut drained = 0usize;
        for task in task_receiver.try_iter().take(crate::request_batch::LIMIT) {
            on_task(state, task);
            drained += 1;
        }
        if drained != 0 {
            tracing::debug!(target: "sail_lsp::latency",
                "task_drain count={drained} duration_us={}", drain_started.elapsed().as_micros());
        }

        // Sail: fetch_workspaces_queue completion is equivalent to vfs_done.
        let (state_changed, memdocs_added_or_removed) =
            if !state.fetch_workspaces_queue.op_in_progress() {
                (state.process_changes(), state.mem_docs.take_changes())
            } else {
                (false, false)
            };

        let state_changed = state_changed || batch_changed;
        if state.is_quiescent() {
            let became_quiescent = !was_quiescent;

            // The workspace symbol index used to be rebuilt here, from a
            // dirty-file list, by cloning the whole map. It is
            // `ide_db::db_query::workspace_symbol_index` now — a salsa
            // query keyed by the workspace's files, so an unchanged file's
            // symbols are reused and nothing is cloned per edit.

            let diagnostics_refresh = std::mem::take(&mut state.diagnostics_refresh_requested);
            let project_or_mem_docs_changed = became_quiescent
                || state_changed
                || memdocs_added_or_removed
                || diagnostics_refresh;

            if became_quiescent {
                state.prime_caches_queue.request_op("became quiescent".to_owned(), ());
            }

            let client_refresh = became_quiescent || state_changed || diagnostics_refresh;
            let semantic_tokens_refresh =
                std::mem::take(&mut state.semantic_tokens_refresh_requested);
            if client_refresh || semantic_tokens_refresh {
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
            }
            if client_refresh {
                state.send_request::<lsp_types::request::CodeLensRefresh>((), noop_response);
                state
                    .send_request::<lsp_types::request::InlayHintRefreshRequest>((), noop_response);
                // A pulling client is told to pull again rather than sent
                // the answer: what changed may be a file that `$include`s
                // the edited one. Sent without checking `refreshSupport`,
                // like the three above: lsp-types 0.94 reads it from
                // `workspace.diagnostic`, where the protocol (and VS Code)
                // put it at `workspace.diagnostics`, so it never reads true.
                if state.client_caps.pull_diagnostics_support() {
                    state.send_request::<lsp_types::request::WorkspaceDiagnosticRefresh>(
                        (),
                        noop_response,
                    );
                }
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
            state.collect_caches_if_idle();

            // A client that pulls gets the server's own findings from
            // `textDocument/diagnostic`; pushing them as well shows each
            // twice. Only flycheck's, which no pull returns, are published
            // to it — rust-analyzer's rule (`main_loop.rs`,
            // `!self.config.text_document_diagnostic()`).
            if project_or_mem_docs_changed && !state.client_caps.pull_diagnostics_support() {
                state.update_diagnostics();
            }
        }

        if state.pending_configuration.is_none() {
            if let Some((cause, ())) = state.fetch_workspaces_queue.should_start_op() {
                let scope = state.take_reload_scope();
                state.fetch_workspaces(cause, scope);
            }
        }

        if let Some((_cause, ())) = state.prime_caches_queue.should_start_op() {
            let analysis = state.analysis_snapshot();
            let cancel = state.prime_caches_cancel.clone();
            let num_worker_threads = state.config.prime_caches_num_threads();
            let prime_imports = state.config.completion.autoimport;
            state.task_pool.spawn_with_sender(stdx::thread::ThreadIntent::Worker, move |sender| {
                let _ = sender.send(Task::PrimeCaches(PrimeCachesProgress::Begin));
                // Through `Analysis`, with the worker count the client set
                // and a progress callback that reaches the client — a
                // literal `2` and `&|_| {}` stood here, and the phase that
                // dominates startup reported nothing while it ran.
                let report = sender.clone();
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if prime_imports {
                        analysis.prime_import_catalog(&cancel)?;
                    }
                    analysis.parallel_prime_caches(num_worker_threads, &cancel, move |p| {
                        let _ = report.send(Task::PrimeCaches(PrimeCachesProgress::Report(p)));
                    })
                }));
                // Priming's temporary index workers have finished.
                // Reclaim free allocator pages on this worker, never on the
                // event loop or on each interactive request.
                drop(analysis);
                profile::release_unused_memory();
                let _ = sender.send(Task::PrimeCaches(PrimeCachesProgress::End {
                    cancelled: cancel.is_cancelled() || matches!(&outcome, Ok(Err(_))),
                    failed: outcome.is_err(),
                }));
            });
        }

        if let Some(diagnostic_changes) = state.diagnostics.take_changes() {
            for file_id in diagnostic_changes {
                if let Some(url) = state.vfs.lookup_url(file_id) {
                    let mut diagnostics: Vec<lsp_types::Diagnostic> =
                        state.diagnostics.diagnostics_for(file_id).collect();
                    crate::diagnostics::explain_unlinked_file(
                        &mut diagnostics,
                        state.project_error.as_deref(),
                    );
                    // The version of the buffer these are about, so a client
                    // can drop what it already knows is stale. `None` says
                    // "no idea", which is the one answer it cannot act on —
                    // and `MemDocs::get`, the only way to answer, carried an
                    // `#[expect(dead_code)]`.
                    let version = state
                        .diagnostics
                        .native_diagnostics_are_current(file_id)
                        .then(|| state.mem_docs.get(url).map(|doc| doc.version))
                        .flatten();
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
        Task::EditorDocument { source, result } => {
            crate::editor_commands::present(state, source, result)
        }
        Task::WorkspacePreprocessed { revision, load_version, result } => {
            state.preprocessing_running = false;
            if revision != state.fetch_workspaces_queue.revision()
                && state
                    .pending_preprocessing
                    .as_ref()
                    .is_some_and(|pending| pending.revision == revision)
            {
                // The fetch operation includes preprocessing. Release the
                // obsolete operation so its queued replacement can start;
                // otherwise a reload requested during planning waits forever.
                state.pending_preprocessing = None;
                state.loading_source_project = None;
                state.loading_module_graph = None;
                state.loading_project_config = None;
                if state.fetch_workspaces_queue.op_in_progress() {
                    state.fetch_workspaces_queue.op_completed(());
                }
                return;
            }
            // Process overlay changes received during the computation before
            // checking the exact source/configuration generation.
            state.process_changes();
            let db = state.analysis_host.raw_database();
            let current = revision == state.fetch_workspaces_queue.revision()
                && load_version == state.workspace_load_version;
            match result {
                Ok(prepared)
                    if current
                        && state.loading_source_project.as_ref() == Some(&prepared.project)
                        && prepared.strict_bitvector == base_db::strict_bitvector(db)
                        && prepared.projections.iter().all(|(id, source)| {
                            db.files()
                                .file_text(*id)
                                .is_some_and(|file| *file.text(db) == source.text)
                        }) =>
                {
                    if let Some(pending) = state.pending_preprocessing.take() {
                        state.pending_change.set_prepared_workspace(prepared);
                        state.finish_workspace_load_inner(&pending.files, pending.count);
                    }
                }
                Err(error) if current => {
                    state.pending_preprocessing = None;
                    state.loading_source_project = None;
                    state.loading_module_graph = None;
                    state.loading_project_config = None;
                    state.report_project_error(Some(error));
                    if state.fetch_workspaces_queue.op_in_progress() {
                        state.fetch_workspaces_queue.op_completed(());
                    }
                }
                Ok(prepared) => state.start_workspace_preprocessing(Some(prepared)),
                Err(_) => state.start_workspace_preprocessing(None),
            }
        }
        Task::WorkspaceScan(scan) => {
            if !scan.inputs.matches_disk() {
                state.request_workspace_reload(
                    "manifest changed during discovery",
                    crate::reload::ReloadScope::All,
                );
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
            state.pending_preprocessing = None;
            let files = match scan.files {
                Ok(files) => files,
                Err(error) => {
                    state.publish_manifest_diagnostics(scan.inputs.manifest_diagnostics);
                    state.discovery_watch_files.extend(scan.inputs.watch_files);
                    state.discovery_watch_files.sort();
                    state.discovery_watch_files.dedup();
                    state.request_workspace_load(&[]);
                    let context = if state.projects.is_empty() {
                        "No project model is available; unlinked files receive syntax diagnostics only."
                    } else {
                        "Keeping the previous project model; project configuration may be out of date."
                    };
                    state.report_project_error(Some(format!(
                        "Failed to load Sail project: {error}\n{context}\nFix the manifest or workspace.projectFile/projectVariables/projectModules settings and reload."
                    )));
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
                    module_options: crate::reload::loaded_module_contexts(&scan.inputs.projects)
                        .iter()
                        .enumerate()
                        .filter_map(|(index, context)| {
                            let manifest = Url::parse(context["manifest"].as_str()?)
                                .ok()?
                                .to_file_path()
                                .ok()?;
                            let project =
                                scan.inputs.projects.values().find(|p| p.manifest == manifest)?;
                            let folders = [Url::from_directory_path(&project.workspace_root).ok()?]
                                .into_iter()
                                .collect();
                            Some((
                                index,
                                base_db::preprocess::ModuleOptions {
                                    project: Some(manifest),
                                    include_paths: crate::reload::include_search_paths(
                                        &folders,
                                        &project.config,
                                    ),
                                    target: project.config.target.clone(),
                                    strict_bitvector: project.config.strict_bitvector,
                                },
                            ))
                        })
                        .collect(),
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
            // Client cancellation may have completed this request while its
            // worker was running. Every request gets exactly one response.
            if !state.is_completed(&response.id) {
                if response.error.is_none() && state.request_document_changed(&response.id) {
                    state.respond(lsp_server::Response::new_err(
                        response.id,
                        lsp_server::ErrorCode::ServerCancelled as i32,
                        "Document changed while this request was running; request again".into(),
                    ));
                } else {
                    state.respond(response);
                }
            }
        }
        Task::Retry(req) => {
            // Retry with the current snapshot only while the client still
            // wants the request. A queued retry must not revive cancellation.
            if !state.is_completed(&req.id) {
                if state.can_retry(&req.id) {
                    on_request(state, req);
                } else {
                    state.respond(lsp_server::Response::new_err(
                        req.id,
                        lsp_server::ErrorCode::ServerCancelled as i32,
                        "Document changed or analysis retry limit reached; request again".into(),
                    ));
                }
            }
        }
        Task::FlycheckStarted { target, generation, cache } => {
            if state.flycheck_latest.get(&target) == Some(&generation) {
                state.flycheck_history.entry(target.clone()).or_default().start(cache);
                state.flycheck_status.insert(target, crate::flycheck::Status::Running);
                // The debounce is over and the compiler is running: the
                // first "Running for 0s." — the tick carries on from here.
                state.report_flycheck_progress();
            }
        }
        Task::Flycheck { target, generation, found, status, run } => {
            // The ticket decides. A scan takes the tickets of the projects it
            // rescans when it starts (`fetch_workspaces`); a target that kept
            // its ticket through a scan was not touched by it, and its
            // answer stands. Dropping every answer that landed during a scan
            // left the target `Running` forever, with nothing to re-run it.
            if state.flycheck_latest.get(&target) != Some(&generation) {
                return;
            }
            if found.iter().any(|(path, diagnostic)| {
                Url::from_file_path(path).ok().and_then(|uri| state.mem_docs.get(&uri)).is_some_and(
                    |document| {
                        !crate::flycheck::structured::matches_source(diagnostic, &document.data)
                    },
                )
            }) {
                state.flycheck_status.insert(target.clone(), crate::flycheck::Status::Stale);
                if let Some(history) = state.flycheck_history.get_mut(&target) {
                    history.invalidate();
                }
                state.diagnostics.set_check_diagnostics(&target, Vec::new());
                state.end_flycheck_progress(&target, "source changed");
                return;
            }
            if status.problem() {
                state.log_message(MessageType::WARNING, format!("Flycheck {target:?}: {status:?}"));
            }
            state.end_flycheck_progress(&target, &status.to_string());
            state.flycheck_history.entry(target.clone()).or_default().finish(run, &status);
            state.flycheck_status.insert(target.clone(), status);
            // Terminal locations require a known VFS path. Structured spans
            // carry validated physical paths and hashes, including unopened files.
            let mut by_file: HashMap<base_db::FileId, Vec<lsp_types::Diagnostic>> = HashMap::new();
            for (path, diag) in found {
                let Ok(url) = lsp_types::Url::from_file_path(&path) else { continue };
                let fid = match state.vfs.lookup_file_id_by_url(&url) {
                    Some(fid) => fid,
                    None if diag
                        .data
                        .as_ref()
                        .and_then(|data| data.get("sailCompiler"))
                        .is_some() =>
                    {
                        state.vfs.file_id_for_url(&url)
                    }
                    None => continue,
                };
                by_file.entry(fid).or_default().push(diag);
            }
            let policy = state.compiler_policy_for(&target);
            state.diagnostics.set_check_policy(&target, policy);
            state.diagnostics.set_check_diagnostics(&target, by_file.into_iter().collect());
            state.apply_compiler_diagnostic_policies();
        }
        Task::PullDiagnosticsHealth(report) => state.record_pull_report(report),
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
                        Some(0),
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
            PrimeCachesProgress::End { cancelled, failed } => {
                if let Some(reporter) = state.prime_caches_progress.take() {
                    reporter.end(if failed {
                        "failed"
                    } else if cancelled {
                        "cancelled"
                    } else {
                        "done"
                    });
                }
                state.cancellable_progress = None;
                // No eviction here. It was the *second* call site, and the
                // unguarded one: priming ends while the diagnostics wave
                // spawned by the same settling change is still computing, so
                // this cancelled it every time a file was opened. The loop's
                // one guarded call is enough — it fires as soon as the pool
                // is idle and the nonce has moved.
                state.prime_caches_queue.op_completed(());
                if !failed && !cancelled {
                    std::sync::Arc::make_mut(&mut state.analysis_health).recovered("cachePriming");
                }
                if failed {
                    std::sync::Arc::make_mut(&mut state.analysis_health).failed(
                        "cachePriming",
                        0,
                        "Cache priming failed; on-demand analysis remains available".into(),
                    );
                    state.log_message(lsp_types::MessageType::ERROR,
                        "Cache priming failed; analysis remains available on demand. Reload the workspace to retry.");
                } else if cancelled {
                    std::sync::Arc::make_mut(&mut state.analysis_health).cancelled_waves += 1;
                    state.prime_caches_queue.request_op("restart after cancellation".into(), ());
                }
            }
        },
        Task::Diagnostics(task_kind) => {
            let (kind, generation, diags) = match task_kind {
                DiagnosticsTaskKind::Syntax(gen, diags) => {
                    (NativeDiagnosticsFetchKind::Syntax, gen, diags)
                }
                DiagnosticsTaskKind::Semantic(gen, diags) => {
                    (NativeDiagnosticsFetchKind::Semantic, gen, diags)
                }
                DiagnosticsTaskKind::Unavailable { kind, generation, files, failure } => {
                    if state.diagnostics.is_current_generation(generation) {
                        let phase = match kind {
                            NativeDiagnosticsFetchKind::Syntax => "syntaxDiagnostics",
                            NativeDiagnosticsFetchKind::Semantic => "semanticDiagnostics",
                        };
                        if let Some(message) = failure {
                            state.log_message(
                                MessageType::ERROR,
                                format!("{phase} failed: {message}"),
                            );
                            std::sync::Arc::make_mut(&mut state.analysis_health)
                                .failed(phase, generation, message);
                            state.diagnostics.set_native_diagnostics(
                                kind,
                                generation,
                                files.into_iter().map(|id| (id, Vec::new())).collect(),
                            );
                        } else {
                            std::sync::Arc::make_mut(&mut state.analysis_health).cancelled_waves +=
                                1;
                        }
                    }
                    return;
                }
            };
            if !state.diagnostics.is_current_generation(generation) {
                return;
            }
            let phase = match kind {
                NativeDiagnosticsFetchKind::Syntax => "syntaxDiagnostics",
                NativeDiagnosticsFetchKind::Semantic => "semanticDiagnostics",
            };
            let health = std::sync::Arc::make_mut(&mut state.analysis_health);
            health.recovered(phase);
            if matches!(kind, NativeDiagnosticsFetchKind::Semantic) {
                health.reported_unknowns.clear();
                for (_, diagnostics) in &diags {
                    for diagnostic in diagnostics {
                        if let Some(reason) = diagnostic
                            .data
                            .as_ref()
                            .and_then(|d| d.get("analysis"))
                            .and_then(|d| d.get("reason"))
                            .and_then(serde_json::Value::as_str)
                        {
                            *health.reported_unknowns.entry(reason.to_string()).or_default() += 1;
                        }
                    }
                }
            }
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
        state.invalidate_flycheck();
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
        .on_sync_mut::<lsp_types::request::ExecuteCommand>(crate::editor_commands::execute)
        // This only serializes loaded project metadata. Reading it directly
        // avoids constructing an analysis snapshot while cache priming owns
        // Salsa state during startup.
        .on_sync_mut::<crate::lsp_ext::IncludeContexts>(handlers::handle_include_contexts)
        .on_sync_mut::<crate::lsp_ext::SelectIncludeContext>(
            handlers::handle_select_include_context,
        )
        .on_sync_mut::<crate::lsp_ext::ProjectGraph>(handlers::handle_project_graph_sync)
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
        // Priority and cancellation retry policy are selected independently.
        .on_latency_sensitive::<true, request::Completion>(handlers::handle_completion)
        .on_latency_sensitive::<true, request::ResolveCompletionItem>(
            handlers::handle_resolve_completion_item,
        )
        .on_latency_sensitive::<true, request::InlayHintRequest>(handlers::handle_inlay_hint)
        .on::<true, crate::lsp_ext::ExplainAnalysis>(handlers::handle_explain_analysis)
        .on::<false, crate::lsp_ext::ExplainFormatting>(crate::formatting::explain)
        .on::<false, crate::lsp_ext::SsrPreview>(handlers::handle_ssr_preview)
        .on::<false, crate::lsp_ext::Ssr>(handlers::handle_ssr)
        .on::<false, crate::lsp_ext::SsrSearch>(handlers::handle_ssr_search)
        .on_latency_sensitive::<true, request::SignatureHelpRequest>(
            handlers::handle_signature_help,
        )
        .on_latency_sensitive::<true, crate::hover_actions::HoverRequest>(handlers::handle_hover)
        // Semantic tokens are redrawn on every edit, so a cancelled one is
        // worth retrying rather than reporting — RA marks the same three
        // latency-sensitive, and the range request no-retry.
        .on_latency_sensitive::<true, request::SemanticTokensFullRequest>(
            handlers::handle_semantic_tokens_full,
        )
        .on_latency_sensitive::<true, request::SemanticTokensFullDeltaRequest>(
            handlers::handle_semantic_tokens_full_delta,
        )
        .on_latency_sensitive::<false, request::SemanticTokensRangeRequest>(
            handlers::handle_semantic_tokens_range,
        )
        .on_latency_sensitive::<true, crate::lsp::ext::CodeActionResolveRequest>(
            handlers::handle_code_action_resolve,
        )
        .on::<false, request::DocumentSymbolRequest>(handlers::handle_document_symbol)
        .on_reporting::<false, request::DocumentDiagnosticRequest>(
            handlers::handle_document_diagnostic,
        )
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
        .on::<true, crate::lsp_ext::ExpandInclude>(handlers::handle_expand_include)
        .on::<false, crate::lsp_ext::SailLspStatus>(handlers::handle_sail_lsp_status)
        .on::<false, crate::lsp_ext::DiagnosticReport>(handlers::handle_diagnostic_report)
        .on::<false, crate::lsp_ext::OpenProjectManifest>(handlers::handle_open_project_manifest)
        .on::<false, crate::lsp_ext::ParentModule>(handlers::handle_parent_module)
        .on::<false, crate::lsp_ext::Runnables>(handlers::handle_runnables)
        .on::<false, crate::lsp_ext::ViewIncludeGraph>(handlers::handle_view_include_graph)
        .on::<false, crate::lsp_ext::MoveItem>(handlers::handle_move_item)
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
        .on_sync_mut::<crate::lsp_ext::Recheck>(handlers::handle_recheck)
        .on_sync_mut::<crate::lsp_ext::RunFlycheck>(handlers::handle_run_flycheck)
        .on_sync_mut::<crate::lsp_ext::CancelFlycheck>(handlers::handle_cancel_flycheck)
        .on_sync_mut::<crate::lsp_ext::ClearFlycheck>(handlers::handle_clear_flycheck)
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

    #[test]
    fn cancelled_preprocessing_unwinds_and_releases_an_isolated_database() {
        for cancel_before_creation in [true, false] {
            let cancel = hir::CancellationToken::new();
            if cancel_before_creation {
                cancel.cancel();
            }
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let db = preprocessing_database(cancel.clone());
                let id = base_db::FileId::from_raw(0);
                let file = base_db::FileText::new(
                    &db,
                    std::sync::Arc::from("$define READY\nfunction test() = 1\n"),
                    id,
                );
                let project = base_db::preprocess::SourceProject {
                    roots: vec![id],
                    paths: vec![("/isolated.sail".into(), id)],
                    ..Default::default()
                };
                let input = base_db::preprocess::WorkspaceSourceProject::new(
                    &db,
                    project,
                    [(id, file)].into(),
                );
                cancel.cancel();
                hir::preprocess::prepare_workspace(&db, input)
            }));
            assert!(result.unwrap_err().is::<stdx::cancellation::Cancelled>());
        }
        assert!(syntax::SourceFile::parse("function recovered() = 1\n").errors().is_empty());
    }

    #[test]
    fn large_workspace_preprocessing_is_background_and_rejects_obsolete_overlays() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.config.flycheck.enable = false;
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<_> = (0..65)
            .map(|index| {
                let path = dir.path().join(format!("file_{index}.sail"));
                (Url::from_file_path(&path).unwrap(), path)
            })
            .collect();
        for (index, (url, _)) in files.iter().enumerate() {
            let text = if index == 0 {
                "$include \"file_1.sail\"\n".to_owned()
            } else {
                format!("val file_{index} : int\n")
            };
            state.sync_disk_text(url, &text);
        }
        state.process_changes();
        let ids: Vec<_> =
            files.iter().map(|(url, _)| state.vfs.lookup_file_id_by_url(url).unwrap()).collect();
        state.loading_source_project = Some(base_db::preprocess::SourceProject {
            roots: vec![ids[0]],
            paths: files.iter().zip(&ids).map(|((_, path), id)| (path.clone(), *id)).collect(),
            ..Default::default()
        });
        state.finish_workspace_load(&files, files.len());
        assert!(state.preprocessing_running);
        assert!(!state.workspace_loaded, "foreground must return before the model is installed");
        let obsolete = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        // Watch registration may echo files before the new model has been
        // installed. Such events must update its sources without requesting
        // another scan, which would otherwise starve startup indefinitely.
        let revision = state.fetch_workspaces_queue.revision();
        state.handle_vfs_msg(vfs::loader::Message::Changed {
            files: vec![(
                paths::AbsPathBuf::assert_utf8(files[0].1.clone()),
                Some(b"$include \"file_1.sail\"\n".to_vec()),
            )],
        });
        assert_eq!(state.fetch_workspaces_queue.revision(), revision);
        assert!(!state.fetch_workspaces_queue.op_requested());
        // A newer overlay arrives while the old model is being derived. It
        // must remain authoritative even when the old completion arrives late.
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: files[0].0.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: "$include \"file_2.sail\"\n".into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        on_task(&mut state, obsolete);
        assert!(!state.workspace_loaded);
        assert!(state.preprocessing_running, "only the latest input is rescheduled");
        let current = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        on_task(&mut state, current);
        assert!(state.workspace_loaded);
        assert!(!state.preprocessing_running);
        assert!(state.pending_preprocessing.is_none());
        let db = state.analysis_host.raw_database();
        let project = base_db::preprocess::WorkspaceSourceProject::try_get(db).unwrap();
        let plan = hir::preprocess::workspace_plan(db, project);
        assert!(plan.edges.contains(&(ids[0], ids[2])));
        assert!(!plan.edges.contains(&(ids[0], ids[1])));
        assert_eq!(db.live_snapshots(), 0, "independent workers must not retain host snapshots");
        let project = project.project(db).clone();
        state.fetch_workspaces_queue.request_op("reload model".into(), ());
        state.fetch_workspaces_queue.should_start_op().unwrap();
        state.loading_source_project = Some(project);
        state.finish_workspace_load(&files, files.len());
        let old = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        state.fetch_workspaces_queue.request_op("newer configuration".into(), ());
        on_task(&mut state, old);
        assert!(!state.preprocessing_running);
        assert!(state.pending_preprocessing.is_none());
        assert!(
            !state.fetch_workspaces_queue.op_in_progress(),
            "obsolete planning must release the fetch operation"
        );
        assert!(state.fetch_workspaces_queue.op_requested(), "the newer reload must remain queued");
    }

    #[test]
    fn eviction_waits_for_the_pool_and_the_next_wave_publishes_its_findings() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let dir = tempfile::tempdir().unwrap();
        let url = Url::from_file_path(dir.path().join("bad.sail")).unwrap();
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: url.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: "function bad() -> int = true\n".into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        state.config.cache.inference_capacity = 1;
        let retained = state.analysis_host.db_snapshot();
        let (release, blocked) = crossbeam_channel::bounded::<()>(0);
        state.task_pool.spawn(stdx::thread::ThreadIntent::Worker, move || {
            blocked.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            drop(retained);
            Task::Response(lsp_server::Response::new_ok(0.into(), ()))
        });
        assert!(!state.task_pool.is_empty());
        assert!(!state.collect_caches_if_idle());
        release.send(()).unwrap();
        let _ = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !state.task_pool.is_empty() {
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(state.collect_caches_if_idle());
        state.update_diagnostics();
        for _ in 0..2 {
            let task = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            assert!(matches!(task, Task::Diagnostics(_)));
            on_task(&mut state, task);
        }
        let file = state.vfs.file_id_for_url(&url);
        assert!(
            state
                .diagnostics
                .diagnostics_for(file)
                .any(|d| d.severity == Some(lsp_types::DiagnosticSeverity::ERROR)),
            "collection must leave the following diagnostics wave operational"
        );
        assert!(state.analysis_health.failures.is_empty());
    }

    /// A pulling client gets the same health record as a pushing one.
    ///
    /// `reported_unknowns` and the `semanticDiagnostics` failure entry were
    /// written only by the push wave's task, and `update_diagnostics` does
    /// not run for a client that pulls, so "native analysis health" and the
    /// exported report were always empty in VS Code. The pull handler now
    /// returns the facts beside its report, the dispatcher sends them as a
    /// task ahead of the response, and the main loop records them.
    #[test]
    fn a_pull_report_records_analysis_health() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let dir = tempfile::tempdir().unwrap();
        let url = Url::from_file_path(dir.path().join("fine.sail")).unwrap();
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: url.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: "function fine() -> int = 1\n".into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        let file = state.vfs.file_id_for_url(&url);
        state.ensure_workspace_files(vec![file]);
        // A failure on record, which a successful semantic pass clears — on
        // the push path through its wave, here through the pull.
        std::sync::Arc::make_mut(&mut state.analysis_health).failed(
            "semanticDiagnostics",
            0,
            "inference fault".into(),
        );

        let request = lsp_server::Request::new(
            1.into(),
            "textDocument/diagnostic".into(),
            serde_json::json!({ "textDocument": { "uri": url } }),
        );
        state.register_request(&request);
        on_request(&mut state, request);
        let mut answered = false;
        while !answered {
            let task = tasks.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
            answered = matches!(task, Task::Response(_));
            on_task(&mut state, task);
        }
        assert!(
            state.analysis_health.failures.is_empty(),
            "a successful pull is a successful semantic pass: {:?}",
            state.analysis_health.failures
        );
        assert!(state.analysis_health.reported_unknowns.is_empty());

        // The reasons are summed over the latest report of each file.
        let other = Url::parse("file:///w/other.sail").unwrap();
        let report = |uri: &Url, reasons: &[(&str, usize)], failure: Option<&str>| {
            Task::PullDiagnosticsHealth(crate::analysis_health::PullReport {
                uri: uri.clone(),
                reported_unknowns: reasons.iter().map(|(r, n)| (r.to_string(), *n)).collect(),
                failure: failure.map(str::to_owned),
            })
        };
        on_task(&mut state, report(&url, &[("not_established", 2)], None));
        on_task(
            &mut state,
            report(&other, &[("not_established", 1), ("budget_exhausted", 1)], None),
        );
        let unknowns = |state: &GlobalState| {
            serde_json::to_value(&state.analysis_health.reported_unknowns).unwrap()
        };
        assert_eq!(
            unknowns(&state),
            serde_json::json!({ "not_established": 3, "budget_exhausted": 1 })
        );
        // A later report of a file replaces that file's share.
        on_task(&mut state, report(&url, &[], None));
        assert_eq!(
            unknowns(&state),
            serde_json::json!({ "not_established": 1, "budget_exhausted": 1 })
        );

        // A failed pass is recorded and logged, as the push path does.
        on_task(&mut state, report(&other, &[], Some("inference fault")));
        assert_eq!(
            state.analysis_health.failures["semanticDiagnostics"].message,
            "inference fault"
        );
        let logged = messages.try_iter().map(|m| serde_json::to_value(m).unwrap()).any(|m| {
            m["method"] == "window/logMessage"
                && m["params"]["message"]
                    .as_str()
                    .is_some_and(|s| s.contains("semanticDiagnostics failed: inference fault"))
        });
        assert!(logged, "the failure reaches the client's log");
        on_task(&mut state, report(&url, &[], None));
        assert_eq!(
            state.analysis_health.failures["semanticDiagnostics"].message, "inference fault",
            "a clean file cannot recover another file's failure"
        );
        on_task(&mut state, report(&url, &[], Some("another inference fault")));
        on_task(&mut state, report(&other, &[], None));
        assert_eq!(
            state.analysis_health.failures["semanticDiagnostics"].message,
            "another inference fault",
            "recovering one of two failed files must keep the other failure"
        );
        on_task(&mut state, report(&url, &[], None));
        assert!(state.analysis_health.failures.is_empty());
    }

    #[test]
    fn rename_keeps_legacy_clients_compatible_with_unversioned_workspace_edits() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let url = Url::from_file_path(std::env::temp_dir().join("legacy-rename.sail")).unwrap();
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: url.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: "function f(x : int) -> int = x\n".into(),
                },
            },
        )
        .unwrap();
        state.process_changes();
        let file = state.vfs.file_id_for_url(&url);
        state.ensure_workspace_files(vec![file]);
        assert!(!state.client_caps.versioned_edit_support());
        let edit = crate::handlers::request::handle_rename(
            state.snapshot(),
            lsp_types::RenameParams {
                text_document_position: lsp_types::TextDocumentPositionParams {
                    text_document: lsp_types::TextDocumentIdentifier { uri: url.clone() },
                    position: lsp_types::Position::new(0, 11),
                },
                new_name: "arg".into(),
                work_done_progress_params: Default::default(),
            },
        )
        .unwrap()
        .unwrap();
        assert!(edit.document_changes.is_none());
        assert_eq!(edit.changes.unwrap()[&url].len(), 2);
    }

    /// `project:` names a project — its manifest file, with or without the
    /// extension, or the directory holding it — rather than matching a
    /// substring anywhere in the manifest's path, where `project:riscv`
    /// also selected every project living under a `riscv/` tree.
    #[test]
    fn workspace_symbol_project_filter_names_a_project() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let root =
            std::env::temp_dir().join(format!("symbol-project-filter-{}", std::process::id()));
        let core = root.join("riscv").join("sail.proj");
        let ext = root.join("riscv").join("vendor").join("ext.sail_project");
        let a = root.join("riscv").join("a.sail");
        let b = root.join("riscv").join("vendor").join("b.sail");
        let mut urls = Vec::new();
        for (path, text) in
            [(&a, "function f_core() -> int = 1\n"), (&b, "function f_ext() -> int = 2\n")]
        {
            let url = Url::from_file_path(path).unwrap();
            crate::handlers::notification::handle_did_open_text_document(
                &mut state,
                lsp_types::DidOpenTextDocumentParams {
                    text_document: lsp_types::TextDocumentItem {
                        uri: url.clone(),
                        language_id: "sail".into(),
                        version: 1,
                        text: text.into(),
                    },
                },
            )
            .unwrap();
            urls.push(url);
        }
        state.process_changes();
        let ids = urls.iter().map(|url| state.vfs.file_id_for_url(url)).collect();
        state.ensure_workspace_files(ids);
        let projects = std::sync::Arc::make_mut(&mut state.projects);
        projects.insert(
            Url::from_directory_path(root.join("riscv")).unwrap(),
            crate::reload::FolderProject {
                manifest: core,
                discovered_files: vec![(urls[0].clone(), a)],
                ..Default::default()
            },
        );
        projects.insert(
            Url::from_directory_path(root.join("riscv").join("vendor")).unwrap(),
            crate::reload::FolderProject {
                manifest: ext,
                discovered_files: vec![(urls[1].clone(), b)],
                ..Default::default()
            },
        );
        let names = |state: &mut GlobalState, query: &str| -> Vec<String> {
            let found = crate::handlers::request::handle_workspace_symbol(
                state.snapshot(),
                lsp_types::WorkspaceSymbolParams { query: query.into(), ..Default::default() },
            )
            .unwrap();
            match found {
                Some(lsp_types::WorkspaceSymbolResponse::Flat(symbols)) => {
                    symbols.into_iter().map(|s| s.name).collect()
                }
                _ => Vec::new(),
            }
        };
        assert_eq!(names(&mut state, "f_"), ["f_core", "f_ext"], "both projects are indexed");
        assert_eq!(names(&mut state, "f_ project:riscv"), ["f_core"]);
        assert_eq!(names(&mut state, "f_ project:ext"), ["f_ext"]);
        assert_eq!(names(&mut state, "f_ project:ext.sail_project"), ["f_ext"]);
        assert_eq!(names(&mut state, "f_ project:vendor"), ["f_ext"]);
        assert!(names(&mut state, "f_ project:ris").is_empty(), "a name, not a substring");
    }

    /// A reload keeps the previous model when a manifest stops parsing, so
    /// a broken manifest can sit in `projects` for a long time. Renaming a
    /// file in *another* project must still fix that project's manifest up;
    /// the broken one is only in the way when the move touches it, and then
    /// it is skipped and reported rather than failing the whole rename.
    #[test]
    fn will_rename_files_skips_projects_the_move_does_not_touch() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let root = std::env::temp_dir().join(format!("will-rename-broken-{}", std::process::id()));
        let good = root.join("a").join("sail.proj");
        let broken = root.join("b").join("sail.proj");
        let good_uri = Url::from_file_path(&good).unwrap();
        let broken_uri = Url::from_file_path(&broken).unwrap();
        let texts = std::sync::Arc::make_mut(&mut state.manifest_texts);
        texts.insert(
            good_uri.clone(),
            crate::mem_docs::DocumentData::new(1, "Main { files x.sail }\n".into()),
        );
        texts.insert(broken_uri, crate::mem_docs::DocumentData::new(1, "Main { files".into()));
        let projects = std::sync::Arc::make_mut(&mut state.projects);
        let x = root.join("a").join("x.sail");
        projects.insert(
            Url::from_directory_path(root.join("a")).unwrap(),
            crate::reload::FolderProject {
                manifest: good,
                files: vec![(Url::from_file_path(&x).unwrap(), x.clone())],
                ..Default::default()
            },
        );
        projects.insert(
            Url::from_directory_path(root.join("b")).unwrap(),
            crate::reload::FolderProject { manifest: broken, ..Default::default() },
        );
        let rename = |state: &mut GlobalState, from: &std::path::Path, to: &std::path::Path| {
            crate::handlers::request::handle_will_rename_files(
                state.snapshot(),
                lsp_types::RenameFilesParams {
                    files: vec![lsp_types::FileRename {
                        old_uri: Url::from_file_path(from).unwrap().into(),
                        new_uri: Url::from_file_path(to).unwrap().into(),
                    }],
                },
            )
        };

        let edit = rename(&mut state, &x, &root.join("a").join("y.sail"))
            .expect("a broken manifest elsewhere does not fail the rename")
            .expect("the good manifest is updated");
        let json = serde_json::to_value(edit).unwrap();
        assert_eq!(json["documentChanges"][0]["textDocument"]["uri"], good_uri.as_str());
        assert!(json["documentChanges"][0]["edits"][0]["newText"].as_str().unwrap().contains("y"));

        // A move inside the broken project's own directory cannot be fixed
        // up there; the rename itself still goes ahead.
        let z = root.join("b").join("z.sail");
        assert!(rename(&mut state, &z, &root.join("b").join("w.sail")).unwrap().is_none());
    }

    #[test]
    fn cache_collection_waits_for_retained_snapshots_and_applies_live_configuration() {
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let (tasks, _task_receiver) = crossbeam_channel::unbounded();
        let mut state = crate::global_state::GlobalState::new(sender, tasks);
        state.config.cache.inference_capacity = 7;
        let retained = state.analysis_host.db_snapshot();
        assert!(
            !state.collect_caches_if_idle(),
            "a retained snapshot must not be cancelled or block collection"
        );
        assert_eq!(state.cache_control.report()["effectiveCapacities"]["inference"], 0);
        drop(retained);
        assert!(state.collect_caches_if_idle());
        assert_eq!(state.cache_control.report()["effectiveCapacities"]["inference"], 7);
        state.config.cache.inference_capacity = 0;
        assert!(state.collect_caches_if_idle());
        assert_eq!(state.cache_control.report()["effectiveCapacities"]["inference"], 0);
        assert!(!state.collect_caches_if_idle(), "no repeated write on every idle request");
    }

    #[test]
    fn cancelled_request_is_recorded_once_without_source_or_internal_failure() {
        let (sender, responses) = crossbeam_channel::unbounded();
        let (tasks, _results) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let request = Request::new(
            1.into(),
            "textDocument/references".into(),
            serde_json::json!({"privateSource":"not collected"}),
        );
        state.register_request(&request);
        state.cancel(request.id.clone());
        state.cancel(request.id);
        assert_eq!(state.analysis_health.recent_requests.len(), 1);
        assert_eq!(state.analysis_health.recent_requests[0].method, "textDocument/references");
        assert_eq!(
            state.analysis_health.recent_requests[0].error_code,
            Some(lsp_server::ErrorCode::RequestCanceled as i32)
        );
        assert!(state.analysis_health.failures.is_empty());
        assert!(!serde_json::to_string(state.analysis_health.as_ref())
            .unwrap()
            .contains("not collected"));
        assert_eq!(responses.try_iter().filter(|m| matches!(m, Message::Response(_))).count(), 1);
    }

    #[test]
    fn diagnostic_worker_failure_is_visible_cancelled_work_is_not_failure_and_retry_recovers() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _results) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let file = base_db::FileId::from_raw(0);
        let generation = state.diagnostics.next_generation();
        let kind = NativeDiagnosticsFetchKind::Semantic;
        let failed = DiagnosticsTaskKind::compute(kind, generation, vec![file], || {
            panic!("inference fault")
        });
        on_task(&mut state, Task::Diagnostics(failed));
        assert!(state
            .current_status()
            .message
            .unwrap()
            .contains("Analysis incomplete: semanticDiagnostics failed: inference fault"));
        assert_eq!(state.diagnostics.diagnostics_for(file).count(), 0);
        let cancelled = DiagnosticsTaskKind::compute(kind, generation, vec![file], || {
            Err(ide_db::ide_types::CancelledError::PendingWrite)
        });
        on_task(&mut state, Task::Diagnostics(cancelled));
        assert_eq!(state.analysis_health.cancelled_waves, 1);
        assert!(state.analysis_health.failures.contains_key("semanticDiagnostics"));
        let next = state.diagnostics.next_generation();
        on_task(
            &mut state,
            Task::Diagnostics(DiagnosticsTaskKind::Semantic(next, vec![(file, Vec::new())])),
        );
        assert!(state.analysis_health.failures.is_empty());
        let stale =
            DiagnosticsTaskKind::compute(kind, generation, vec![file], || panic!("old fault"));
        on_task(&mut state, Task::Diagnostics(stale));
        assert!(state.analysis_health.failures.is_empty(), "stale failures cannot undo recovery");
        let dependent = DiagnosticsTaskKind::compute(kind, next, vec![file], || {
            Err(ide_db::ide_types::CancelledError::PropagatedPanic)
        });
        on_task(&mut state, Task::Diagnostics(dependent));
        assert!(state.analysis_health.failures["semanticDiagnostics"]
            .message
            .contains("dependent query panicked"));
        assert_eq!(state.analysis_health.cancelled_waves, 1);
    }

    #[test]
    fn shutdown_batch_answers_requests_until_exit() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let request =
            |id: i32, method: &str| Message::Request(Request::new(id.into(), method.into(), ()));
        assert!(on_lsp_batch(&mut state, vec![request(1, "shutdown")]).0);
        assert!(state.shutdown_requested);
        assert!(
            !on_lsp_batch(
                &mut state,
                vec![
                    request(2, "textDocument/hover"),
                    Message::Notification(Notification::new("exit".into(), ()))
                ]
            )
            .0
        );
        let responses: Vec<_> = messages
            .try_iter()
            .filter_map(|m| match m {
                Message::Response(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2);
        assert!(responses[0].error.is_none());
        assert_eq!(
            responses[1].error.as_ref().unwrap().code,
            lsp_server::ErrorCode::InvalidRequest as i32
        );
    }

    #[test]
    fn queued_edits_skip_obsolete_reads_and_apply_once_before_the_final_request() {
        use serde_json::json;
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let uri = "file:///sail-batch/main.sail";
        let notification =
            |method: &str, params| Message::Notification(Notification::new(method.into(), params));
        let request = |id: i32, method: &str| {
            Message::Request(Request::new(
                id.into(),
                method.into(),
                json!({"textDocument":{"uri":uri},"position":{"line":0,"character":9}}),
            ))
        };
        let change = |version: i32, text: &str| {
            notification(
                "textDocument/didChange",
                json!({
            "textDocument":{"uri":uri,"version":version},"contentChanges":[{"text":text}]}),
            )
        };
        let batch = vec![
            notification(
                "textDocument/didOpen",
                json!({"textDocument":{
                "uri":uri,"languageId":"sail","version":1,"text":"function old() = 0\n"}}),
            ),
            request(1, "textDocument/hover"),
            change(2, "function middle() = 0\n"),
            request(2, "textDocument/hover"),
            notification("$/cancelRequest", json!({"id":2})),
            change(3, "function newest() = 0\n"),
            change(2, "function stale() = 0\n"),
            request(3, "textDocument/documentSymbol"),
        ];
        let before = state.analysis_host.raw_database().nonce();
        assert_eq!(on_lsp_batch(&mut state, batch), (true, true));
        assert_eq!(state.analysis_host.raw_database().nonce(), before + 1);
        assert_eq!(state.document_version(&Url::parse(uri).unwrap()), Some(3));
        let responses: Vec<_> = messages
            .try_iter()
            .filter_map(|m| match m {
                Message::Response(r) => Some(r),
                _ => None,
            })
            .collect();
        assert_eq!(responses.len(), 2);
        assert_eq!(
            responses[0].error.as_ref().unwrap().code,
            lsp_server::ErrorCode::ServerCancelled as i32
        );
        assert_eq!(
            responses[1].error.as_ref().unwrap().code,
            lsp_server::ErrorCode::RequestCanceled as i32
        );
        let Task::Response(response) =
            tasks.recv_timeout(std::time::Duration::from_secs(5)).unwrap()
        else {
            panic!("expected final symbol response");
        };
        assert!(response.error.is_none(), "{response:?}");
        let result = response.result.unwrap().to_string();
        assert!(result.contains("newest") && !result.contains("middle"), "{result}");
    }

    #[test]
    fn retries_are_bounded_and_never_reuse_positions_after_document_changes() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let uri = Url::parse("file:///sail-retry/main.sail").unwrap();
        state.mem_docs.insert(uri.clone(), crate::mem_docs::DocumentData::new(1, String::new()));
        let request = Request::new(
            1.into(),
            "textDocument/hover".into(),
            serde_json::json!({"textDocument":{"uri":uri},"position":{"line":0,"character":0}}),
        );
        state.register_request(&request);
        assert!(state.can_retry(&request.id));
        assert!(state.can_retry(&request.id));
        assert!(!state.can_retry(&request.id));
        state.register_request(&request);
        state.mem_docs.get_mut(&uri).unwrap().version = 2;
        assert!(!state.can_retry(&request.id));
        state.mem_docs.get_mut(&uri).unwrap().version = 1;
        state.register_request(&request);
        state.mem_docs.remove(&uri);
        state.mem_docs.insert(uri, crate::mem_docs::DocumentData::new(1, "different text".into()));
        assert!(!state.can_retry(&request.id), "reopening can reset the version to one");
        on_task(&mut state, Task::Response(lsp_server::Response::new_ok(request.id, "old symbol")));
        let Message::Response(response) = messages.try_recv().unwrap() else { panic!("response") };
        assert_eq!(response.error.unwrap().code, lsp_server::ErrorCode::ServerCancelled as i32);
    }

    #[test]
    fn recheck_includes_open_standalone_files_and_deduplicates_projects() {
        use crate::flycheck::CheckTarget;
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.config.flycheck.enable = true;
        // Keep workers pending so this test exercises scheduling without an installed compiler.
        state.config.flycheck.debounce_ms = 60_000;
        let root =
            std::env::temp_dir().join(format!("flycheck-mixed-workspace-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let manifest = root.join("p.sail_project");
        let linked = root.join("linked.sail");
        let standalone = root.join("standalone.sail");
        let linked_uri = Url::from_file_path(&linked).unwrap();
        std::sync::Arc::make_mut(&mut state.projects).insert(
            Url::from_directory_path(&root).unwrap(),
            crate::reload::FolderProject {
                manifest: manifest.clone(),
                files: vec![(linked_uri.clone(), linked)],
                ..Default::default()
            },
        );
        for uri in [linked_uri, Url::from_file_path(&standalone).unwrap()] {
            std::fs::write(uri.to_file_path().unwrap(), "").unwrap();
            state.mem_docs.insert(uri, crate::mem_docs::DocumentData::new(1, String::new()));
        }
        state.recheck_flycheck();
        assert_eq!(state.flycheck_generation, 2, "each target must be submitted exactly once");
        assert_eq!(state.flycheck_latest.len(), 2);
        assert!(state.flycheck_latest.contains_key(&CheckTarget::Project(manifest)));
        assert!(state.flycheck_latest.contains_key(&CheckTarget::File(standalone)));
        state.invalidate_flycheck();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A document edited and then undone is not dirty: its buffer is the
    /// disk text, which is what a disk compiler checks. The dirty set only
    /// ever grew — `didChange` marked the document and nothing compared it
    /// to disk again — so saving another file of the project reported
    /// `Stale` and ran nothing until the undone file was saved or closed.
    #[test]
    fn an_edit_undone_does_not_hold_the_project_check_back() {
        use crate::flycheck::CheckTarget;
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.config.flycheck.enable = true;
        // Keep the worker pending: scheduling is what is under test.
        state.config.flycheck.debounce_ms = 60_000;
        let root = std::env::temp_dir().join(format!("flycheck-undo-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let manifest = root.join("sail.proj");
        let a = root.join("a.sail");
        let b = root.join("b.sail");
        let text = "function f() -> int = 1\n";
        for path in [&a, &b] {
            std::fs::write(path, text).unwrap();
        }
        let a_uri = Url::from_file_path(&a).unwrap();
        std::sync::Arc::make_mut(&mut state.projects).insert(
            Url::from_directory_path(&root).unwrap(),
            crate::reload::FolderProject {
                manifest: manifest.clone(),
                files: vec![
                    (a_uri.clone(), a.clone()),
                    (Url::from_file_path(&b).unwrap(), b.clone()),
                ],
                ..Default::default()
            },
        );
        crate::handlers::notification::handle_did_open_text_document(
            &mut state,
            lsp_types::DidOpenTextDocumentParams {
                text_document: lsp_types::TextDocumentItem {
                    uri: a_uri.clone(),
                    language_id: "sail".into(),
                    version: 1,
                    text: text.into(),
                },
            },
        )
        .unwrap();
        // Type a character, then undo it: the buffer is the disk text again.
        for (version, buffer) in [(2, format!("x{text}")), (3, text.to_owned())] {
            crate::handlers::notification::handle_did_change_text_document(
                &mut state,
                lsp_types::DidChangeTextDocumentParams {
                    text_document: lsp_types::VersionedTextDocumentIdentifier {
                        uri: a_uri.clone(),
                        version,
                    },
                    content_changes: vec![lsp_types::TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: buffer,
                    }],
                },
            )
            .unwrap();
        }
        state.request_flycheck(&b);
        let target = CheckTarget::Project(manifest);
        assert!(
            matches!(state.flycheck_status.get(&target), Some(crate::flycheck::Status::Queued)),
            "a buffer equal to disk holds nothing back: {:?}",
            state.flycheck_status.get(&target)
        );
        state.invalidate_flycheck();
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flycheck_controls_are_scoped_to_the_document_project() {
        use crate::flycheck::CheckTarget;
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let root =
            std::env::temp_dir().join(format!("flycheck-control-workspace-{}", std::process::id()));
        let a = root.join("a.sail");
        let b = root.join("b.sail");
        let a_uri = Url::from_file_path(&a).unwrap();
        let b_uri = Url::from_file_path(&b).unwrap();
        let a_target = CheckTarget::Project(root.join("a.sail_project"));
        let b_target = CheckTarget::Project(root.join("b.sail_project"));
        let projects = std::sync::Arc::make_mut(&mut state.projects);
        projects.insert(
            Url::parse("file:///workspace-a").unwrap(),
            crate::reload::FolderProject {
                manifest: root.join("a.sail_project"),
                files: vec![(a_uri.clone(), a.clone())],
                ..Default::default()
            },
        );
        projects.insert(
            Url::parse("file:///workspace-b").unwrap(),
            crate::reload::FolderProject {
                manifest: root.join("b.sail_project"),
                files: vec![(b_uri, b)],
                ..Default::default()
            },
        );
        for target in [&a_target, &b_target] {
            state.flycheck_latest.insert(target.clone(), 1);
            state.flycheck_status.insert(target.clone(), crate::flycheck::Status::Running);
        }
        let file_id = state.vfs.file_id_for_url(&a_uri);
        state.diagnostics.set_check_diagnostics(
            &a_target,
            vec![(
                file_id,
                vec![lsp_types::Diagnostic {
                    message: "from compiler".into(),
                    ..Default::default()
                }],
            )],
        );
        let document = lsp_types::TextDocumentIdentifier { uri: a_uri };

        state.cancel_flycheck(Some(&document));
        assert_eq!(state.flycheck_status.get(&a_target), Some(&crate::flycheck::Status::Cancelled));
        assert_eq!(state.flycheck_status.get(&b_target), Some(&crate::flycheck::Status::Running));
        assert_eq!(state.diagnostics.diagnostics_for(file_id).count(), 1);

        state.clear_flycheck(Some(&document));
        assert!(!state.flycheck_status.contains_key(&a_target));
        assert!(state.flycheck_status.contains_key(&b_target));
        assert_eq!(state.diagnostics.diagnostics_for(file_id).count(), 0);
    }

    #[test]
    fn late_worker_results_and_retries_do_not_revive_completed_requests() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        for cancelled in [false, true] {
            let request = Request::new(i32::from(cancelled).into(), "test/completed".into(), ());
            state.register_request(&request);
            if cancelled {
                state.cancel(request.id.clone());
            } else {
                on_task(
                    &mut state,
                    Task::Response(lsp_server::Response::new_ok(request.id.clone(), ())),
                );
            }
            assert!(state.is_completed(&request.id));
            let Message::Response(response) = messages.try_recv().unwrap() else {
                panic!("expected response")
            };
            assert_eq!(response.id, request.id);
            assert_eq!(
                response.error.map(|error| error.code),
                cancelled.then_some(lsp_server::ErrorCode::RequestCanceled as i32)
            );
            on_task(
                &mut state,
                Task::Response(lsp_server::Response::new_ok(request.id.clone(), ())),
            );
            on_task(&mut state, Task::Retry(request));
            assert!(messages.try_recv().is_err(), "duplicate response after completion");
            assert!(tasks.try_recv().is_err(), "completed request scheduled new work");
        }
    }

    /// A save starts a run per target; an earlier run that finishes later
    /// describes files as they no longer are, and is not believed.
    #[test]
    fn only_the_latest_check_of_a_target_is_believed() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        let path = std::env::temp_dir().join("flycheck-stale/a.sail");
        let url = lsp_types::Url::from_file_path(&path).unwrap();
        let file_id = state.vfs.file_id_for_url(&url);
        let target = crate::flycheck::CheckTarget::Project(path.with_file_name("p.sail_project"));
        let finding = |message: &str| {
            vec![(
                path.clone(),
                lsp_types::Diagnostic { message: message.into(), ..Default::default() },
            )]
        };
        state.flycheck_latest.insert(target.clone(), 2);
        let result = |generation, message| Task::Flycheck {
            run: crate::flycheck::RunReport::default(),
            status: crate::flycheck::Status::Diagnostics { count: 1 },
            target: target.clone(),
            generation,
            found: finding(message),
        };
        on_task(&mut state, result(2, "current"));
        on_task(&mut state, result(1, "stale"));
        let shown: Vec<_> =
            state.diagnostics.diagnostics_for(file_id).map(|d| d.message.clone()).collect();
        assert_eq!(shown, ["current"]);
    }

    #[test]
    fn an_unrepresentable_check_is_reported_and_clears_old_results() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.config.flycheck.enable = true;
        state.config.workspace.include_paths.push("lib".into());
        let path = std::env::temp_dir().join("flycheck-skipped.sail");
        let file_id = state.vfs.file_id_for_url(&Url::from_file_path(&path).unwrap());
        let target = crate::flycheck::CheckTarget::File(path.clone());
        state.flycheck_latest.insert(target.clone(), 1);
        state.diagnostics.set_check_diagnostics(
            &target,
            vec![(
                file_id,
                vec![lsp_types::Diagnostic { message: "obsolete".into(), ..Default::default() }],
            )],
        );
        on_task(
            &mut state,
            Task::Flycheck {
                run: crate::flycheck::RunReport::default(),
                target: target.clone(),
                generation: 1,
                found: vec![],
                status: crate::flycheck::Status::Skipped {
                    message: "includePaths requires check-context v1".into(),
                },
            },
        );
        assert!(matches!(
            state.flycheck_status.get(&target),
            Some(crate::flycheck::Status::Skipped { .. })
        ));
        assert_eq!(state.diagnostics.diagnostics_for(file_id).count(), 0);
        let messages: Vec<_> =
            messages.try_iter().map(|m| serde_json::to_value(m).unwrap()).collect();
        assert!(
            messages.iter().any(|m| m["method"] == "window/logMessage"
                && m["params"]["message"]
                    .as_str()
                    .is_some_and(|s| s.contains("Skipped") && s.contains("includePaths"))),
            "{messages:?}"
        );
    }

    #[test]
    fn flycheck_configuration_and_reload_invalidate_published_and_pending_results() {
        for change in ["disable", "command", "arguments", "variables", "roots", "reload"] {
            let (sender, _messages) = crossbeam_channel::unbounded();
            let (task_sender, _tasks) = crossbeam_channel::unbounded();
            let mut state = GlobalState::new(sender, task_sender);
            state.config.flycheck.enable = true;
            let path = std::env::temp_dir().join("flycheck-invalidation/a.sail");
            let url = Url::from_file_path(&path).unwrap();
            let file_id = state.vfs.file_id_for_url(&url);
            let target = crate::flycheck::CheckTarget::Project(path.with_file_name("sail.proj"));
            let result = || Task::Flycheck {
                run: crate::flycheck::RunReport::default(),
                status: crate::flycheck::Status::Diagnostics { count: 1 },
                target: target.clone(),
                generation: 1,
                found: vec![(
                    path.clone(),
                    lsp_types::Diagnostic { message: "obsolete".into(), ..Default::default() },
                )],
            };
            state.flycheck_latest.insert(target.clone(), 1);
            on_task(&mut state, result());
            assert_eq!(state.diagnostics.diagnostics_for(file_id).count(), 1);
            state.diagnostics.take_changes();
            let mut config = state.config.clone();
            match change {
                "disable" => config.flycheck.enable = false,
                "command" => config.flycheck.command = "other-sail".into(),
                "arguments" => config.flycheck.extra_args.push("--strict-bitvector".into()),
                "variables" => {
                    config
                        .workspace
                        .project_variables
                        .insert("RMEM".into(), crate::config::ProjectVariableValue::Bool(true));
                }
                "roots" => config.workspace.project_modules.push("Main".into()),
                "reload" => state.fetch_workspaces(
                    "manifest changed at same path".into(),
                    crate::reload::ReloadScope::All,
                ),
                _ => unreachable!(),
            }
            state.update_configuration(config);
            assert!(
                state.diagnostics.take_changes().unwrap().contains(&file_id),
                "{change}: publish clearing"
            );
            on_task(&mut state, result());
            assert_eq!(
                state.diagnostics.diagnostics_for(file_id).count(),
                0,
                "{change}: reject late result"
            );
            assert!(!state.flycheck_latest.contains_key(&target));
        }
    }

    #[test]
    fn failed_cache_priming_ends_progress_and_allows_a_later_reload() {
        let (sender, messages) = crossbeam_channel::unbounded();
        let (task_sender, _tasks) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, task_sender);
        state.supports_progress = true;
        state.prime_caches_queue.request_op("test".into(), ());
        assert!(state.prime_caches_queue.should_start_op().is_some());
        on_task(&mut state, Task::PrimeCaches(PrimeCachesProgress::Begin));
        assert!(state.prime_caches_progress.is_some());
        on_task(
            &mut state,
            Task::PrimeCaches(PrimeCachesProgress::End { cancelled: false, failed: true }),
        );
        assert!(state.prime_caches_progress.is_none());
        assert!(state.cancellable_progress.is_none());
        assert!(!state.prime_caches_queue.op_in_progress());
        assert!(!state.prime_caches_queue.op_requested(), "a panic must not cause a restart loop");
        let messages: Vec<_> =
            messages.try_iter().map(|message| serde_json::to_value(message).unwrap()).collect();
        assert!(
            messages.iter().any(|message| message["params"]["value"]["kind"] == "end"
                && message["params"]["value"]["message"] == "failed"),
            "{messages:?}"
        );
        state.prime_caches_queue.request_op("reload".into(), ());
        assert!(state.prime_caches_queue.should_start_op().is_some());
    }

    #[track_caller]
    fn pump_until(
        state: &mut GlobalState,
        tasks: &crossbeam_channel::Receiver<Task>,
        done: impl Fn(&GlobalState) -> bool,
    ) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            if let Some((cause, ())) = state.fetch_workspaces_queue.should_start_op() {
                let scope = state.take_reload_scope();
                state.fetch_workspaces(cause, scope);
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
        let before_rescan = state.workspace_load_version;
        state.handle_vfs_msg(vfs::loader::Message::WatchStatus {
            config_version: before_rescan,
            error: Some("native stream lost events".into()),
        });
        state.handle_vfs_msg(vfs::loader::Message::Rescan {
            config_version: before_rescan,
            roots: vec![paths::AbsPathBuf::assert_utf8(external.clone())],
        });
        pump_until(&mut state, &tasks, |s| {
            s.workspace_load_version > before_rescan && !s.fetch_workspaces_queue.op_in_progress()
        });
        assert!(source_is(&state, overlay), "recovery rescans must preserve editor text");
        assert!(
            state.watcher_error.is_none(),
            "successful subscription refresh clears the warning"
        );
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
        let url = Url::from_file_path(std::env::temp_dir().join("previous.sail")).unwrap();
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
        let b = a.join("consumer.sail").unwrap();
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
    fn reload_requested_before_large_project_preprocessing_releases_the_obsolete_load() {
        let (mut state, url, id) = state_with_active_file();
        state.sync_disk_text(
            &url,
            &format!("{}val previous : int\n", "// generated padding\n".repeat(16_000)),
        );
        state.process_changes();
        state.fetch_workspaces_queue.request_op("initial discovery".into(), ());
        state.fetch_workspaces_queue.should_start_op().unwrap();
        let revision = state.fetch_workspaces_queue.revision();
        state.loading_project_config =
            Some((state.config.workspace.clone(), state.workspace_folders.clone(), revision));
        state.loading_source_project = Some(base_db::preprocess::SourceProject {
            roots: vec![id],
            paths: vec![(url.to_file_path().unwrap(), id)],
            ..Default::default()
        });
        // A watcher event arrives after discovery, before the VFS barrier.
        state.request_workspace_reload(
            "source discovered during loading",
            crate::reload::ReloadScope::All,
        );
        state.finish_workspace_load(&[(url.clone(), url.to_file_path().unwrap())], 1);
        assert!(!state.fetch_workspaces_queue.op_in_progress(), "a plan rejected before spawning a worker has no completion event to release this operation");
        assert!(state.pending_preprocessing.is_none());
        assert!(state.loading_source_project.is_none());
        assert!(!state.preprocessing_running);
        assert!(
            state.fetch_workspaces_queue.should_start_op().is_some(),
            "the queued replacement must be allowed to load"
        );
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
                ..Default::default()
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
