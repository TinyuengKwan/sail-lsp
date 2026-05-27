//! Preprocessor symbol definitions and include options.
//!
//! The actual conditional-compilation logic (ifdef/define/include) now
//! lives in `hir_def::item_tree::build_from_cst_with_preprocess` for
//! CST-level preprocessing, and in `parsing` for token-level
//! balance checking. This module retains only the shared types:
//! `default_symbols`, `PreprocessOptions`, and `IncludeReader`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Default preprocessor symbols for the Sail compiler.
pub fn default_symbols() -> HashSet<String> {
    [
        "FEATURE_IMPLICITS",
        "FEATURE_CONSTANT_TYPES",
        "FEATURE_BITVECTOR_TYPE",
        "FEATURE_UNION_BARRIER",
        "FEATURE_STRICT_VAR",
        "FEATURE_STRICT_BITVECTOR",
        "FEATURE_STRICT_EXPONENTIALS",
    ]
    .into_iter()
    .map(String::from)
    .collect()
}

/// Callback type the preprocessor uses to read included files.
pub type IncludeReader = Arc<dyn Fn(&Path) -> Option<String> + Send + Sync>;

/// Options driving the preprocessor's `$include` resolution.
#[derive(Clone)]
pub struct PreprocessOptions {
    pub current_file_dir: Option<PathBuf>,
    pub search_dirs: Vec<PathBuf>,
    pub sail_dir: Option<PathBuf>,
    pub max_include_depth: usize,
    pub reader: IncludeReader,
    /// Current compilation target for `$iftarget` directives.
    ///
    /// - `Some("c")`: target-specific build, `$iftarget c` is taken
    /// - `None`: LSP mode — both branches are taken (over-indexing)
    pub target: Option<String>,
}

impl Default for PreprocessOptions {
    fn default() -> Self {
        Self {
            current_file_dir: None,
            search_dirs: Vec::new(),
            sail_dir: None,
            max_include_depth: 32,
            reader: Arc::new(|_| None),
            target: None,
        }
    }
}

impl PreprocessOptions {
    /// Build a child options object for an included file.
    pub fn child_for_included(&self, included_path: &Path) -> Self {
        Self {
            current_file_dir: included_path.parent().map(Path::to_path_buf),
            search_dirs: self.search_dirs.clone(),
            sail_dir: self.sail_dir.clone(),
            max_include_depth: self.max_include_depth,
            reader: Arc::clone(&self.reader),
            target: self.target.clone(),
        }
    }
}

impl std::fmt::Debug for PreprocessOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreprocessOptions")
            .field("current_file_dir", &self.current_file_dir)
            .field("search_dirs", &self.search_dirs)
            .field("sail_dir", &self.sail_dir)
            .field("max_include_depth", &self.max_include_depth)
            .field("reader", &"<callback>")
            .finish()
    }
}
