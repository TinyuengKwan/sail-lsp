//! Workspace loading and reloading.
//! Key methods on `GlobalState`:
//! - `is_quiescent()` — whether the server is fully loaded
//! - `current_status()` — compute health/message for ServerStatus
//! - `update_configuration()` — apply config diff, trigger reload if needed
//! - `fetch_workspaces()` — spawn background workspace scan

use std::collections::HashSet;
use std::path::PathBuf;

use lsp_types::Url;

use crate::config::SailLspConfig;
use crate::global_state::GlobalState;

/// Whether a loader watches the filesystem, or only answers questions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Watching {
    Yes,
    No,
}

/// Scan a directory for Sail files.
/// Returns all `.sail` files found recursively.
pub fn scan_sail_files(root: &std::path::Path) -> Vec<(PathBuf, String)> {
    let mut result = Vec::new();

    let walker = walkdir::WalkDir::new(root).follow_links(true).into_iter().filter_map(|e| e.ok());

    for entry in walker {
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "sail") {
            if let Ok(text) = std::fs::read_to_string(path) {
                result.push((path.to_path_buf(), text));
            }
        }
    }

    result
}

/// Check if a file path is a Sail source file.
pub fn is_sail_file(path: &std::path::Path) -> bool {
    path.extension().is_some_and(|ext| ext == "sail")
}

pub(crate) fn include_search_paths(
    folders: &HashSet<Url>,
    config: &crate::config::WorkspaceConfig,
) -> Vec<PathBuf> {
    let mut roots: Vec<_> = folders.iter().filter_map(|u| u.to_file_path().ok()).collect();
    roots.sort();
    config
        .include_paths
        .iter()
        .flat_map(|p| {
            let path = PathBuf::from(p);
            if path.is_absolute() {
                vec![path]
            } else {
                roots.iter().map(|r| r.join(&path)).collect()
            }
        })
        .collect()
}

/// Check if a file path is a Sail project file.
pub fn is_project_file(path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name == "sail.proj" || name.ends_with(".sail_project")
    })
}

/// Discover only at the workspace root and, if it has no manifest, one
/// directory level below it. Like RA's child Cargo.toml discovery, this is
/// bounded in depth rather than walking the repository. Explicit configuration wins.
fn find_project_file(
    folder: &std::path::Path,
    configured: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    if let Some(file) = configured {
        let path = folder.join(file);
        return if path.is_file() {
            Ok(Some(path))
        } else {
            Err(format!("Project file does not exist: {}", path.display()))
        };
    }
    fn in_directory(folder: &std::path::Path) -> Result<Vec<PathBuf>, String> {
        let preferred = folder.join("sail.proj");
        if preferred.is_file() {
            return Ok(vec![preferred]);
        }
        std::fs::read_dir(folder)
            .map_err(|e| format!("{}: {e}", folder.display()))?
            .map(|entry| entry.map(|e| e.path()).map_err(|e| e.to_string()))
            .filter_map(|entry| match entry {
                Ok(path) if path.is_file() && is_project_file(&path) => Some(Ok(path)),
                Ok(_) => None,
                Err(error) => Some(Err(error)),
            })
            .collect()
    }
    let mut candidates = in_directory(folder)?;
    if candidates.is_empty() {
        for entry in std::fs::read_dir(folder).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.') {
                candidates.extend(in_directory(&entry.path())?);
            }
        }
    }
    candidates.sort();
    candidates.dedup();
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(candidates.pop()),
        _ => Err(format!(
            "Multiple Sail project files in {}; select workspace.projectFile: {}",
            folder.display(),
            candidates.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
        )),
    }
}

/// Background discovery carries paths and the configuration they were resolved
/// under. Text remains the VFS loader's responsibility.
#[derive(Clone, Debug, Default)]
pub(crate) struct DiscoveryInputs {
    pub watch_files: Vec<PathBuf>,
    /// Recoverable source errors. The manifest still defines a usable graph.
    pub source_errors: Vec<String>,
    /// Manifest contents used to derive membership and module edges. Checking
    /// again at publication also covers delayed OS/client notifications.
    pub manifests: std::collections::BTreeMap<PathBuf, Option<String>>,
    /// The project each folder loaded (or kept), by folder.
    pub projects: std::collections::BTreeMap<Url, FolderProject>,
    /// Folders whose manifest failed while others loaded, one message each.
    pub project_errors: Vec<String>,
    /// What is wrong in each manifest, placed in its text. A manifest with
    /// nothing wrong is absent.
    pub manifest_diagnostics: std::collections::BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    /// Evidence for scoped reloads and status reporting.
    pub scanned_projects: usize,
    pub reused_projects: usize,
}

/// A manifest problem where the manifest says it: the token that does not
/// parse, the module name that is unknown or cyclic, the `files` entry that
/// names a missing file. A problem in the configuration (an undeclared root
/// module) has no place in the text and goes at its start.
fn manifest_diagnostic(
    text: &str,
    range: Option<std::ops::Range<usize>>,
    code: &str,
    message: String,
) -> lsp_types::Diagnostic {
    let index = ide_db::line_index::LineIndex::new(text);
    let range = range.unwrap_or(0..0);
    let at = |offset: usize| crate::lsp::to_proto::position_in(&index, offset.min(text.len()));
    lsp_types::Diagnostic {
        range: lsp_types::Range::new(at(range.start), at(range.end)),
        severity: Some(lsp_types::DiagnosticSeverity::ERROR),
        code: Some(lsp_types::NumberOrString::String(code.to_owned())),
        source: Some("sail-lsp".to_owned()),
        message,
        ..Default::default()
    }
}

fn error_code(kind: project_model::ProjectErrorKind) -> &'static str {
    use project_model::ProjectErrorKind as K;
    match kind {
        K::Syntax => "manifest-syntax",
        K::UnknownModule => "unknown-module",
        K::UnknownRoot => "unknown-root-module",
        K::Cycle => "cyclic-modules",
        K::Duplicate => "duplicate-declaration",
        K::UnknownVariable => "unknown-variable",
        K::Type => "manifest-type-error",
        K::VirtualFile => "unregistered-virtual-file",
        K::Explicit => "manifest-error",
    }
}

/// Where a module is declared and which files it lists, as positions in
/// the manifest, selected or not.
#[derive(Clone, Debug, Default)]
pub(crate) struct ModuleDeclaration {
    pub name_range: lsp_types::Range,
    /// Each listed file and the `files` entry that lists it.
    pub files: Vec<(Url, lsp_types::Range)>,
}

/// One workspace folder's project, with module ids local to it.
#[derive(Clone, Debug, Default)]
pub(crate) struct FolderProject {
    pub manifest: PathBuf,
    pub workspace_root: PathBuf,
    pub config: crate::config::WorkspaceConfig,
    pub resolved: Option<project_model::ResolvedProject>,
    /// Every module the manifest declares, in declaration order.
    pub declarations: Vec<ModuleDeclaration>,
    /// Project source files that exist, each once.
    pub files: Vec<(Url, PathBuf)>,
    pub modules: Vec<(Vec<Url>, Vec<usize>)>,
    pub source_roots: Vec<(Url, Option<usize>)>,
    /// Physical files in this project's transitive `$include` closure, in
    /// preprocessing order. Scoped manifest reloads reuse this for projects
    /// whose manifest and configuration did not change.
    pub discovered_files: Vec<(Url, PathBuf)>,
    /// Discovery state owned by this project, retained when another linked
    /// project reloads. This makes a scoped reload observationally identical
    /// to a full scan without reparsing the unaffected manifest.
    pub watch_files: Vec<PathBuf>,
    pub source_errors: Vec<String>,
    pub manifest_text: Option<String>,
    pub manifest_diagnostics: Vec<lsp_types::Diagnostic>,
}

