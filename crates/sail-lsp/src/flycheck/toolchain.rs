//! Describe the compiler that actually checked the project. Library equality is
//! based on relative Sail source paths and bytes, rather than installation paths.
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolchainReport {
    pub command: String,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub compiler_stdlib: Option<PathBuf>,
    pub native_stdlib: Option<PathBuf>,
    pub libraries_match: Option<bool>,
    pub differing_files: Vec<String>,
    pub warnings: Vec<String>,
    pub native_options: BTreeMap<String, &'static str>,
}

#[derive(Default)]
pub(crate) struct ProbeCache {
    key: Option<super::worker::CompilerKey>,
    version: Option<String>,
    directory: Option<PathBuf>,
    warnings: Vec<String>,
}

impl ProbeCache {
    pub fn report(
        &mut self,
        command: &str,
        cwd: &Path,
        native: Option<&Path>,
        args: &[String],
        mut probe: impl FnMut(&str) -> Result<String, String>,
    ) -> ToolchainReport {
        let key = super::worker::CompilerKey::new(command, cwd);
        if self.key.as_ref() != Some(&key) || self.version.is_none() || self.directory.is_none() {
            self.version = None;
            self.directory = None;
            self.warnings.clear();
            for option in ["--version", "--dir"] {
                match probe(option) {
                    Ok(value)
                        if !value.trim().is_empty()
                            && value.len() <= 4096
                            && value.trim().lines().count() == 1 =>
                    {
                        if option == "--version" {
                            self.version = Some(value.trim().to_owned());
                        } else {
                            self.directory = Some(cwd.join(value.trim()));
                        }
                    }
                    result => self.warnings.push(format!(
                        "Compiler {option} unavailable: {}",
                        result
                            .err()
                            .unwrap_or_else(|| {
                                "expected a nonempty single-line response up to 4096 bytes".into()
                            })
                            .chars()
                            .take(2048)
                            .collect::<String>()
                    )),
                }
            }
            self.key = Some(key);
        }
        let mut report = ToolchainReport {
            command: command.into(),
            executable: self.key.as_ref().and_then(|key| key.executable_path()),
            version: self.version.clone(),
            compiler_stdlib: self.directory.clone(),
            native_stdlib: native.map(Path::to_path_buf),
            warnings: self.warnings.clone(),
            native_options: BTreeMap::from([
                ("--strict-bitvector".into(), "supported"),
                ("--strict-var".into(), "source-directive-only"),
                ("--strict-exponentials".into(), "unsupported"),
            ]),
            ..Default::default()
        };
        if let (Some(compiler), Some(native)) = (&report.compiler_stdlib, native) {
            match (fingerprints(compiler), fingerprints(native)) {
                (Ok(compiler), Ok(native)) => {
                    report.libraries_match = Some(compiler == native);
                    report.differing_files = compiler
                        .keys()
                        .chain(native.keys())
                        .filter(|path| compiler.get(*path) != native.get(*path))
                        .cloned()
                        .collect::<std::collections::BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    if report.libraries_match == Some(false) {
                        report.warnings.push(format!("Compiler and native analysis use different standard-library contents ({} Sail files differ). Set SAIL_DIR consistently and restart the server.", report.differing_files.len()));
                    }
                }
                (a, b) => report.warnings.extend(a.err().into_iter().chain(b.err())),
            }
        }
        for arg in args {
            if matches!(
                arg.as_str(),
                "--strict-var" | "-strict_var" | "--strict-exponentials" | "-strict_exponentials"
            ) {
                report.warnings.push(format!("{arg} is forwarded to Sail; native analysis does not implement this command-line mode. See nativeOptions for source-directive support."));
            }
        }
        report
    }
}

fn fingerprints(root: &Path) -> Result<BTreeMap<String, String>, String> {
    let lib = root.join("lib");
    if !lib.is_dir() {
        return Err(format!("No standard library at {}", lib.display()));
    }
    let mut result = BTreeMap::new();
    let mut bytes = 0usize;
    for entry in walkdir::WalkDir::new(&lib).follow_links(true).max_depth(16) {
        let entry = entry.map_err(|e| format!("Cannot inspect {}: {e}", lib.display()))?;
        if entry.depth() == 16 && entry.file_type().is_dir() {
            return Err("Standard-library comparison exceeded its directory depth limit".into());
        }
        if !entry.file_type().is_file() || entry.path().extension().is_none_or(|e| e != "sail") {
            continue;
        }
        let size = entry.metadata().map_err(|e| e.to_string())?.len();
        if size > 64 * 1024 * 1024
            || bytes as u64 + size > 64 * 1024 * 1024
            || result.len() >= 10_000
        {
            return Err("Standard-library comparison exceeded its size limit".into());
        }
        let contents = std::fs::read(entry.path()).map_err(|e| e.to_string())?;
        bytes += contents.len();
        let path = entry.path().strip_prefix(&lib).unwrap().to_string_lossy().replace('\\', "/");
        result.insert(path, format!("{:x}", Sha256::digest(contents)));
    }
    if result.is_empty() {
        return Err(format!("No Sail sources at {}", lib.display()));
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pairing_compares_contents_and_rechecks_edits_without_reprobing_the_binary() {
        let root = tempfile::tempdir().unwrap();
        let native = root.path().join("native");
        let compiler = root.path().join("compiler");
        for dir in [&native, &compiler] {
            std::fs::create_dir_all(dir.join("lib")).unwrap();
            std::fs::write(dir.join("lib/prelude.sail"), "val x : int").unwrap();
        }
        let mut cache = ProbeCache::default();
        let mut calls = 0;
        let first = cache.report("chosen-sail", root.path(), Some(&native), &[], |arg| {
            calls += 1;
            Ok(if arg == "--dir" {
                compiler.display().to_string()
            } else {
                "Sail chosen-version".into()
            })
        });
        assert_eq!(calls, 2);
        assert_eq!(
            first.libraries_match,
            Some(true),
            "different locations can contain the same library"
        );
        std::fs::write(native.join("lib/prelude.sail"), "val x : bool").unwrap();
        let second = cache.report(
            "chosen-sail",
            root.path(),
            Some(&native),
            &["--strict-exponentials".into()],
            |_| panic!("binary probe must be cached"),
        );
        assert_eq!(second.version.as_deref(), Some("Sail chosen-version"));
        assert_eq!(second.libraries_match, Some(false));
        assert_eq!(second.differing_files, ["prelude.sail"]);
        assert!(second
            .warnings
            .iter()
            .any(|warning| warning.contains("native analysis does not implement")));
    }
}
