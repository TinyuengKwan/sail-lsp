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

/// Progress for workspace loading.
#[derive(Debug)]
#[allow(dead_code)] // WIP: workspace loading progress reporting
pub(crate) enum WorkspaceProgress {
    Begin,
    Report(String),
    End(usize), // number of files loaded
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

/// Check if a file path is a Sail project file.
pub fn is_project_file(path: &std::path::Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name == "sail.proj" || name.ends_with(".sail_project")
    })
}

/// Find a `.sail_project` or `sail.proj` file in the given folder.
fn find_project_file(folder: &std::path::Path) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(folder).ok()? {
        let entry = entry.ok()?;
        let path = entry.path();
        if is_project_file(&path) {
            return Some(path);
        }
    }
    None
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

        // Check if files are loaded
        let file_count = self.analysis_host.raw_database().files().len();
        if file_count == 0 && !self.workspace_folders.is_empty() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("No Sail files loaded. Check workspace folders.\n");
        }

        // Check for include graph cycles
        if self.include_graph.find_cycle().is_some() {
            status.health = crate::lsp_ext::Health::Warning;
            message.push_str("Circular $include dependency detected.\n");
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

        let folders = self.workspace_folders.clone();
        if folders.is_empty() {
            // No folders to scan — mark op as completed immediately.
            if self.fetch_workspaces_queue.op_in_progress() {
                self.fetch_workspaces_queue.op_completed(());
            }
            return;
        }

        let sender = self.sender.clone();
        let supports_progress = self.supports_progress;
        let effective_sail_dir = self.effective_sail_dir.clone();

        self.task_pool.spawn(move || {
            let progress = if supports_progress {
                Some(crate::progress::ProgressReporter::begin(&sender, "Reloading workspace"))
            } else {
                None
            };
            if let Some(ref p) = progress {
                p.report("scanning files...", Some(10));
            }

            let discovered = scan_workspace_folders(&folders, effective_sail_dir.as_deref());

            if let Some(ref p) = progress {
                p.report(&format!("reading {} files...", discovered.len()), Some(50));
            }

            // Read file contents, deduplicating by URL
            let mut seen = HashSet::new();
            let files: Vec<_> = discovered
                .into_iter()
                .filter(|(uri, _)| seen.insert(uri.clone()))
                .filter_map(|(uri, path)| {
                    let text = std::fs::read_to_string(&path).ok()?;
                    Some((uri, path, text))
                })
                .collect();

            let count = files.len();
            if let Some(p) = progress {
                p.end(&format!("{count} files loaded"));
            }

            crate::main_loop::Task::WorkspaceScan { files }
        });
    }
}

/// Scan workspace folders for `.sail` files.
/// Returns `(url, path)` pairs for all discovered files.
/// respecting project structure.
fn scan_workspace_folders(
    folders: &HashSet<Url>,
    sail_dir: Option<&std::path::Path>,
) -> Vec<(Url, PathBuf)> {
    let mut result = Vec::new();
    for folder_url in folders {
        let Ok(folder_path) = folder_url.to_file_path() else {
            continue;
        };

        // Check for project file first — use project_model::parse_project()
        // directly (投産-2: prefer project-model crate over hir-def re-export).
        if let Some(project_file_path) = find_project_file(&folder_path) {
            log::info!("found project file: {}", project_file_path.display());
            if let Ok(source) = std::fs::read_to_string(&project_file_path) {
                match project_model::parse_project(&source) {
                    Ok(project) => {
                        log::info!(
                            "parsed project file: {} entries, {} modules",
                            project.files.len(),
                            project.modules.len(),
                        );
                        // Resolve file paths relative to the project file's directory
                        let project_dir = project_file_path.parent().unwrap_or(&folder_path);
                        for relative_path in &project.files {
                            let abs_path = project_dir.join(relative_path);
                            if abs_path.exists() && is_sail_file(&abs_path) {
                                if let Ok(uri) = Url::from_file_path(&abs_path) {
                                    result.push((uri, abs_path));
                                }
                            }
                        }
                        // If the project file listed files, skip the directory walk
                        // for this folder to respect the project's file ordering.
                        if !project.files.is_empty() {
                            continue;
                        }
                    }
                    Err(e) => {
                        log::warn!(
                            "failed to parse project file {}: {e}",
                            project_file_path.display()
                        );
                        // Fall through to directory scan
                    }
                }
            }
        }

        let walker = walkdir::WalkDir::new(&folder_path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok());

        for entry in walker {
            let path = entry.path();
            if is_sail_file(path) {
                if let Ok(uri) = Url::from_file_path(path) {
                    result.push((uri, path.to_path_buf()));
                }
            }
        }
    }

    // Also scan Sail standard library ($SAIL_DIR or embedded).
    if let Some(sail_dir) = sail_dir {
        let lib_dir = sail_dir.join("lib");
        if lib_dir.is_dir() {
            let walker = walkdir::WalkDir::new(&lib_dir)
                .follow_links(true)
                .max_depth(5)
                .into_iter()
                .filter_map(|e| e.ok());
            for entry in walker {
                let path = entry.path();
                if is_sail_file(path) {
                    if let Ok(uri) = Url::from_file_path(path) {
                        result.push((uri, path.to_path_buf()));
                    }
                }
            }
            log::info!("scanned Sail stdlib: {}", lib_dir.display());
        }
    }

    result
}
