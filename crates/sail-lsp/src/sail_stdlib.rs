//! Embedded Sail standard library.
//!
//! The upstream `sail/lib/` directory is vendored into `data/sail-lib/` and
//! compiled into the binary via [`include_dir`].  When `$SAIL_DIR` is **not**
//! set, the server materialises these files into a temporary directory so the
//! existing workspace-scan and include-resolution logic can pick them up
//! unchanged.

use include_dir::{include_dir, Dir};
use std::path::{Path, PathBuf};

/// All `.sail` files from `sail/lib/`, embedded at compile time.
static SAIL_STDLIB: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/data/sail-lib");

/// Materialise the embedded stdlib into `parent_dir/lib/` and return the path
/// that should be used as the effective `SAIL_DIR` (i.e. `parent_dir` itself,
/// so that `$SAIL_DIR/lib/<file>` works).
///
/// The caller is responsible for keeping the returned [`PathBuf`] (or the
/// `TempDir` that owns `parent_dir`) alive for the lifetime of the server.
pub(crate) fn materialise_stdlib(parent_dir: &Path) -> std::io::Result<PathBuf> {
    let lib_dir = parent_dir.join("lib");
    extract_dir(&SAIL_STDLIB, &lib_dir)?;
    Ok(parent_dir.to_path_buf())
}

/// Recursively extract an [`include_dir::Dir`] to disk.
fn extract_dir(dir: &Dir<'_>, dest: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dest)?;
    for file in dir.files() {
        let target = dest.join(file.path().file_name().unwrap());
        std::fs::write(&target, file.contents())?;
    }
    for sub in dir.dirs() {
        let sub_name = sub.path().file_name().unwrap();
        extract_dir(sub, &dest.join(sub_name))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_stdlib_has_files() {
        let count = SAIL_STDLIB.files().count()
            + SAIL_STDLIB.dirs().map(|d| d.files().count()).sum::<usize>();
        // We expect ~60 .sail files (top-level + float/ + concurrency_interface/).
        assert!(count >= 30, "expected at least 30 stdlib files, got {count}");
    }

    #[test]
    fn materialise_round_trip() {
        let tmp = std::env::temp_dir().join("sail_lsp_stdlib_test");
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let sail_dir = materialise_stdlib(&tmp).unwrap();
        assert!(sail_dir.join("lib").join("prelude.sail").exists());
        assert!(sail_dir.join("lib").join("float").join("common.sail").exists());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
