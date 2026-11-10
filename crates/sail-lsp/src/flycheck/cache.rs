//! Compiler memo files belong to a project and toolchain, not an editor buffer.
//! Checks use private copies and publish completed caches atomically. Cancelled
//! runs cannot leave a truncated shared memo file behind.
use crate::config::{CompilerCacheMode, FlycheckConfig};
use std::{
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CacheInfo {
    pub mode: CompilerCacheMode,
    pub file: Option<PathBuf>,
    pub existing_at_start: bool,
    pub bytes_at_start: u64,
    pub bytes_after_check: u64,
    pub warning: Option<String>,
}

pub(crate) struct PreparedCache {
    pub args: Vec<String>,
    pub info: CacheInfo,
    work: Option<tempfile::TempDir>,
}

fn size(path: &Path) -> u64 {
    std::fs::metadata(path).ok().filter(|m| m.is_file()).map_or(0, |m| m.len())
}

fn default_directory() -> Result<PathBuf, String> {
    let home = || crate::install::home().map_err(|e| e.to_string());
    let root = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| home().map(|p| p.join("AppData/Local")))?
    } else if cfg!(target_os = "macos") {
        home()?.join("Library/Caches")
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .map(Ok)
            .unwrap_or_else(|| home().map(|p| p.join(".cache")))?
    };
    Ok(root.join("sail-lsp").join("flycheck-v1"))
}

impl PreparedCache {
    pub fn prepare(config: &FlycheckConfig, args: &[String], cwd: &Path) -> Self {
        let mut result = Self { args: args.to_vec(), info: CacheInfo::default(), work: None };
        // Explicit compiler options retain their previous meaning. In
        // particular, managed caching never defeats --no-memo-z3.
        let last_switch = args.iter().rev().find(|a| {
            matches!(a.as_str(), "--no-memo-z3" | "-no_memo_z3" | "--memo-z3" | "-memo_z3")
        });
        let disabled =
            last_switch.is_some_and(|a| matches!(a.as_str(), "--no-memo-z3" | "-no_memo_z3"));
        let explicit_path = args.iter().enumerate().rev().find_map(|(index, arg)| {
            arg.strip_prefix("--memo-z3-path=").map(|path| cwd.join(path)).or_else(|| {
                matches!(arg.as_str(), "--memo-z3-path" | "-memo_z3_path")
                    .then(|| args.get(index + 1).map(|path| cwd.join(path)))
                    .flatten()
            })
        });
        let explicit = args.iter().any(|a| {
            matches!(a.as_str(), "--memo-z3" | "-memo_z3" | "--memo-z3-path" | "-memo_z3_path")
                || a.starts_with("--memo-z3-path=")
        });
        if disabled || config.cache == CompilerCacheMode::Disabled && !explicit {
            result.info.mode = CompilerCacheMode::Disabled;
            if !disabled {
                result.args.push("--no-memo-z3".into());
            }
            return result;
        }
        if explicit || config.cache == CompilerCacheMode::Compiler {
            result.info.mode = CompilerCacheMode::Compiler;
            let path = explicit_path.unwrap_or_else(|| cwd.join("sail_smt_cache"));
            result.info.bytes_at_start = size(&path);
            result.info.existing_at_start = result.info.bytes_at_start != 0;
            result.info.file = Some(path);
            return result;
        }
        let prepare = || -> Result<(PathBuf, tempfile::TempDir), String> {
            let directory = match &config.cache_directory {
                Some(path) => cwd.join(path),
                None => default_directory()?,
            };
            let directory = if directory.is_absolute() { directory } else { cwd.join(directory) };
            std::fs::create_dir_all(&directory)
                .map_err(|e| format!("{}: {e}", directory.display()))?;
            let mut identity = std::collections::hash_map::DefaultHasher::new();
            "sail-lsp-compiler-memo-v1".hash(&mut identity);
            super::worker::CompilerKey::new(&config.command, cwd).hash(&mut identity);
            super::worker::CompilerKey::new("z3", cwd).hash(&mut identity);
            std::env::var_os("SAIL_DIR").hash(&mut identity);
            args.hash(&mut identity);
            let path = directory.join(format!("{:016x}.cache", identity.finish()));
            let work = tempfile::Builder::new()
                .prefix(".check-")
                .tempdir_in(&directory)
                .map_err(|e| format!("{}: {e}", directory.display()))?;
            if size(&path) != 0 {
                std::fs::copy(&path, work.path().join("memo"))
                    .map_err(|e| format!("{}: {e}", path.display()))?;
            }
            if work.path().join("memo").to_str().is_none() {
                return Err("Compiler memo path is not valid UTF-8".into());
            }
            Ok((path, work))
        };
        match prepare() {
            Ok((path, work)) => {
                result.info.bytes_at_start = size(&path);
                result.info.existing_at_start = result.info.bytes_at_start != 0;
                result.info.file = Some(path);
                result.args.extend([
                    "--memo-z3".into(),
                    "--memo-z3-path".into(),
                    work.path().join("memo").to_string_lossy().into_owned(),
                ]);
                result.work = Some(work);
            }
            Err(error) => {
                // An optional cache must not prevent an authoritative check.
                result.args.push("--no-memo-z3".into());
                result.info.warning = Some(format!(
                    "Managed compiler cache unavailable; checking without memoization: {error}"
                ));
            }
        }
        result
    }