impl FolderProject {
    /// Whether `name` names this project: its manifest file with or without
    /// the extension (`riscv.sail_project`, `riscv`), the directory the
    /// manifest sits in (`riscv` for `riscv/sail.proj`), or the manifest's
    /// full path. A name, not a substring: `project:riscv` used to select
    /// every project anywhere under a `riscv/` tree.
    pub(crate) fn is_named(&self, name: &str) -> bool {
        let part = |p: Option<&std::ffi::OsStr>| p.is_some_and(|p| p == name);
        part(self.manifest.file_name())
            || part(self.manifest.file_stem())
            || part(self.manifest.parent().and_then(std::path::Path::file_name))
            || self.manifest.as_os_str() == name
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReloadScope {
    /// Folder/configuration/discovery changes can add or remove projects.
    #[default]
    All,
    /// Only these already-known manifests changed.
    Projects(std::collections::BTreeSet<PathBuf>),
}

impl ReloadScope {
    pub(crate) fn project(path: PathBuf) -> Self {
        Self::Projects([syntax::preprocess::normalize_path(&path)].into_iter().collect())
    }

    pub(crate) fn merge(&mut self, other: Self) {
        match (&mut *self, other) {
            (Self::All, _) | (_, Self::All) => *self = Self::All,
            (Self::Projects(current), Self::Projects(other)) => current.extend(other),
        }
    }

    fn includes(&self, manifest: &std::path::Path) -> bool {
        match self {
            Self::All => true,
            Self::Projects(paths) => paths.contains(&syntax::preprocess::normalize_path(manifest)),
        }
    }
}

fn reuse_project_inputs(project: &FolderProject, inputs: &mut DiscoveryInputs) {
    inputs.watch_files.extend(project.watch_files.iter().cloned());
    inputs.source_errors.extend(project.source_errors.iter().cloned());
    inputs.manifests.insert(project.manifest.clone(), project.manifest_text.clone());
    if !project.manifest_diagnostics.is_empty() {
        inputs
            .manifest_diagnostics
            .insert(project.manifest.clone(), project.manifest_diagnostics.clone());
    }
}

impl DiscoveryInputs {
    pub(crate) fn matches_disk(&self) -> bool {
        self.manifests.iter().all(|(path, text)| std::fs::read_to_string(path).ok() == *text)
    }
}

#[derive(Debug)]
pub(crate) struct WorkspaceScan {
    pub revision: u64,
    pub inputs: DiscoveryInputs,
    pub files: Result<Vec<(Url, PathBuf)>, String>,
    pub modules: Vec<(Vec<Url>, Vec<usize>)>,
    pub source_roots: Option<Vec<(Url, Option<usize>)>>,
    pub config: crate::config::WorkspaceConfig,
    pub folders: HashSet<Url>,
}

impl GlobalState {
    pub(crate) fn request_workspace_reload(
        &mut self,
        cause: impl Into<String>,
        scope: ReloadScope,
    ) {
        let cause = cause.into();
        tracing::debug!(%cause, ?scope, "workspace reload requested");
        self.preprocessing_cancel.0.cancel();
        if let Some(pending) = &mut self.pending_reload_scope {
            pending.merge(scope);
        } else {
            self.pending_reload_scope = Some(scope);
        }
        self.fetch_workspaces_queue.request_op(cause, ());
    }

    pub(crate) fn take_reload_scope(&mut self) -> ReloadScope {
        self.pending_reload_scope.take().unwrap_or_default()
    }

    /// Is the server quiescent?
    /// Returns true when workspace has been fully loaded and no
    /// background tasks are pending.
    /// Full quiescence check — all background operations complete.
    /// checks VFS, workspace fetch, build data, proc macros.
    /// Sail checks: workspace fetch, prime caches, workspace context,
    /// and dirty file index.
    /// index_dirty_files is NOT checked here — it's drained inside
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.fetch_workspaces_queue.op_in_progress() && self.pending_configuration.is_none()
    }

    /// Compute current server status for the client.
    /// Returns a status struct with health, quiescent flag, and
    /// optional message describing any issues.
    pub(crate) fn current_status(&self) -> crate::lsp_ext::ServerStatusParams {
        let loading = self.pending_configuration.is_some()
            || self.fetch_workspaces_queue.op_in_progress()
            || self.prime_caches_queue.op_in_progress();

        let mut status = crate::lsp_ext::ServerStatusParams {
            health: if loading {
                crate::lsp_ext::Health::Warning
            } else {
                crate::lsp_ext::Health::Ok
            },
            quiescent: self.is_quiescent(),
            message: if loading { Some("Loading workspace...".to_string()) } else { None },
        };
        let mut message = String::new();

        // Check workspace state
        if self.workspace_folders.is_empty() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("No workspace folders configured.\n");
        }

        // Check if files are loaded — but only once the scan has finished.
        //
        // `workspace_loaded` is set when the first `Task::WorkspaceScan`
        // completes and had no reader, so this warned "No Sail files
        // loaded" for the whole of startup, when the count is legitimately
        // zero because nothing has been scanned yet.
        let file_count = self.analysis_host.raw_database().files().len();
        if self.workspace_loaded && file_count == 0 && !self.workspace_folders.is_empty() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("No Sail files loaded. Check workspace folders.\n");
        }
        if self.workspace_loaded && self.projects.is_empty() && self.project_error.is_none() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("No Sail project loaded. Set sail-lsp.workspace.projectFile; unlinked files receive syntax diagnostics only.\n");
        }

        // Check for include graph cycles
        if self.include_graph.find_cycle().is_some() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("Circular $include dependency detected.\n");
        }

        let mut unsaved_manifests: Vec<_> = self
            .dirty_documents
            .iter()
            .filter(|uri| crate::manifest_editing::is_manifest(uri))
            .collect();
        unsaved_manifests.sort();
        for uri in unsaved_manifests {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str(&format!(
                "Unsaved project manifest: {uri}. Save to apply project changes; analysis uses the last loaded project.\n"
            ));
        }

        for (target, check) in &self.flycheck_status {
            status.quiescent &= !check.busy();
            if check.problem() && status.health != crate::lsp_ext::Health::Error {
                status.health = crate::lsp_ext::Health::Warning;
            }
            let path = match target {
                crate::flycheck::CheckTarget::Project(path)
                | crate::flycheck::CheckTarget::File(path) => path,
            };
            message.push_str(&format!("Flycheck {}: {check}\n", path.display()));
            if let Some(history) = self.flycheck_history.get(target) {
                message.push_str(&history.summary());
                message.push('\n');
                let toolchain_warning = history
                    .last_run
                    .as_ref()
                    .and_then(|run| run.toolchain.as_ref())
                    .is_some_and(|report| !report.warnings.is_empty());
                if (history.cache.warning.is_some() || toolchain_warning)
                    && status.health != crate::lsp_ext::Health::Error
                {
                    status.health = crate::lsp_ext::Health::Warning;
                }
            }
        }
        for error in &self.config_errors {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str(&format!("Configuration: {error}\n"));
        }
        for (phase, failure) in &self.analysis_health.failures {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str(&format!("Analysis incomplete: {phase} failed: {}. Retry by editing or reloading the workspace.\n", failure.message));
        }
        if !self.analysis_health.reported_unknowns.is_empty() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str(&format!(
                "Undecided constraints in the latest published diagnostics: {:?}\n",
                self.analysis_health.reported_unknowns
            ));
        }
        if let Some(error) = &self.watcher_error {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str(&format!("File watching degraded: {error}. Background rescans and recovery retries are active.\n"));
        }
        if let Some(error) = &self.project_error {
            status.health = crate::lsp_ext::Health::Error;
            message.push_str(error);
        }
        if !message.is_empty() {
            status.message = Some(message);
        }
        status
    }

    /// Publish what is wrong in each manifest on the manifest itself, and
    /// clear a manifest that is no longer wrong.
    pub(crate) fn publish_manifest_diagnostics(
        &mut self,
        current: std::collections::BTreeMap<PathBuf, Vec<lsp_types::Diagnostic>>,
    ) {
        let previous = std::mem::replace(&mut self.manifest_diagnostics, current);
        let paths: std::collections::BTreeSet<_> =
            previous.keys().chain(self.manifest_diagnostics.keys()).cloned().collect();
        for path in paths {
            let now = self.manifest_diagnostics.get(&path).cloned().unwrap_or_default();
            if previous.get(&path).map_or(now.is_empty(), |before| *before == now) {
                continue;
            }
            let Ok(uri) = Url::from_file_path(&path) else { continue };
            self.send_notification::<lsp_types::notification::PublishDiagnostics>(
                lsp_types::PublishDiagnosticsParams { uri, diagnostics: now, version: None },
            );
        }
    }

    /// Keep the cause in server status and surface it in clients without RA's
    /// server-status UI. Repeated watcher events must not repeat the same popup.
    pub(crate) fn report_project_error(&mut self, error: Option<String>) {
        if self.project_error == error {
            return;
        }
        if let Some(message) = &error {
            self.log_message(lsp_types::MessageType::ERROR, message.clone());
            self.send_notification::<lsp_types::notification::ShowMessage>(
                lsp_types::ShowMessageParams {
                    typ: lsp_types::MessageType::ERROR,
                    message: message.clone(),
                },
            );
        } else {
            self.log_message(lsp_types::MessageType::INFO, "Sail project loading recovered.");
        }
        self.project_error = error;
        self.diagnostics.mark_all_changed();
        self.diagnostics_refresh_requested = true;
    }

    /// Apply configuration changes and trigger reload if needed.
    /// Compares old and new config, scheduling a workspace reload if
    /// structural settings (search paths, workspace folders) changed.
    /// Apply settings as the client sent them, and tell the user about any
    /// it could not read — once per distinct set, so an unrelated settings
    /// change does not repeat the popup.
    pub(crate) fn apply_client_settings(&mut self, settings: &serde_json::Value) {
        self.configuration_inputs.global = crate::shared_configuration::section(settings).clone();
        self.refresh_shared_configuration();
    }

    pub(crate) fn report_config_errors(&mut self, errors: Vec<String>) {
        if errors == self.config_errors {
            return;
        }
        if !errors.is_empty() {
            let message =
                format!("sail-lsp: these settings could not be applied: {}", errors.join("; "));
            self.log_message(lsp_types::MessageType::WARNING, message.clone());
            self.send_notification::<lsp_types::notification::ShowMessage>(
                lsp_types::ShowMessageParams { typ: lsp_types::MessageType::WARNING, message },
            );
        }
        self.config_errors = errors;
    }

    pub(crate) fn update_configuration(&mut self, new_config: SailLspConfig) {
        let old_config = std::mem::replace(&mut self.config, new_config);
        self.semantic_tokens_refresh_requested |=
            self.config.semantic_tokens != old_config.semantic_tokens;
        if self.config.completion.autoimport && !old_config.completion.autoimport {
            self.prime_caches_queue.request_op("autoimport enabled".into(), ());
        }

        if self.config.workspace != old_config.workspace
            || !self.config.flycheck.same_invocation(&old_config.flycheck)
        {
            self.invalidate_flycheck();
        }
        if self.config.workspace.strict_bitvector != old_config.workspace.strict_bitvector {
            let mut change = ide_db::apply_change::Change::new();
            change.set_strict_bitvector(self.config.workspace.strict_bitvector);
            self.analysis_host.apply_change(change);
            self.diagnostics.mark_all_changed();
        }
        // If workspace-level config changed, trigger reload
        if self.config.workspace != old_config.workspace {
            log::info!("workspace config changed, triggering reload");
            self.request_workspace_reload("configuration changed", ReloadScope::All);
        }

        if !self.config.flycheck.same_invocation(&old_config.flycheck) {
            self.recheck_flycheck();
        }
        self.apply_compiler_diagnostic_policies();

        // Diagnostics are re-computed lazily on next request;
        // no explicit invalidation needed when diagnostics config changes.
    }

    /// Spawn a background workspace scan.
    /// Called from the main loop when `fetch_workspaces_queue.should_start_op()`
    /// returns `Some`. Scans workspace folders for `.sail` files and sends
    /// results back via `Task::WorkspaceScan`.
    pub(crate) fn fetch_workspaces(&mut self, cause: String, scope: ReloadScope) {
        log::info!("fetching workspaces: {cause}");
        // The projects about to be rescanned may come back different, so
        // their checks are forgotten now. A file no project lists is checked
        // on its own and owes nothing to the model: its run goes on, and its
        // answer is still believed when it lands. This used to forget every
        // target, and the save of an unlisted file — which triggers a scan
        // on the native watcher path — discarded the very check it started.
        self.invalidate_flycheck_targets(|target| match target {
            crate::flycheck::CheckTarget::Project(manifest) => scope.includes(manifest),
            crate::flycheck::CheckTarget::File(_) => false,
        });

        // Ask the client to tell us when a file changes on disk. Without
        // this it never sends `didChangeWatchedFiles`, and the handler for
        // it — which has been here all along — waits for a notification
        // nobody was asked to send: a `.sail` file replaced by `git
        // checkout` or written by another tool went unnoticed.
        // rust-analyzer registers the same way (`reload.rs`), and falls
        // back to watching itself when the client cannot.
        self.register_file_watchers();

        let folders = self.workspace_folders.clone();
        // Removing the last folder still needs to publish an empty project.
        // Completing the operation here would leave its old members active.

        let sender = self.sender.clone();
        let supports_progress = self.supports_progress;
        let progress_token = supports_progress.then(|| self.progress_token("Reloading workspace"));
        let effective_sail_dir = self.effective_sail_dir.clone();
        let previous = self.projects.clone();
        let config = self.config.workspace.clone();
        let scoped_configs = self.scoped_configs.clone();
        let revision = self.fetch_workspaces_queue.revision();
        let overlays: std::collections::HashMap<PathBuf, String> = self
            .mem_docs
            .iter()
            .filter_map(|url| {
                Some((
                    syntax::preprocess::normalize_path(&url.to_file_path().ok()?),
                    self.mem_docs.get(url)?.data.clone(),
                ))
            })
            .collect();

        self.task_pool.spawn(stdx::thread::ThreadIntent::Worker, move || {
            let progress = progress_token.map(|token| {
                crate::progress::ProgressReporter::begin(
                    &sender,
                    token,
                    "Reloading workspace",
                    false,
                    Some(0),
                )
            });
            if let Some(ref p) = progress {
                p.report("scanning files...", Some(10));
            }

            let mut modules = Vec::new();
            let mut source_roots = Vec::new();
            let mut inputs = DiscoveryInputs::default();
            let files = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scan_workspace_project_scoped(
                    &folders,
                    effective_sail_dir.as_deref(),
                    &config,
                    &scoped_configs,
                    &mut modules,
                    &mut source_roots,
                    &overlays,
                    &previous,
                    &scope,
                    &mut inputs,
                )
            }))
            .unwrap_or_else(|payload| {
                Err(format!(
                    "Workspace discovery failed internally: {}",
                    crate::analysis_health::panic_message(payload.as_ref())
                ))
            });
            if let Some(p) = progress {
                match &files {
                    Ok(files) => p.end(&format!("{} files discovered", files.len())),
                    Err(error) => p.end(error),
                }
            }
            crate::main_loop::Task::WorkspaceScan(WorkspaceScan {
                revision,
                inputs,
                files,
                modules,
                source_roots: Some(source_roots),
                config,
                folders,
            })
        });
    }
}

