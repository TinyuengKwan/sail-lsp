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
    /// Manifest contents used to derive membership and module edges. Checking
    /// again at publication also covers delayed OS/client notifications.
    pub manifests: std::collections::BTreeMap<PathBuf, Option<String>>,
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
        if self.workspace_loaded && self.project_manifests.is_empty() {
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

    /// Apply configuration changes and trigger reload if needed.
    /// Compares old and new config, scheduling a workspace reload if
    /// structural settings (search paths, workspace folders) changed.
    pub(crate) fn update_configuration(&mut self, new_config: SailLspConfig) {
        let old_config = std::mem::replace(&mut self.config, new_config);

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
        &mut DiscoveryInputs::default(),
    )
}

fn scan_workspace_project(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
    config: &crate::config::WorkspaceConfig,
    modules: &mut Vec<(Vec<Url>, Vec<usize>)>,
    source_roots: &mut Vec<(Url, Option<usize>)>,
    overlays: &std::collections::HashMap<PathBuf, String>,
    inputs: &mut DiscoveryInputs,
) -> Result<Vec<(Url, PathBuf)>, String> {
    let mut local_files = Vec::new();
    let search = include_search_paths(folders, config);
    let mut folders: Vec<_> = folders.iter().collect();
    folders.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    for folder_url in folders {
        let Ok(folder_path) = folder_url.to_file_path() else {
            continue;
        };
        let mut folder_files = Vec::new();
        if let Some(configured) = &config.project_file {
            let path = syntax::preprocess::normalize_path(&folder_path.join(configured));
            inputs.manifests.insert(path.clone(), std::fs::read_to_string(&path).ok());
            inputs.watch_files.push(path);
        }

        if let Some(project_file_path) =
            find_project_file(&folder_path, config.project_file.as_deref())?
        {
            inputs.watch_files.push(project_file_path.clone());
            let source = std::fs::read_to_string(&project_file_path);
            inputs.manifests.insert(project_file_path.clone(), source.as_ref().ok().cloned());
            let source = source.map_err(|e| format!("{}: {e}", project_file_path.display()))?;
            let project = project_model::ProjectManifest::parse(&source)
                .and_then(|manifest| manifest.resolve(&config.project_options()))
                .map_err(|error| format!("{}: {error}", project_file_path.display()))?;
            let project_dir = project_file_path.parent().unwrap_or(&folder_path);
            let offset = modules.len();
            for (id, module) in project.modules.iter().enumerate() {
                let files = if project.module_order.contains(&id) {
                    module
                        .files
                        .iter()
                        .filter_map(|p| Url::from_file_path(project_dir.join(p)).ok())
                        .collect()
                } else {
                    Vec::new()
                };
                modules.push((files, module.requires.iter().map(|id| offset + id).collect()));
            }
            // Keep semantic root occurrences and their owners even when the
            // same path belongs to several modules. Discovery still dedups I/O.
            for &id in &project.module_order {
                for path in &project.modules[id].files {
                    if let Ok(url) = Url::from_file_path(project_dir.join(path)) {
                        source_roots.push((url, Some(offset + id)));
                    }
                }
            }
            for relative_path in project.files {
                let path = project_dir.join(relative_path);
                inputs.watch_files.push(syntax::preprocess::normalize_path(&path));
                if !path.is_file() {
                    return Err(format!("Project source does not exist: {}", path.display()));
                }
                if is_sail_file(&path) {
                    let uri = Url::from_file_path(&path)
                        .map_err(|_| format!("Invalid project path: {}", path.display()))?;
                    folder_files.push((uri, path));
                }
            }
            local_files.extend(folder_files);
            // An explicitly empty configuration is still a project. Directory
            // fallback would reintroduce every inactive file.
            continue;
        }

        // No manifest means no semantic project. Opening a buffer still
        // supplies its text through the VFS, but directory order cannot supply
        // Sail declaration order or module membership.
    }

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
    fn invalid_or_ambiguous_projects_never_fall_back_to_directory_scan() {
        let f = Fixture::new();
        f.write("stray.sail", "");
        for source in [
            "M {",
            "M { files missing.sail }",
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