    pub fn finish(&mut self, completed: bool) {
        if let (Some(work), Some(destination)) = (&self.work, &self.info.file) {
            let memo = work.path().join("memo");
            if completed && memo.is_file() {
                if let Err(error) = std::fs::rename(&memo, destination) {
                    self.info.warning = Some(format!(
                        "Could not publish compiler cache {}: {error}",
                        destination.display()
                    ));
                }
            }
        }
        self.info.bytes_after_check = self.info.file.as_ref().map_or(0, |path| size(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(root: &Path) -> FlycheckConfig {
        FlycheckConfig {
            command: root.join("sail").to_string_lossy().into_owned(),
            cache_directory: Some("cache".into()),
            ..Default::default()
        }
    }

    #[test]
    fn completed_caches_persist_and_cancelled_copies_do_not_overwrite_them() {
        let _environment =
            crate::server_check::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        let args = vec!["--project".into(), "model.sail_project".into(), "--just-check".into()];
        let mut first = PreparedCache::prepare(&config, &args, root.path());
        assert!(!first.info.existing_at_start);
        let destination = first.info.file.clone().unwrap();
        let memo = first.work.as_ref().unwrap().path().join("memo");
        assert!(!memo.exists(), "Sail must see an absent cold cache, not an empty marshal stream");
        std::fs::write(&memo, b"first complete cache").unwrap();
        assert!(!destination.exists(), "working output is private until completion");
        first.finish(true);
        drop(first);
        let mut second = PreparedCache::prepare(&config, &args, root.path());
        assert!(second.info.existing_at_start);
        let private = second.work.as_ref().unwrap().path().to_owned();
        assert_eq!(std::fs::read(private.join("memo")).unwrap(), b"first complete cache");
        std::fs::write(private.join("memo"), b"partial output").unwrap();
        second.finish(false);
        drop(second);
        assert_eq!(std::fs::read(&destination).unwrap(), b"first complete cache");
        assert!(!private.exists(), "cancelled private files are reclaimed");
        let mut third = PreparedCache::prepare(&config, &args, root.path());
        std::fs::write(third.work.as_ref().unwrap().path().join("memo"), b"new complete cache")
            .unwrap();
        third.finish(true);
        assert_eq!(std::fs::read(destination).unwrap(), b"new complete cache");
    }

    #[test]
    fn explicit_compiler_cache_flags_are_preserved() {
        let root = tempfile::tempdir().unwrap();
        for args in [vec!["--no-memo-z3"], vec!["--memo-z3-path", "custom"], vec!["--memo-z3"]] {
            let args: Vec<String> = args.into_iter().map(String::from).collect();
            let cache = PreparedCache::prepare(&config(root.path()), &args, root.path());
            assert_eq!(cache.args, args);
            assert!(cache.work.is_none());
        }
        assert!(!root.path().join("cache").exists());
        let enabled = PreparedCache::prepare(
            &config(root.path()),
            &["--no-memo-z3".into(), "--memo-z3".into()],
            root.path(),
        );
        assert_eq!(enabled.info.mode, CompilerCacheMode::Compiler);
        let disabled = PreparedCache::prepare(
            &config(root.path()),
            &["--memo-z3".into(), "--no-memo-z3".into()],
            root.path(),
        );
        assert_eq!(disabled.info.mode, CompilerCacheMode::Disabled);
    }

    #[test]
    fn compiler_updates_and_project_arguments_select_distinct_cache_files() {
        let _environment =
            crate::server_check::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
        let root = tempfile::tempdir().unwrap();
        let config = config(root.path());
        std::fs::write(&config.command, b"compiler version one").unwrap();
        let first = PreparedCache::prepare(&config, &[], root.path()).info.file.unwrap();
        std::fs::write(&config.command, b"compiler version two changed size").unwrap();
        let second = PreparedCache::prepare(&config, &[], root.path()).info.file.unwrap();
        let third = PreparedCache::prepare(
            &config,
            &["--variable".into(), "RMEM=true".into()],
            root.path(),
        )
        .info
        .file
        .unwrap();
        assert_ne!(first, second);
        assert_ne!(second, third);
    }

    #[test]
    fn unusable_cache_directory_falls_back_to_a_real_uncached_check() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("cache"), b"not a directory").unwrap();
        let args = vec!["--just-check".into()];
        let result = PreparedCache::prepare(&config(root.path()), &args, root.path());
        assert_eq!(result.args, ["--just-check", "--no-memo-z3"]);
        assert!(result.info.warning.is_some());
        assert!(result.work.is_none());
    }
}