/// Scan workspace folders for `.sail` files.
/// Returns `(url, path)` pairs for all discovered files.
/// respecting project structure.
#[cfg(test)]
fn scan_workspace_folders(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
) -> Result<Vec<(Url, PathBuf)>, String> {
    scan_workspace_project(
        folders,
        sail_dir,
        config,
        &mut Vec::new(),
        &mut Vec::new(),
        &Default::default(),
        &Default::default(),
        &ReloadScope::All,
        &mut DiscoveryInputs::default(),
    )
}

/// The project one workspace folder defines, if it has a manifest. Inputs
/// (manifests, watched paths, missing sources) are recorded even when it
/// fails, so fixing the manifest reloads it.
fn scan_folder(
    folder_path: &std::path::Path,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
    overlays: &std::collections::HashMap<PathBuf, String>,
    inputs: &mut DiscoveryInputs,
) -> Result<Option<FolderProject>, String> {
    let watch_start = inputs.watch_files.len();
    let source_error_start = inputs.source_errors.len();
    if let Some(configured) = &config.project_file {
        let path = syntax::preprocess::normalize_path(&folder_path.join(configured));
        inputs.manifests.insert(path.clone(), std::fs::read_to_string(&path).ok());
        inputs.watch_files.push(path);
    }

    let Some(project_file_path) = find_project_file(folder_path, config.project_file.as_deref())?
    else {
        return Ok(None);
    };
    {
        inputs.watch_files.push(project_file_path.clone());
        let source = std::fs::read_to_string(&project_file_path);
        inputs.manifests.insert(project_file_path.clone(), source.as_ref().ok().cloned());
        let source = source.map_err(|e| format!("{}: {e}", project_file_path.display()))?;
        let mut options = config.project_options();
        for path in options.virtual_files.values_mut() {
            *path = syntax::preprocess::normalize_path(&folder_path.join(&*path));
        }
        // Upstream's corelib_sail.ml registers this exact embedded source
        // under corelib.sail. Keep a physical URI for navigation and edits;
        // other names require an explicit producer-provided source mapping.
        if let Some(sail_dir) = sail_dir {
            options
                .virtual_files
                .entry("corelib.sail".into())
                .or_insert_with(|| sail_dir.join("lib/core/corelib.sail"));
        }
        let resolved = project_model::ProjectManifest::parse(&source)
            .and_then(|manifest| manifest.resolve(&options))
            .map_err(|error| {
                let diagnostic = manifest_diagnostic(
                    &source,
                    error.range.clone(),
                    error_code(error.kind),
                    error.message.clone(),
                );
                let place = match error.range {
                    Some(_) => format!(
                        "{}:{}:{}",
                        project_file_path.display(),
                        diagnostic.range.start.line + 1,
                        diagnostic.range.start.character + 1
                    ),
                    None => project_file_path.display().to_string(),
                };
                inputs
                    .manifest_diagnostics
                    .entry(project_file_path.clone())
                    .or_default()
                    .push(diagnostic);
                format!("{place}: {error}")
            })?;
        let project_dir = project_file_path.parent().unwrap_or(folder_path);
        let mut project = FolderProject {
            manifest: project_file_path.clone(),
            workspace_root: folder_path.to_owned(),
            config: config.clone(),
            ..Default::default()
        };
        let index = ide_db::line_index::LineIndex::new(&source);
        let range = |r: &std::ops::Range<usize>| {
            let at = |o: usize| crate::lsp::to_proto::position_in(&index, o);
            lsp_types::Range::new(at(r.start), at(r.end))
        };
        project.declarations = resolved
            .modules
            .iter()
            .map(|m| ModuleDeclaration {
                name_range: range(&m.name_range),
                files: m
                    .files
                    .iter()
                    .zip(&m.file_ranges)
                    .filter_map(|(f, r)| {
                        let path = syntax::preprocess::normalize_path(&project_dir.join(f));
                        Some((Url::from_file_path(path).ok()?, range(r)))
                    })
                    .collect(),
            })
            .collect();
        for (id, module) in resolved.modules.iter().enumerate() {
            let files = if resolved.module_order.contains(&id) {
                module
                    .files
                    .iter()
                    .filter_map(|p| Url::from_file_path(project_dir.join(p)).ok())
                    .collect()
            } else {
                Vec::new()
            };
            project.modules.push((files, module.requires.clone()));
        }
        // Keep semantic root occurrences and their owners even when the
        // same path belongs to several modules. Discovery still dedups I/O.
        for (id, path) in resolved.file_occurrences() {
            if let Ok(url) = Url::from_file_path(project_dir.join(path)) {
                project.source_roots.push((url, Some(id)));
            }
        }
        for relative_path in &resolved.files {
            let path = project_dir.join(relative_path);
            let normalized = syntax::preprocess::normalize_path(&path);
            inputs.watch_files.push(normalized.clone());
            // A bad path is a local project problem, like RA's unresolved
            // module, not a reason to discard every other source file.
            // Keep watching it so creating the file repairs the project.
            if !overlays.contains_key(&normalized) {
                let problem = match std::fs::metadata(&path) {
                    Ok(metadata) if metadata.is_file() => None,
                    Ok(_) => Some("not a regular file".to_owned()),
                    Err(error) => Some(error.to_string()),
                };
                if let Some(problem) = problem {
                    inputs.source_errors.push(format!(
                        "{}: cannot load source {}: {problem}",
                        project_file_path.display(),
                        path.display(),
                    ));
                    let written = resolved.modules.iter().find_map(|m| {
                        let k = m.files.iter().position(|f| f == relative_path)?;
                        m.file_ranges.get(k).cloned()
                    });
                    inputs.manifest_diagnostics.entry(project_file_path.clone()).or_default().push(
                        manifest_diagnostic(
                            &source,
                            written,
                            "missing-file",
                            format!("{}: {problem}", relative_path.display()),
                        ),
                    );
                    continue;
                }
            }
            if is_sail_file(&path) {
                let uri = Url::from_file_path(&path)
                    .map_err(|_| format!("Invalid project path: {}", path.display()))?;
                project.files.push((uri, path));
            }
        }
        project.resolved = Some(resolved);
        project.watch_files = inputs.watch_files[watch_start..].to_vec();
        project.source_errors = inputs.source_errors[source_error_start..].to_vec();
        project.manifest_text = inputs.manifests.get(&project_file_path).cloned().flatten();
        project.manifest_diagnostics =
            inputs.manifest_diagnostics.get(&project_file_path).cloned().unwrap_or_default();
        // An explicitly empty configuration is still a project. Directory
        // fallback would reintroduce every inactive file.
        Ok(Some(project))
    }
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn scan_workspace_project(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
    modules: &mut Vec<(Vec<Url>, Vec<usize>)>,
    source_roots: &mut Vec<(Url, Option<usize>)>,
    overlays: &std::collections::HashMap<PathBuf, String>,
    previous: &std::collections::BTreeMap<Url, FolderProject>,
    scope: &ReloadScope,
    inputs: &mut DiscoveryInputs,
) -> Result<Vec<(Url, PathBuf)>, String> {
    scan_workspace_project_scoped(
        folders,
        sail_dir,
        config,
        &Default::default(),
        modules,
        source_roots,
        overlays,
        previous,
        scope,
        inputs,
    )
}

#[allow(clippy::too_many_arguments)]
fn scan_workspace_project_scoped(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
    scoped_configs: &std::collections::BTreeMap<Url, SailLspConfig>,
    modules: &mut Vec<(Vec<Url>, Vec<usize>)>,
    source_roots: &mut Vec<(Url, Option<usize>)>,
    overlays: &std::collections::HashMap<PathBuf, String>,
    previous: &std::collections::BTreeMap<Url, FolderProject>,
    scope: &ReloadScope,
    inputs: &mut DiscoveryInputs,
) -> Result<Vec<(Url, PathBuf)>, String> {
    let mut folders: Vec<_> = folders.iter().collect();
    folders.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let folder_paths: Vec<_> = folders.iter().filter_map(|url| url.to_file_path().ok()).collect();
    let mut failures = Vec::new();
    let mut reuse_discovery = std::collections::BTreeSet::new();
    // Scan each project independently. A failed entry retains only its own
    // last usable model; it never unloads the other entries in this folder.
    for folder_url in folders {
        let config = scoped_configs.get(folder_url).map_or(config, |config| &config.workspace);
        let Ok(folder_path) = folder_url.to_file_path() else { continue };
        let entries: Vec<_> = if config.linked_projects.is_empty() {
            vec![(folder_url.clone(), config.clone())]
        } else {
            config
                .linked_projects
                .iter()
                .filter_map(|entry| {
                    let path = syntax::preprocess::normalize_path(&folder_path.join(entry.path()));
                    if std::path::Path::new(entry.path()).is_absolute() {
                        // Resolve workspace-relative generated sources against the
                        // containing folder, not whichever folder sorts first.
                        let owner = folder_paths
                            .iter()
                            .filter(|root| path.starts_with(root))
                            .max_by_key(|root| root.components().count())
                            .or_else(|| folder_paths.first());
                        if owner.is_some_and(|owner| owner != &folder_path) {
                            return None;
                        }
                    }
                    Some((Url::from_file_path(path).ok()?, entry.effective(config)))
                })
                .collect()
        };
        for (key, effective) in entries {
            if inputs.projects.contains_key(&key) {
                continue;
            }
            if let Some(project) =
                previous.get(&key).filter(|project| !scope.includes(&project.manifest))
            {
                let project = project.clone();
                reuse_project_inputs(&project, inputs);
                reuse_discovery.insert(key.clone());
                inputs.projects.insert(key, project);
                inputs.reused_projects += 1;
                continue;
            }
            inputs.scanned_projects += 1;
            let project = match scan_folder(&folder_path, sail_dir, &effective, overlays, inputs) {
                Ok(project) => project,
                Err(error) => {
                    let previous = previous.get(&key).cloned();
                    let context = match &previous {
                        Some(kept) => {
                            inputs.watch_files.extend(kept.watch_files.iter().cloned());
                            reuse_discovery.insert(key.clone());
                            "Keeping the previous project model; project configuration may be out of date."
                        }
                        None => "No project model is available for this project; its files receive syntax diagnostics only.",
                    };
                    failures.push((error, context));
                    previous
                }
            };
            if let Some(project) = project {
                inputs.projects.insert(key, project);
            }
        }
    }
    for config in std::iter::once(config).chain(scoped_configs.values().map(|c| &c.workspace)) {
        if let Some(active) = &config.active_project {
            if !inputs.projects.values().any(|project| {
                syntax::preprocess::normalize_path(&project.workspace_root.join(active))
                    == project.manifest
            }) {
                failures.push((format!("workspace.activeProject {active:?} does not name a loaded project."),
                "Using the first loaded manifest in path order for shared files. Select a loaded project to choose its context."));
            }
        }
    }
    // Module numbering and source discovery must share the same project order.
    // Keep one key sequence so these phases cannot acquire different tie breaks.
    let mut ordered_keys: Vec<_> = inputs.projects.keys().cloned().collect();
    ordered_keys.sort_by_key(|key| {
        let project = &inputs.projects[key];
        let active = project.config.active_project.as_ref().is_some_and(|active| {
            syntax::preprocess::normalize_path(&project.workspace_root.join(active))
                == project.manifest
        });
        (!active, project.manifest.clone())
    });
    for key in &ordered_keys {
        let project = &inputs.projects[key];
        let offset = modules.len();
        modules.extend(project.modules.iter().map(|(files, requires)| {
            (files.clone(), requires.iter().map(|id| offset + id).collect())
        }));
        source_roots.extend(
            project
                .source_roots
                .iter()
                .map(|(url, owner)| (url.clone(), owner.map(|id| offset + id))),
        );
    }
    if inputs.projects.is_empty() && !failures.is_empty() {
        return Err(failures.into_iter().map(|(error, _)| error).collect::<Vec<_>>().join("\n"));
    }
    inputs.project_errors =
        failures.into_iter().map(|(error, context)| format!("{error}\n{context}")).collect();

    if inputs.manifests.is_empty() {
        return Ok(Vec::new());
    }
    let library = sail_dir.map(|p| p.join("lib"));
    let mut candidates = Vec::new();
    for key in ordered_keys {
        if reuse_discovery.contains(&key) {
            candidates
                .extend(inputs.projects[&key].discovered_files.iter().map(|(_, p)| p.clone()));
            continue;
        }
        let roots: Vec<_> =
            inputs.projects[&key].files.iter().map(|(_, path)| path.clone()).collect();
        let project = &inputs.projects[&key];
        let search = include_search_paths(
            &[Url::from_directory_path(&project.workspace_root).unwrap()].into_iter().collect(),
            &project.config,
        );
        let mut watched = Vec::new();
        let discovered =
            hir::preprocess::discover_candidates(&roots, library.as_deref(), &search, |path| {
                watched.push(path.to_path_buf());
                overlays.get(path).cloned().or_else(|| std::fs::read_to_string(path).ok())
            });
        let project = inputs.projects.get_mut(&key).unwrap();
        project.watch_files.extend(watched.iter().cloned());
        project.watch_files.sort();
        project.watch_files.dedup();
        project.discovered_files = discovered
            .iter()
            .filter_map(|path| Url::from_file_path(path).ok().map(|url| (url, path.clone())))
            .collect();
        inputs.watch_files.extend(watched);
        candidates.extend(discovered);
    }
    inputs.watch_files.sort();
    inputs.watch_files.dedup();
    let mut seen = std::collections::HashSet::new();
    let result = candidates
        .into_iter()
        .filter(|path| seen.insert(syntax::preprocess::normalize_path(path)))
        .filter_map(|path| Url::from_file_path(&path).ok().map(|url| (url, path)))
        .collect();
    Ok(result)
}

/// Freeze presentation metadata alongside the model; a pending/failed config
/// reload must not describe the old database using newly requested settings.
pub(crate) fn loaded_module_contexts(
    projects: &std::collections::BTreeMap<Url, FolderProject>,
) -> Vec<serde_json::Value> {
    let mut projects: Vec<_> = projects.values().collect();
    projects.sort_by_key(|project| {
        let preferred = project.config.active_project.as_ref().is_some_and(|active| {
            syntax::preprocess::normalize_path(&project.workspace_root.join(active))
                == project.manifest
        });
        (!preferred, project.manifest.clone())
    });
    projects.into_iter().flat_map(|project| {
        (0..project.modules.len()).map(move |index| {
            let module = project.resolved.as_ref().and_then(|p| p.modules.get(index));
            serde_json::json!({
                "manifest": Url::from_file_path(&project.manifest).ok(),
                "module": module.map(|m| &m.name),
                "requires": module.into_iter().flat_map(|m| &m.requires)
                    .filter_map(|&i| project.resolved.as_ref()?.modules.get(i).map(|m| &m.name)).collect::<Vec<_>>(),
                "variables": project.config.project_variables,
                "roots": project.config.project_modules,
                "target": project.config.target,
                "includePaths": project.config.include_paths,
            })
        })
    }).collect()
}

impl GlobalState {
    /// Register `didChangeWatchedFiles` with the client for everything the
    /// loaded model reads.
    ///
    /// Only when the client offers dynamic registration; otherwise the
    /// loader below does the watching. Called when a scan starts and again
    /// when its model is installed, like rust-analyzer's
    /// `switch_workspaces`: the set follows the source roots — folders,
    /// include paths, the library, files reached outside all of them — and
    /// a client that already watches exactly that set is left alone. One
    /// that watches an older set is asked to drop it before the new one is
    /// registered. This used to register the workspace folders once, so a
    /// folder added later, an out-of-root include path and the library were
    /// never watched.
    pub(crate) fn register_file_watchers(&mut self) {
        if !self.client_caps.did_change_watched_files_dynamic_registration() {
            if !self.registered_file_watchers {
                self.start_file_watcher();
            }
            return;
        }
        // The client will watch; the loader still exists, to read.
        self.start_loader(Watching::No);

        let watchers = self.file_watchers();
        if watchers.is_empty() {
            return;
        }
        if self.client_file_watchers.as_ref().is_some_and(|(_, current)| *current == watchers) {
            return;
        }
        self.registered_file_watchers = true;
        if let Some((previous, _)) = self.client_file_watchers.take() {
            self.send_request::<lsp_types::request::UnregisterCapability>(
                lsp_types::UnregistrationParams {
                    unregisterations: vec![lsp_types::Unregistration {
                        id: previous,
                        method: "workspace/didChangeWatchedFiles".to_owned(),
                    }],
                },
                |_, _| (),
            );
        }
        self.file_watcher_registrations += 1;
        let id = format!("workspace/didChangeWatchedFiles/{}", self.file_watcher_registrations);
        self.client_file_watchers = Some((id.clone(), watchers.clone()));

        let registration = lsp_types::Registration {
            id,
            method: "workspace/didChangeWatchedFiles".to_owned(),
            register_options: serde_json::to_value(
                lsp_types::DidChangeWatchedFilesRegistrationOptions { watchers },
            )
            .ok(),
        };
        self.send_request::<lsp_types::request::RegisterCapability>(
            lsp_types::RegistrationParams { registrations: vec![registration] },
            |state, response| {
                if let Some(err) = response.error {
                    log::warn!("the client refused to watch our files: {}", err.message);
                    // It will not tell us, so we watch them ourselves.
                    state.client_file_watchers = None;
                    state.registered_file_watchers = false;
                    state.start_file_watcher();
                }
            },
        );
    }

    /// The watchers for [`Self::watched_roots`]: the Sail sources, the
    /// manifests and the shared configuration under each. A
    /// `RelativePattern` when the client takes one, an absolute glob
    /// otherwise — rust-analyzer's `reload.rs` makes the same choice.
    fn file_watchers(&self) -> Vec<lsp_types::FileSystemWatcher> {
        let relative = self.client_caps.did_change_watched_files_relative_pattern_support();
        self.watched_roots()
            .into_iter()
            .flat_map(|root| {
                [
                    "**/*.sail",
                    "**/*.sail_project",
                    "**/sail.proj",
                    crate::shared_configuration::FILE_NAME,
                ]
                .into_iter()
                .map(move |pattern| {
                    if relative {
                        lsp_types::GlobPattern::Relative(lsp_types::RelativePattern {
                            base_uri: lsp_types::OneOf::Right(
                                Url::from_directory_path(&root).expect("an absolute root"),
                            ),
                            pattern: pattern.to_owned(),
                        })
                    } else {
                        lsp_types::GlobPattern::String(format!(
                            "{}/{pattern}",
                            root.to_string_lossy()
                        ))
                    }
                })
            })
            .map(|glob_pattern| lsp_types::FileSystemWatcher { glob_pattern, kind: None })
            .collect()
    }

    /// The directories whose files the loaded model reads: the workspace
    /// folders, the library, every include search path, and the directory
    /// of each file a project reached outside all of those — a manifest
    /// linked from elsewhere, a source it lists or `$include`s by a path
    /// that leaves the folder. Sorted, and with no root inside another.
    fn watched_roots(&self) -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> =
            self.workspace_folders.iter().filter_map(|folder| folder.to_file_path().ok()).collect();
        if let Some(lib) = self.effective_sail_dir.as_ref().map(|dir| dir.join("lib")) {
            roots.push(lib);
        }
        for config in
            std::iter::once(&self.config.workspace).chain(self.projects.values().map(|p| &p.config))
        {
            roots.extend(include_search_paths(&self.workspace_folders, config));
        }
        let reached = self
            .projects
            .values()
            .flat_map(|project| {
                std::iter::once(&project.manifest)
                    .chain(project.watch_files.iter())
                    .chain(project.discovered_files.iter().map(|(_, path)| path))
            })
            .chain(self.discovery_watch_files.iter());
        let outside: Vec<PathBuf> = reached
            .filter(|file| !roots.iter().any(|root| file.starts_with(root)))
            .filter_map(|file| file.parent().map(std::path::Path::to_path_buf))
            .collect();
        roots.extend(outside);
        let mut roots: Vec<PathBuf> =
            roots.iter().map(|root| syntax::preprocess::normalize_path(root)).collect();
        roots.sort();
        roots.dedup();
        let all = roots.clone();
        roots.retain(|root| !all.iter().any(|other| other != root && root.starts_with(other)));
        roots
    }
}

