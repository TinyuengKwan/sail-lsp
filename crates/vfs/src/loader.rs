//! File loading and watching interface.
//!
//! File content is `Option<Vec<u8>>` (raw bytes), same as ra.

use std::fmt;

use paths::{AbsPath, AbsPathBuf};

/// A set of files to load.
#[derive(Debug, Clone)]
pub enum Entry {
    /// Explicit list of files.
    Files(Vec<AbsPathBuf>),
    /// Directory scan with extension/exclusion filters.
    Directories(Directories),
}

/// Directory scanning configuration.
#[derive(Debug, Clone, Default)]
pub struct Directories {
    /// File extensions to include (e.g., `["sail"]`).
    pub extensions: Vec<String>,
    /// Directories to include.
    pub include: Vec<AbsPathBuf>,
    /// Directories to exclude.
    pub exclude: Vec<AbsPathBuf>,
}

impl Directories {
    /// Check if a file path is covered by this directory config.
    pub fn contains_file(&self, path: &AbsPath) -> bool {
        self.includes_path(path)
            && self.extensions.iter().any(|ext| path.extension().is_some_and(|e| e == ext.as_str()))
    }

    /// Check if a directory path is covered.
    pub fn contains_dir(&self, path: &AbsPath) -> bool {
        self.includes_path(path)
    }

    // We use the simpler any/none since Sail projects have fewer roots.
    fn includes_path(&self, path: &AbsPath) -> bool {
        let dominated = self.include.iter().any(|inc| path.starts_with(inc));
        let excluded = self.exclude.iter().any(|exc| path.starts_with(exc));
        dominated && !excluded
    }
}

impl Entry {
    // `cargo_package_dependency` — we have a single Sail equivalent.
    /// Create an entry for recursively scanning `.sail` files.
    pub fn sail_files_recursively(base: AbsPathBuf) -> Entry {
        Entry::Directories(Directories {
            extensions: vec!["sail".to_string()],
            include: vec![base],
            exclude: Vec::new(),
        })
    }

    /// Check if a file is contained in this entry.
    pub fn contains_file(&self, path: &AbsPath) -> bool {
        match self {
            Entry::Files(files) => files.iter().any(|f| f.as_path() == path),
            Entry::Directories(dirs) => dirs.contains_file(path),
        }
    }

    /// Check if a directory is contained in this entry.
    pub fn contains_dir(&self, path: &AbsPath) -> bool {
        match self {
            Entry::Files(_) => false,
            Entry::Directories(dirs) => dirs.contains_dir(path),
        }
    }
}

/// Configuration for the file loader.
#[derive(Debug)]
pub struct Config {
    /// Version number for tracking config changes.
    pub version: u32,
    /// File sets to initially load.
    pub load: Vec<Entry>,
    /// Indices into `load` for which to enable file watching.
    pub watch: Vec<usize>,
}

/// Loading progress indication.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LoadingProgress {
    Started,
    Progress(usize),
    Finished,
}

/// Messages from the loader to the main loop.
pub enum Message {
    /// Loading progress update.
    Progress {
        n_total: usize,
        n_done: LoadingProgress,
        dir: Option<AbsPathBuf>,
        config_version: u32,
    },
    /// Files loaded from disk.
    Loaded { files: Vec<(AbsPathBuf, Option<Vec<u8>>)> },
    /// Files changed on disk (from file watcher).
    Changed { files: Vec<(AbsPathBuf, Option<Vec<u8>>)> },
}

/// Sender type for loader messages.
pub type Sender = crossbeam_channel::Sender<Message>;

/// Trait for file loading and watching.
/// Implementations provide async file I/O and optional file watching.
/// Messages flow back to the main loop via `Sender`.
pub trait Handle: fmt::Debug {
    /// Spawn a new loader with the given message sender.
    fn spawn(sender: Sender) -> Self
    where
        Self: Sized;

    /// Set the loader configuration (which files/directories to load/watch).
    fn set_config(&mut self, config: Config);

    /// Invalidate a path (force re-read on next access).
    fn invalidate(&mut self, path: AbsPathBuf);

    /// Synchronously load a file's contents as raw bytes.
    fn load_sync(&mut self, path: &AbsPath) -> Option<Vec<u8>>;
}
