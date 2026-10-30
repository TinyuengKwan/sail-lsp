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
    /// Every module the manifest declares, in declaration order.
    pub declarations: Vec<ModuleDeclaration>,
    /// Project source files that exist, each once.
    pub files: Vec<(Url, PathBuf)>,
    pub modules: Vec<(Vec<Url>, Vec<usize>)>,
    pub source_roots: Vec<(Url, Option<usize>)>,
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
    /// Is the server quiescent?
    /// Returns true when workspace has been fully loaded and no
    /// background tasks are pending.
    /// Full quiescence check — all background operations complete.
    /// checks VFS, workspace fetch, build data, proc macros.
    /// Sail checks: workspace fetch, prime caches, workspace context,
    /// and dirty file index.
    /// index_dirty_files is NOT checked here — it's drained inside
    pub(crate) fn is_quiescent(&self) -> bool {
        !self.fetch_workspaces_queue.op_in_progress()
    }

    /// Compute current server status for the client.
    /// Returns a status struct with health, quiescent flag, and
    /// optional message describing any issues.
    pub(crate) fn current_status(&self) -> crate::lsp_ext::ServerStatusParams {
        let loading = self.fetch_workspaces_queue.op_in_progress()
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
    }

    /// Apply configuration changes and trigger reload if needed.
    /// Compares old and new config, scheduling a workspace reload if
    /// structural settings (search paths, workspace folders) changed.
    /// Apply settings as the client sent them, and tell the user about any
    /// it could not read — once per distinct set, so an unrelated settings
    /// change does not repeat the popup.
    pub(crate) fn apply_client_settings(&mut self, settings: &serde_json::Value) {
        let (config, errors) = SailLspConfig::from_json(settings);
        self.update_configuration(config);
        if errors == self.config_errors {
            return;
        }
        if !errors.is_empty() {
            let message = format!(
                "sail-lsp: these settings were not understood and use their defaults: {}",
                errors.join("; ")
            );
            self.log_message(lsp_types::MessageType::WARNING, message.clone());
            self.send_notification::<lsp_types::notification::ShowMessage>(
                lsp_types::ShowMessageParams { typ: lsp_types::MessageType::WARNING, message },
            );
        }
        self.config_errors = errors;
    }

    pub(crate) fn update_configuration(&mut self, new_config: SailLspConfig) {
        let old_config = std::mem::replace(&mut self.config, new_config);

        if self.config.workspace != old_config.workspace
            || self.config.flycheck != old_config.flycheck
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
            self.fetch_workspaces_queue.request_op("configuration changed".to_string(), ());
        }

        // Diagnostics are re-computed lazily on next request;
        // no explicit invalidation needed when diagnostics config changes.
    }

    /// Spawn a background workspace scan.
    /// Called from the main loop when `fetch_workspaces_queue.should_start_op()`
    /// returns `Some`. Scans workspace folders for `.sail` files and sends
    /// results back via `Task::WorkspaceScan`.
    pub(crate) fn fetch_workspaces(&mut self, cause: String) {
        log::info!("fetching workspaces: {cause}");
        self.invalidate_flycheck();

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
                )
            });
            if let Some(ref p) = progress {
                p.report("scanning files...", Some(10));
            }

            let mut modules = Vec::new();
            let mut source_roots = Vec::new();
            let mut inputs = DiscoveryInputs::default();
            let files = scan_workspace_project(
                &folders,
                effective_sail_dir.as_deref(),
                &config,
                &mut modules,
                &mut source_roots,
                &overlays,
                &previous,
                &mut inputs,
            );
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
        let mut project =
            FolderProject { manifest: project_file_path.clone(), ..Default::default() };
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
        for relative_path in resolved.files {
            let path = project_dir.join(&relative_path);
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
                        let k = m.files.iter().position(|f| *f == relative_path)?;
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
        // An explicitly empty configuration is still a project. Directory
        // fallback would reintroduce every inactive file.
        Ok(Some(project))
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_workspace_project(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
    modules: &mut Vec<(Vec<Url>, Vec<usize>)>,
    source_roots: &mut Vec<(Url, Option<usize>)>,
    overlays: &std::collections::HashMap<PathBuf, String>,
    previous: &std::collections::BTreeMap<Url, FolderProject>,
    inputs: &mut DiscoveryInputs,
) -> Result<Vec<(Url, PathBuf)>, String> {
    let search = include_search_paths(folders, config);
    let mut folders: Vec<_> = folders.iter().collect();
    folders.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let mut local_files = Vec::new();
    let mut failures = Vec::new();
    for folder_url in folders {
        let Ok(folder_path) = folder_url.to_file_path() else {
            continue;
        };
        // One folder's broken manifest is that project's problem, as one
        // failed Cargo workspace is in rust-analyzer: the others still load,
        // and the broken one keeps the model it last loaded, if any.
        let project = match scan_folder(&folder_path, sail_dir, config, overlays, inputs) {
            Ok(project) => project,
            Err(error) => {
                let previous = previous.get(folder_url).cloned();
                let context = match &previous {
                    Some(kept) => {
                        inputs.watch_files.extend(kept.files.iter().map(|(_, p)| p.clone()));
                        "Keeping the previous project model; project configuration may be out of date."
                    }
                    None => "No project model is available for this folder; its files receive syntax diagnostics only.",
                };
                failures.push((error, context));
                previous
            }
        };
        // No manifest means no semantic project. Opening a buffer still
        // supplies its text through the VFS, but directory order cannot supply
        // Sail declaration order or module membership.
        let Some(project) = project else { continue };
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
        local_files.extend(project.files.iter().cloned());
        inputs.projects.insert(folder_url.clone(), project);
    }
    if inputs.projects.is_empty() && !failures.is_empty() {
        return Err(failures.into_iter().map(|(error, _)| error).collect::<Vec<_>>().join("\n"));
    }
    inputs.project_errors =
        failures.into_iter().map(|(error, context)| format!("{error}\n{context}")).collect();

    if inputs.manifests.is_empty() {
        return Ok(Vec::new());
    }
    let roots: Vec<_> = local_files.iter().map(|(_, p)| p.clone()).collect();
    let library = sail_dir.map(|p| p.join("lib"));
    let candidates =
        hir::preprocess::discover_candidates(&roots, library.as_deref(), &search, |path| {
            inputs.watch_files.push(path.to_path_buf());
            overlays.get(path).cloned().or_else(|| std::fs::read_to_string(path).ok())
        });
    inputs.watch_files.sort();
    inputs.watch_files.dedup();
    let result = candidates
        .into_iter()
        .filter_map(|path| Url::from_file_path(&path).ok().map(|url| (url, path)))
        .collect();
    Ok(result)
}

impl GlobalState {
    /// Register `didChangeWatchedFiles` with the client, once.
    ///
    /// Only when the client offers dynamic registration; otherwise the
    /// loader below does the watching.
    pub(crate) fn register_file_watchers(&mut self) {
        if self.registered_file_watchers {
            return;
        }
        if !self.client_caps.did_change_watched_files_dynamic_registration() {
            self.start_file_watcher();
            return;
        }
        // The client will watch; the loader still exists, to read.
        self.start_loader(Watching::No);
        self.registered_file_watchers = true;

        let watchers: Vec<lsp_types::FileSystemWatcher> = self
            .workspace_folders
            .iter()
            .filter_map(|folder| folder.to_file_path().ok())
            .flat_map(|root| {
                let root = root.to_string_lossy().into_owned();
                [
                    format!("{root}/**/*.sail"),
                    format!("{root}/**/*.sail_project"),
                    format!("{root}/**/sail.proj"),
                ]
            })
            .map(|glob_pattern| lsp_types::FileSystemWatcher {
                glob_pattern: lsp_types::GlobPattern::String(glob_pattern),
                kind: None,
            })
            .collect();
        if watchers.is_empty() {
            self.registered_file_watchers = false;
            return;
        }

        let registration = lsp_types::Registration {
            id: "workspace/didChangeWatchedFiles".to_owned(),
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
                    state.registered_file_watchers = false;
                    state.start_file_watcher();
                }
            },
        );
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
            if is_project_file(path.as_ref() as &std::path::Path) {
                if changed {
                    self.fetch_workspaces_queue.request_op("project file changed".into(), ());
                }
                continue;
            }
            let Ok(url) = Url::from_file_path(path.as_ref() as &std::path::Path) else {
                continue;
            };
            if changed {
                let db = self.analysis_host.raw_database();
                let known = base_db::preprocess::WorkspaceSourceProject::try_get(db).is_some_and(
                    |project| {
                        project.project(db).paths.iter().any(|(p, _)| {
                            syntax::preprocess::normalize_path(p)
                                == path.as_ref() as &std::path::Path
                        })
                    },
                );
                if contents.is_none() || !known {
                    self.fetch_workspaces_queue.request_op(
                        "source created, removed, or include candidate changed".into(),
                        (),
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
                &mut inputs
            )
            .is_err(),
            "with nothing loaded and nothing kept, the scan fails as before"
        );
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