impl GlobalState {
    /// Watch the workspace ourselves, because the client will not.
    pub(crate) fn start_file_watcher(&mut self) {
        self.start_loader(Watching::Yes);
    }

    /// The loader exists whether or not it watches.
    ///
    /// rust-analyzer always has one, because reading a file the server does
    /// not have is the loader's job and noticing one change is a *second*
    /// job it happens to do. Here it was created only for clients that
    /// cannot register watchers, so on VS Code `self.loader` was `None` and
    /// the one thing `didClose` wanted — "tell me what is on disk now" —
    /// had to be a bare `std::fs::read`. Half the file paths in the server
    /// went through the loader and half did not, which is the shape every
    /// defect in this area has had.
    pub(crate) fn start_loader(&mut self, watching: Watching) {
        if self.loader.is_some() {
            return;
        }
        let roots: Vec<paths::AbsPathBuf> = self
            .workspace_folders
            .iter()
            .filter_map(|folder| folder.to_file_path().ok())
            .filter_map(|path| path.to_str().and_then(|p| paths::AbsPathBuf::try_from(p).ok()))
            .collect();
        if roots.is_empty() {
            return;
        }

        let (sender, receiver) = crossbeam_channel::unbounded();
        let mut handle: Box<dyn vfs::loader::Handle> =
            Box::new(<vfs_notify::NotifyHandle as vfs::loader::Handle>::spawn(sender));
        // A watch is an index into `load`, so a loader that only answers
        // questions is configured with neither: the workspace scan is
        // already reading these files, and asking the OS to report them as
        // well would read every one of them twice at startup.
        // Configured only if it has something to do: the workspace scan
        // hands it the file list, and a config it cannot act on still
        // answers `Progress { Finished }`, which the load barrier would
        // otherwise take for the answer it was waiting for.
        if matches!(watching, Watching::Yes) {
            handle.set_config(vfs::loader::Config {
                version: 0,
                load: vec![vfs::loader::Entry::Directories(vfs::loader::Directories {
                    extensions: vec![
                        "sail".to_owned(),
                        "sail_project".to_owned(),
                        "proj".to_owned(),
                    ],
                    include: roots,
                    exclude: Vec::new(),
                })],
                watch: vec![0],
            });
        }
        self.loader = Some(handle);
        self.loader_receiver = Some(receiver);
        if matches!(watching, Watching::Yes) {
            self.registered_file_watchers = true;
        }
    }

    /// Hand the scan's file list to the loader to read.
    ///
    /// `Entry::Files` rather than a directory glob, so the loader reads
    /// exactly what the scan decided — a `.sail_project` names its files,
    /// and a walk would pull in the ones it left out. Returns whether a
    /// loader took it; without one the caller reads them itself.
    pub(crate) fn request_workspace_load(&mut self, files: &[(Url, PathBuf)]) -> bool {
        let paths: Vec<paths::AbsPathBuf> = files
            .iter()
            .filter_map(|(_, path)| path.to_str())
            .filter_map(|p| paths::AbsPathBuf::try_from(p).ok())
            .collect();
        if self.loader.is_none() {
            return false;
        }
        let version = self.workspace_load_version.wrapping_add(1);
        let mut watched_paths = paths;
        watched_paths.extend(
            self.discovery_watch_files
                .iter()
                .filter_map(|p| p.to_str().and_then(|p| paths::AbsPathBuf::try_from(p).ok())),
        );
        watched_paths.extend(
            self.workspace_folders.iter().filter_map(|root| root.to_file_path().ok()).filter_map(
                |root| {
                    paths::AbsPathBuf::try_from(
                        root.join(crate::shared_configuration::FILE_NAME)
                            .to_string_lossy()
                            .as_ref(),
                    )
                    .ok()
                },
            ),
        );
        watched_paths.sort();
        watched_paths.dedup();
        // Client glob support outside workspace folders varies. The native
        // watcher owns exact discovery dependencies for all clients, including
        // missing files and candidates preceding the selected search path.
        // Directory watches discover new roots/manifests without loading
        // inactive source files into the analysis database.
        let roots = self
            .workspace_folders
            .iter()
            .filter_map(|u| u.to_file_path().ok())
            .filter_map(|p| p.to_str().and_then(|p| paths::AbsPathBuf::try_from(p).ok()))
            .collect();
        let load = vec![
            vfs::loader::Entry::Files(watched_paths),
            vfs::loader::Entry::WatchDirectories(vfs::loader::Directories {
                extensions: vec!["sail".into(), "sail_project".into(), "proj".into()],
                include: roots,
                exclude: Vec::new(),
            }),
        ];
        let watch = vec![0, 1];
        let Some(loader) = self.loader.as_mut() else { return false };
        loader.set_config(vfs::loader::Config { version, load, watch });
        self.workspace_load_version = version;
        true
    }

    /// A batch of files the watcher saw change on disk.
    pub(crate) fn handle_vfs_msg(&mut self, msg: vfs::loader::Message) {
        let changed = matches!(&msg, vfs::loader::Message::Changed { .. });
        let files = match msg {
            vfs::loader::Message::WatchStatus { config_version, error } => {
                if config_version == self.workspace_load_version && self.watcher_error != error {
                    if let Some(error) = &error {
                        self.log_message(
                            lsp_types::MessageType::WARNING,
                            format!("File watching degraded: {error}"),
                        );
                    } else if self.watcher_error.is_some() {
                        self.log_message(
                            lsp_types::MessageType::INFO,
                            "Native file watching recovered",
                        );
                    }
                    self.watcher_error = error;
                }
                return;
            }
            vfs::loader::Message::Rescan { config_version, roots } => {
                if config_version != self.workspace_load_version {
                    return;
                }
                let full = roots.is_empty()
                    || self.workspace_folders.iter().filter_map(|u| u.to_file_path().ok()).any(
                        |folder| {
                            roots
                                .iter()
                                .any(|root| folder.starts_with(root.as_ref() as &std::path::Path))
                        },
                    );
                let projects: std::collections::BTreeSet<_> = self
                    .projects
                    .values()
                    .filter(|p| {
                        roots.iter().any(|root| {
                            let root = root.as_ref() as &std::path::Path;
                            p.manifest.starts_with(root)
                                || p.watch_files.iter().any(|f| f.starts_with(root))
                                || p.discovered_files.iter().any(|(_, f)| f.starts_with(root))
                        })
                    })
                    .map(|p| p.manifest.clone())
                    .collect();
                let scope = if full || projects.is_empty() {
                    ReloadScope::All
                } else {
                    ReloadScope::Projects(projects)
                };
                self.request_workspace_reload(
                    "file watcher requested rescan after lost events",
                    scope,
                );
                return;
            }
            vfs::loader::Message::Loaded { files } | vfs::loader::Message::Changed { files } => {
                files
            }
            // The barrier. Everything the loader was asked for has been
            // read, so the workspace can be assembled out of it — which is
            // rust-analyzer's `vfs_progress`/`vfs_done`, for the same
            // reason: `WorkspaceFiles` is the environment every cross-file
            // query resolves against, and building it early means a
            // workspace that briefly knows nothing.
            vfs::loader::Message::Progress {
                n_done: vfs::loader::LoadingProgress::Finished,
                config_version,
                ..
            } => {
                if config_version != self.workspace_load_version {
                    return;
                }
                if let Some((files, count)) = self.pending_workspace.take() {
                    self.process_changes();
                    self.finish_workspace_load(&files, count);
                }
                return;
            }
            vfs::loader::Message::Progress { .. } => return,
        };
        for (path, contents) in files {
            if crate::shared_configuration::is_config(path.as_ref() as &std::path::Path) {
                if changed {
                    self.refresh_shared_configuration();
                }
                continue;
            }
            if is_project_file(path.as_ref() as &std::path::Path) {
                if changed {
                    // A loaded manifest's change is that project's alone. A
                    // manifest no project owns is new, and a new manifest can
                    // change which one a folder loads, so the folder is
                    // rediscovered — the line the client-watcher path
                    // (`handle_did_change_watched_files`) draws too.
                    let manifest =
                        syntax::preprocess::normalize_path(path.as_ref() as &std::path::Path);
                    let scope = if self.projects.values().any(|p| p.manifest == manifest) {
                        ReloadScope::project(manifest)
                    } else {
                        ReloadScope::All
                    };
                    self.request_workspace_reload("project file changed", scope);
                }
                continue;
            }
            let Ok(url) = Url::from_file_path(path.as_ref() as &std::path::Path) else {
                continue;
            };
            if changed {
                let db = self.analysis_host.raw_database();
                // A discovered source is already known while its model is
                // being prepared. Watch registration can deliver initial
                // change events before that background work finishes. Treating
                // them as new files repeatedly cancels the very first load.
                let known = self
                    .loading_source_project
                    .iter()
                    .chain(
                        base_db::preprocess::WorkspaceSourceProject::try_get(db)
                            .map(|project| project.project(db)),
                    )
                    .any(|project| {
                        project.paths.iter().any(|(p, _)| {
                            syntax::preprocess::normalize_path(p)
                                == path.as_ref() as &std::path::Path
                        })
                    });
                if contents.is_none() || !known {
                    self.request_workspace_reload(
                        "source created, removed, or include candidate changed",
                        ReloadScope::All,
                    );
                }
            }
            match contents.and_then(|bytes| String::from_utf8(bytes).ok()) {
                Some(text) => self.sync_disk_text(&url, &text),
                // Deleted, or not text: an empty file is what the rest of
                // the server understands as "nothing here".
                None => self.sync_disk_text(&url, ""),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lost_events_rescan_affected_projects_and_ignore_stale_health() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        state.workspace_load_version = 7;
        let root = std::env::temp_dir().join("sail-watch-health");
        let folder = Url::from_directory_path(&root).unwrap();
        state.workspace_folders.insert(folder);
        let projects = ["a", "b"].map(|name| {
            let manifest = root.join(name).join("sail.proj");
            let file = root.join(name).join("main.sail");
            (
                Url::from_file_path(&manifest).unwrap(),
                FolderProject {
                    manifest,
                    workspace_root: root.clone(),
                    discovered_files: vec![(Url::from_file_path(&file).unwrap(), file)],
                    ..Default::default()
                },
            )
        });
        state.projects = std::sync::Arc::new(projects.into_iter().collect());
        let problem = Some("native watch limit reached".to_owned());
        state.handle_vfs_msg(vfs::loader::Message::WatchStatus {
            config_version: 6,
            error: problem.clone(),
        });
        assert!(state.watcher_error.is_none());
        state.handle_vfs_msg(vfs::loader::Message::WatchStatus {
            config_version: 7,
            error: problem,
        });
        assert!(state.current_status().message.unwrap().contains("watch limit"));
        state.handle_vfs_msg(vfs::loader::Message::Rescan { config_version: 6, roots: vec![] });
        assert!(state.pending_reload_scope.is_none());
        state.handle_vfs_msg(vfs::loader::Message::Rescan {
            config_version: 7,
            roots: vec![paths::AbsPathBuf::assert_utf8(root.join("a").join("main.sail"))],
        });
        assert_eq!(
            state.pending_reload_scope,
            Some(ReloadScope::project(root.join("a").join("sail.proj")))
        );
        state.handle_vfs_msg(vfs::loader::Message::WatchStatus { config_version: 7, error: None });
        assert!(state.watcher_error.is_none());
        assert!(!state.current_status().message.unwrap_or_default().contains("watch limit"));
    }

    #[test]
    fn enabling_autoimport_schedules_priming_without_project_reload() {
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let mut config = state.config.clone();
        config.completion.autoimport = true;
        state.update_configuration(config);
        assert!(state.prime_caches_queue.op_requested());
        assert!(!state.fetch_workspaces_queue.op_requested());
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "sail-project-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn write(&self, name: &str, source: &str) {
            let path = self.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, source).unwrap();
        }
        fn scan(&self, config: &crate::config::WorkspaceConfig) -> Result<Vec<String>, String> {
            let folders = HashSet::from([Url::from_directory_path(&self.0).unwrap()]);
            scan_workspace_folders(&folders, None, config).map(|files| {
                files
                    .into_iter()
                    .map(|(_, p)| p.file_name().unwrap().to_str().unwrap().to_owned())
                    .collect()
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A manifest the native watcher reports that no loaded project owns is
    /// new, and a new manifest can change which one a folder loads:
    /// discovery prefers `sail.proj` to `model.sail_project`. The
    /// client-watcher path reloads everything for an unknown manifest; the
    /// native path reloaded only "that manifest's project", which reused the
    /// folder's previous project — its manifest was not in scope — and the
    /// new manifest was ignored until the next full reload.
    #[test]
    fn a_new_manifest_from_the_native_watcher_reloads_the_whole_folder() {
        let fixture = Fixture::new();
        fixture.write("model.sail_project", "Main { files main.sail }");
        fixture.write("main.sail", "");
        let root = Url::from_directory_path(&fixture.0).unwrap();
        let old = fixture.0.join("model.sail_project");
        let new = fixture.0.join("sail.proj");
        let previous: std::collections::BTreeMap<_, _> = [(
            root.clone(),
            FolderProject {
                manifest: old.clone(),
                workspace_root: fixture.0.clone(),
                ..Default::default()
            },
        )]
        .into_iter()
        .collect();
        fixture.write("sail.proj", "Main { files main.sail }");
        let loaded = |scope: &ReloadScope| {
            let mut inputs = DiscoveryInputs::default();
            scan_workspace_project_scoped(
                &HashSet::from([root.clone()]),
                None,
                &Default::default(),
                &Default::default(),
                &mut Vec::new(),
                &mut Vec::new(),
                &Default::default(),
                &previous,
                scope,
                &mut inputs,
            )
            .unwrap();
            inputs.projects[&root].manifest.clone()
        };
        // Why the scope matters: scoped to the new manifest, the folder's
        // previous project is reused; only a full scan switches.
        assert_eq!(loaded(&ReloadScope::project(new.clone())), old);
        assert_eq!(loaded(&ReloadScope::All), new);

        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        state.workspace_folders.insert(root.clone());
        state.projects = std::sync::Arc::new(previous);
        let event = |path: &std::path::Path| vfs::loader::Message::Changed {
            files: vec![(
                paths::AbsPathBuf::assert_utf8(path.to_path_buf()),
                Some(b"Main { files main.sail }".to_vec()),
            )],
        };
        state.handle_vfs_msg(event(&old));
        assert_eq!(
            state.take_reload_scope(),
            ReloadScope::project(old.clone()),
            "a loaded manifest's change is that project's alone"
        );
        state.handle_vfs_msg(event(&new));
        assert_eq!(
            state.take_reload_scope(),
            ReloadScope::All,
            "a manifest no project loaded is new: the folder is rediscovered"
        );
    }

    #[test]
    fn workspace_scopes_discover_only_their_own_relative_include_paths() {
        let fixture = Fixture::new();
        for name in ["a", "b"] {
            fixture.write(&format!("{name}/sail.proj"), "Main { files main.sail }");
            fixture.write(&format!("{name}/main.sail"), "$include <choice.sail>\n");
            fixture.write(&format!("{name}/lib/choice.sail"), &format!("val from_{name} : int\n"));
        }
        let roots: Vec<_> = ["a", "b"]
            .into_iter()
            .map(|name| Url::from_directory_path(fixture.0.join(name)).unwrap())
            .collect();
        let scopes = roots
            .iter()
            .map(|root| {
                let mut config = SailLspConfig::default();
                config.workspace.include_paths = vec!["lib".into()];
                (root.clone(), config)
            })
            .collect();
        let mut inputs = DiscoveryInputs::default();
        scan_workspace_project_scoped(
            &roots.iter().cloned().collect(),
            None,
            &Default::default(),
            &scopes,
            &mut Vec::new(),
            &mut Vec::new(),
            &Default::default(),
            &Default::default(),
            &ReloadScope::All,
            &mut inputs,
        )
        .unwrap();
        for root in roots {
            let project = &inputs.projects[&root];
            let include = root.join("lib/choice.sail").unwrap();
            assert!(project.discovered_files.iter().any(|(uri, _)| uri == &include));
            assert!(project
                .discovered_files
                .iter()
                .all(|(_, path)| path.starts_with(root.to_file_path().unwrap())));
        }
    }

    #[test]
    fn absolute_linked_project_uses_its_containing_workspace_for_generated_files() {
        let fixture = Fixture::new();
        fixture.write("a/placeholder", "");
        fixture.write("z/sail.proj", "Main { files __virtual generated.sail }");
        fixture.write("z/generated.sail", "function f() = 0\n");
        let manifest = fixture.0.join("z/sail.proj");
        let config: crate::config::WorkspaceConfig = serde_json::from_value(serde_json::json!({
            "linkedProjects": [manifest, manifest],
            "generatedFiles": {"generated.sail": "generated.sail"},
        }))
        .unwrap();
        let folders = HashSet::from(
            ["a", "z"].map(|name| Url::from_directory_path(fixture.0.join(name)).unwrap()),
        );
        let mut inputs = DiscoveryInputs::default();
        let files = scan_workspace_project(
            &folders,
            None,
            &config,
            &mut Vec::new(),
            &mut Vec::new(),
            &Default::default(),
            &Default::default(),
            &ReloadScope::All,
            &mut inputs,
        )
        .unwrap();
        assert_eq!(inputs.projects.len(), 1, "absolute entries must not be loaded once per folder");
        let project = inputs.projects.values().next().unwrap();
        assert_eq!(project.workspace_root, fixture.0.join("z"));
        assert_eq!(
            files.iter().map(|(_, path)| path.clone()).collect::<Vec<_>>(),
            [fixture.0.join("z/generated.sail")]
        );
        assert!(inputs.project_errors.is_empty(), "{:?}", inputs.project_errors);
    }

    #[test]
    fn discovery_is_bounded_and_does_not_invent_a_project() {
        let f = Fixture::new();
        f.write("stray.sail", "function stray() = missing()\n");
        f.write("deep/nested/model.sail_project", "Main { files main.sail }");
        f.write("deep/nested/main.sail", "");
        assert_eq!(find_project_file(&f.0, None).unwrap(), None);
        assert!(f.scan(&Default::default()).unwrap().is_empty());
        let config = crate::config::WorkspaceConfig {
            project_file: Some("deep/nested/model.sail_project".into()),
            ..Default::default()
        };
        assert_eq!(f.scan(&config).unwrap(), ["main.sail"]);
    }

    #[test]
    fn nested_manifest_selection_preserves_precedence_and_rejects_ambiguity() {
        let f = Fixture::new();
        f.write("stray.sail", "");
        f.write("model/main.sail", "");
        f.write("model/riscv.sail_project", "Main { files main.sail }");
        f.write(".cache/ignored.sail_project", "Invalid {");
        assert_eq!(f.scan(&Default::default()).unwrap(), ["main.sail"]);

        f.write("model/sail.proj", "Empty { files [] }");
        assert!(f.scan(&Default::default()).unwrap().is_empty());
        f.write("other/other.sail_project", "Empty { files [] }");
        assert!(f.scan(&Default::default()).unwrap_err().contains("Multiple"));
        let config = crate::config::WorkspaceConfig {
            project_file: Some("model/riscv.sail_project".into()),
            ..Default::default()
        };
        assert_eq!(f.scan(&config).unwrap(), ["main.sail"]);

        f.write("root.sail_project", "Main { files stray.sail }");
        assert_eq!(f.scan(&Default::default()).unwrap(), ["stray.sail"]);
        f.write("sail.proj", "Empty { files [] }");
        assert!(f.scan(&Default::default()).unwrap().is_empty());
    }

    #[test]
    fn workspace_scan_evaluates_configuration_and_requirements() {
        let f = Fixture::new();
        for name in ["main.sail", "seq.sail", "rmem.sail", "stray.sail"] {
            f.write(name, "");
        }
        f.write("model.sail_project", "variable RMEM = false Main { requires Impl files main.sail } Impl { files if $RMEM then rmem.sail else seq.sail } Stray { files stray.sail }");
        let mut config = crate::config::WorkspaceConfig {
            project_modules: vec!["Main".into()],
            ..Default::default()
        };
        assert_eq!(f.scan(&config).unwrap(), ["seq.sail", "main.sail"]);
        config
            .project_variables
            .insert("RMEM".into(), crate::config::ProjectVariableValue::Bool(true));
        assert_eq!(f.scan(&config).unwrap(), ["rmem.sail", "main.sail"]);
        f.write("model.sail_project", "Empty { files [] }");
        config.project_modules.clear();
        assert!(f.scan(&config).unwrap().is_empty());
    }

    #[test]
    fn discovery_keeps_visibility_edges_separate_from_file_order() {
        let f = Fixture::new();
        f.write("a.sail", "");
        f.write("b.sail", "");
        let folders = HashSet::from([Url::from_directory_path(&f.0).unwrap()]);
        let mut previous = None;
        for dependency in ["after", "requires"] {
            f.write(
                "model.sail_project",
                &format!("A {{ files a.sail }} B {{ {dependency} A files b.sail }}"),
            );
            let mut modules = Vec::new();
            let files = scan_workspace_project(
                &folders,
                None,
                &Default::default(),
                &mut modules,
                &mut Vec::new(),
                &Default::default(),
                &Default::default(),
                &ReloadScope::All,
                &mut DiscoveryInputs::default(),
            )
            .unwrap();
            if let Some(previous) = &previous {
                assert_eq!(&files, previous);
            }
            previous = Some(files);
            assert_eq!(modules[0].0.len(), 1);
            assert_eq!(modules[1].0.len(), 1);
            assert_eq!(modules[1].1, if dependency == "requires" { vec![0] } else { vec![] });
        }
    }

    #[test]
    fn corelib_virtual_source_uses_the_effective_standard_library() {
        let fixture = Fixture::new();
        fixture.write("sail.proj", "corelib { files __virtual corelib.sail }");
        fixture.write("toolchain/lib/core/corelib.sail", "type generated_width = 8\n");
        let folders = HashSet::from([Url::from_directory_path(&fixture.0).unwrap()]);
        let files = scan_workspace_folders(
            &folders,
            Some(&fixture.0.join("toolchain")),
            &Default::default(),
        )
        .unwrap();
        assert!(files
            .iter()
            .any(|(_, path)| path == &fixture.0.join("toolchain/lib/core/corelib.sail")));
        assert!(!files.iter().any(|(_, path)| path == &fixture.0.join("corelib.sail")));
    }

    #[test]
    fn invalid_or_ambiguous_projects_never_fall_back_to_directory_scan() {
        let f = Fixture::new();
        f.write("stray.sail", "");
        for source in [
            "M {",
            "A { requires B } B { requires A }",
            "M { files if $MISSING then a.sail else b.sail }",
        ] {
            f.write("a.sail_project", source);
            assert!(f.scan(&Default::default()).is_err(), "{source}");
        }
        f.write("a.sail_project", "A { files [] }");
        f.write("b.sail_project", "B { files stray.sail }");
        assert!(f.scan(&Default::default()).unwrap_err().contains("Multiple"));
        let config = crate::config::WorkspaceConfig {
            project_file: Some("b.sail_project".into()),
            ..Default::default()
        };
        assert_eq!(f.scan(&config).unwrap(), ["stray.sail"]);
        f.write("sail.proj", "Preferred { files [] }");
        assert!(f.scan(&Default::default()).unwrap().is_empty());
    }

    /// One folder's broken manifest does not take the other folder's project
    /// down, and the broken one keeps what it last loaded.
    #[test]
    fn a_broken_folder_does_not_unload_its_neighbour() {
        let (good, bad) = (Fixture::new(), Fixture::new());
        good.write("good.sail_project", "G { files g.sail }");
        good.write("g.sail", "");
        bad.write("bad.sail_project", "B { files b.sail }");
        bad.write("b.sail", "");
        let good_url = Url::from_directory_path(&good.0).unwrap();
        let bad_url = Url::from_directory_path(&bad.0).unwrap();
        let folders = HashSet::from([good_url.clone(), bad_url.clone()]);
        let scan = |previous: &std::collections::BTreeMap<Url, FolderProject>| {
            let mut inputs = DiscoveryInputs::default();
            let files = scan_workspace_project(
                &folders,
                None,
                &Default::default(),
                &mut Vec::new(),
                &mut Vec::new(),
                &Default::default(),
                previous,
                &ReloadScope::All,
                &mut inputs,
            );
            let mut names: Vec<_> = files
                .map(|f| {
                    f.iter()
                        .map(|(_, p)| p.file_name().unwrap().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            names.sort();
            (names, inputs)
        };
        let (names, healthy) = scan(&Default::default());
        assert_eq!(names, ["b.sail", "g.sail"]);
        assert!(healthy.project_errors.is_empty());

        bad.write("bad.sail_project", "B { files b.sail");
        let (names, fresh) = scan(&Default::default());
        assert_eq!(names, ["g.sail"], "the healthy folder still loads");
        assert_eq!(fresh.project_errors.len(), 1);
        assert!(fresh.project_errors[0].contains("bad.sail_project"), "{:?}", fresh.project_errors);
        assert!(!fresh.projects.contains_key(&bad_url));

        let (names, kept) = scan(&healthy.projects);
        assert_eq!(names, ["b.sail", "g.sail"], "the broken folder keeps its last model");
        assert!(
            kept.project_errors[0].contains("previous project model"),
            "{:?}",
            kept.project_errors
        );
        assert_eq!(kept.projects[&bad_url].manifest, healthy.projects[&bad_url].manifest);

        good.write("good.sail_project", "G {");
        assert!(scan(&Default::default()).1.projects.is_empty());
        let mut inputs = DiscoveryInputs::default();
        assert!(
            scan_workspace_project(
                &folders,
                None,
                &Default::default(),
                &mut Vec::new(),
                &mut Vec::new(),
                &Default::default(),
                &Default::default(),
                &ReloadScope::All,
                &mut inputs
            )
            .is_err(),
            "with nothing loaded and nothing kept, the scan fails as before"
        );
    }

    #[test]
    fn a_manifest_reload_reuses_other_linked_projects() {
        let fixture = Fixture::new();
        fixture.write("a.sail_project", "A { files a1.sail }");
        fixture.write("b.sail_project", "B { files b.sail }");
        for file in ["a1.sail", "a2.sail", "b_dep.sail"] {
            fixture.write(file, "");
        }
        fixture.write("b.sail", "$include \"b_dep.sail\"\n");
        let folders = HashSet::from([Url::from_directory_path(&fixture.0).unwrap()]);
        let config: crate::config::WorkspaceConfig = serde_json::from_value(serde_json::json!({
            "linkedProjects": ["a.sail_project", "b.sail_project"]
        }))
        .unwrap();
        let scan = |previous: &std::collections::BTreeMap<Url, FolderProject>,
                    scope: &ReloadScope| {
            let mut inputs = DiscoveryInputs::default();
            let files = scan_workspace_project(
                &folders,
                None,
                &config,
                &mut Vec::new(),
                &mut Vec::new(),
                &Default::default(),
                previous,
                scope,
                &mut inputs,
            )
            .unwrap();
            (files, inputs)
        };
        let (_, initial) = scan(&Default::default(), &ReloadScope::All);
        assert_eq!((initial.scanned_projects, initial.reused_projects), (2, 0));
        let b = Url::from_file_path(fixture.0.join("b.sail_project")).unwrap();
        assert!(initial.projects[&b]
            .discovered_files
            .iter()
            .any(|(_, path)| path.ends_with("b_dep.sail")));

        fixture.write("a.sail_project", "A { files a2.sail }");
        // A scoped manifest event must not read B's source/include closure.
        // A real deletion also sends its own watcher event; removing it here
        // isolates the manifest scan and makes an accidental global traversal
        // observable because b_dep.sail would disappear from the result.
        std::fs::remove_file(fixture.0.join("b.sail")).unwrap();
        let (files, scoped) =
            scan(&initial.projects, &ReloadScope::project(fixture.0.join("a.sail_project")));
        assert_eq!((scoped.scanned_projects, scoped.reused_projects), (1, 1));
        let mut names: Vec<_> = files
            .iter()
            .map(|(_, path)| path.file_name().unwrap().to_string_lossy().into_owned())
            .filter(|name| matches!(name.as_str(), "a1.sail" | "a2.sail" | "b.sail" | "b_dep.sail"))
            .collect();
        names.sort();
        assert_eq!(names, ["a2.sail", "b.sail", "b_dep.sail"]);
        assert_eq!(scoped.projects[&b].manifest_text.as_deref(), Some("B { files b.sail }"));
    }

    #[test]
    fn recreated_known_manifest_keeps_reload_scoped() {
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let (tasks, _tasks_receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let root = std::env::temp_dir().join("known-manifest-watch");
        let known = syntax::preprocess::normalize_path(&root.join("known.sail_project"));
        std::sync::Arc::make_mut(&mut state.projects).insert(
            Url::from_directory_path(&root).unwrap(),
            FolderProject { manifest: known.clone(), ..Default::default() },
        );
        for (path, expected) in [
            (known.clone(), ReloadScope::project(known)),
            (root.join("new.sail_project"), ReloadScope::All),
        ] {
            crate::handlers::notification::handle_did_change_watched_files(
                &mut state,
                lsp_types::DidChangeWatchedFilesParams {
                    changes: vec![lsp_types::FileEvent {
                        uri: Url::from_file_path(path).unwrap(),
                        typ: lsp_types::FileChangeType::CREATED,
                    }],
                },
            )
            .unwrap();
            assert_eq!(state.take_reload_scope(), expected);
        }
    }

    #[test]
    fn manifest_watch_events_reload_without_parsing_manifests_as_sail() {
        let (sender, _receiver) = crossbeam_channel::unbounded();
        let (tasks, _tasks_receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let path = paths::AbsPathBuf::assert_utf8(std::env::temp_dir().join("watch.sail_project"));
        let files = vec![(path, Some(b"M {}".to_vec()))];
        state.handle_vfs_msg(vfs::loader::Message::Loaded { files: files.clone() });
        assert!(!state.fetch_workspaces_queue.op_requested());
        state.handle_vfs_msg(vfs::loader::Message::Changed { files });
        assert!(state.fetch_workspaces_queue.op_requested());
        assert!(state.analysis_host.raw_database().files().is_empty());
    }

    #[test]
    fn discovery_follows_includes_and_ignores_unreferenced_library_files() {
        use std::time::{SystemTime, UNIX_EPOCH};

        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let root = std::env::temp_dir().join(format!("sail_lsp_workspace_order_{nonce}"));
        let workspace = root.join("workspace");
        let sail_dir = root.join("sail");
        std::fs::create_dir_all(&workspace).expect("create workspace");
        std::fs::create_dir_all(sail_dir.join("lib")).expect("create Sail lib");
        std::fs::write(workspace.join("model.sail_project"), "model {\n  files main.sail\n}\n")
            .expect("write project file");
        std::fs::write(workspace.join("main.sail"), "$include <z.sail>\nfunction main() = ()\n")
            .expect("write project source");
        std::fs::write(workspace.join("not-listed.sail"), "function stray() = ()\n")
            .expect("write unlisted source");
        std::fs::write(sail_dir.join("lib/z.sail"), "$include \"a.sail\"\nval z : unit\n")
            .expect("write library source");
        std::fs::write(sail_dir.join("lib/a.sail"), "val a : unit\n")
            .expect("write library source");
        std::fs::write(sail_dir.join("lib/unused.sail"), "val unused : unit\n").unwrap();

        let folder = Url::from_directory_path(&workspace).expect("workspace URL");
        let scanned =
            scan_workspace_folders(&HashSet::from([folder]), Some(&sail_dir), &Default::default())
                .unwrap();
        let names: Vec<_> = scanned
            .iter()
            .map(|(_, path)| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, ["main.sail", "z.sail", "a.sail"]);
        assert!(!names.iter().any(|name| name == "not-listed.sail"));

        let _ = std::fs::remove_dir_all(root);
    }
}
