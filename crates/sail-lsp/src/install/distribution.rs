use super::*;
use std::io::{Read, Write};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Manifest {
    pub schema_version: u32,
    pub version: String,
    pub tag: String,
    pub revision: String,
    pub target: String,
    pub stdlib_revision: String,
    pub solver: String,
    pub binary: String,
    pub binary_sha256: String,
    #[serde(default)]
    pub crt_static: bool,
    pub requirements: serde_json::Value,
}
pub(super) struct Payload {
    pub directory: tempfile::TempDir,
    pub manifest: Manifest,
}
impl Manifest {
    fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.schema_version == 1,
            "Unsupported distribution manifest schema {}",
            self.schema_version
        );
        anyhow::ensure!(
            self.target == env!("SAIL_LSP_BUILD_TARGET"),
            "Archive targets {}, installer targets {}",
            self.target,
            env!("SAIL_LSP_BUILD_TARGET")
        );
        anyhow::ensure!(self.binary == executable_name(), "Unexpected executable name");
        anyhow::ensure!(
            self.solver == "z3-vendored",
            "Official distributions must contain vendored Z3"
        );
        anyhow::ensure!(
            self.binary_sha256.len() == 64
                && self.binary_sha256.chars().all(|c| c.is_ascii_hexdigit()),
            "Invalid binary checksum"
        );
        anyhow::ensure!(
            !self.version.is_empty()
                && !self.revision.is_empty()
                && !self.stdlib_revision.is_empty(),
            "Incomplete distribution provenance"
        );
        Ok(())
    }
}
fn verify(path: &Path, hash: &str) -> Result<()> {
    let expected = hash.split_whitespace().next().context("Empty checksum")?;
    anyhow::ensure!(
        expected.len() == 64 && expected.chars().all(|c| c.is_ascii_hexdigit()),
        "Invalid SHA256 checksum"
    );
    anyhow::ensure!(
        sha256(path)?.eq_ignore_ascii_case(expected),
        "SHA256 mismatch for {}",
        path.display()
    );
    Ok(())
}
fn safe_entry(path: &Path) -> Result<()> {
    anyhow::ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, std::path::Component::Normal(_))),
        "Unsafe archive path: {}",
        path.display()
    );
    anyhow::ensure!(
        !path.to_string_lossy().contains('\\'),
        "Backslashes are not permitted in distribution paths"
    );
    Ok(())
}
fn copy_entry(mut input: impl Read, path: &Path, total: &mut u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(path)?;
    let count = std::io::copy(&mut input.by_ref().take(512 * 1024 * 1024 + 1), &mut file)?;
    *total += count;
    anyhow::ensure!(*total <= 512 * 1024 * 1024, "Distribution expands beyond 512 MiB");
    file.flush()?;
    Ok(())
}
pub(super) fn unpack(path: &Path, expected: &str) -> Result<Payload> {
    verify(path, expected)?;
    let directory = tempfile::tempdir()?;
    let mut total = 0;
    if path.extension().is_some_and(|e| e == "zip") {
        let mut zip = zip::ZipArchive::new(std::fs::File::open(path)?)?;
        for i in 0..zip.len() {
            let mut entry = zip.by_index(i)?;
            let entry_path = PathBuf::from(entry.name());
            safe_entry(&entry_path)?;
            anyhow::ensure!(
                entry.unix_mode().is_none_or(|mode| mode & 0o170000 != 0o120000),
                "Symlinks are not permitted in distributions"
            );
            if entry.is_dir() {
                std::fs::create_dir_all(directory.path().join(entry_path))?;
            } else {
                copy_entry(&mut entry, &directory.path().join(entry_path), &mut total)?;
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(std::fs::File::open(path)?));
        for entry in tar.entries()? {
            let mut entry = entry?;
            let entry_path = entry.path()?.into_owned();
            safe_entry(&entry_path)?;
            if entry.header().entry_type().is_dir() {
                std::fs::create_dir_all(directory.path().join(entry_path))?;
            } else {
                anyhow::ensure!(
                    entry.header().entry_type().is_file(),
                    "Links and special archive entries are not permitted"
                );
                copy_entry(&mut entry, &directory.path().join(entry_path), &mut total)?;
            }
        }
    }
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(directory.path().join("manifest.json"))?)?;
    manifest.validate()?;
    let binary = directory.path().join(&manifest.binary);
    verify(&binary, &manifest.binary_sha256)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755))?;
    }
    let mut cmd = Command::new(&binary);
    cmd.args(["--version", "--json"]);
    let actual: serde_json::Value =
        serde_json::from_slice(&checked(cmd, Duration::from_secs(15))?.stdout)?;
    for (key, expected) in [
        ("version", &manifest.version),
        ("revision", &manifest.revision),
        ("target", &manifest.target),
        ("stdlibRevision", &manifest.stdlib_revision),
        ("solver", &manifest.solver),
    ] {
        anyhow::ensure!(
            actual[key].as_str() == Some(expected),
            "Executable and manifest disagree on {key}"
        );
    }
    Ok(Payload { directory, manifest })
}
pub(super) fn acquire(options: &Options) -> Result<Payload> {
    if options.from_source {
        return source(options);
    }
    if let Some(path) = &options.archive {
        let hash = if let Some(hash) = &options.sha256 {
            hash.clone()
        } else {
            std::fs::read_to_string(format!("{}.sha256", path.display()))
                .context("Offline archives require a .sha256 sidecar or --sha256")?
        };
        let payload = unpack(path, &hash)?;
        verify_version(options, &payload.manifest)?;
        return Ok(payload);
    }
    let temp = tempfile::tempdir()?;
    let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
    let asset = format!("sail-lsp-{}.{}", env!("SAIL_LSP_BUILD_TARGET"), ext);
    let archive = temp.path().join(&asset);
    let sum = temp.path().join("checksum");
    download(&release_url(&options.release_base, &options.version, &asset)?, &archive)?;
    download(
        &release_url(&options.release_base, &options.version, &format!("{asset}.sha256"))?,
        &sum,
    )?;
    let payload = unpack(&archive, &std::fs::read_to_string(sum)?)?;
    verify_version(options, &payload.manifest)?;
    Ok(payload)
}
fn verify_version(options: &Options, m: &Manifest) -> Result<()> {
    if options.version != "latest" {
        anyhow::ensure!(
            options.version == m.tag || options.version.trim_start_matches('v') == m.version,
            "Requested version {} but archive contains {} ({})",
            options.version,
            m.tag,
            m.version
        );
    }
    Ok(())
}
// User-facing installer progress; this path never runs in the LSP server.
#[allow(clippy::print_stderr)]
fn source(options: &Options) -> Result<Payload> {
    eprintln!("Preparing a locked source build (vendored Z3; the first build can take several minutes)...");
    let checkout = tempfile::tempdir()?;
    let root = if let Some(root) = &options.source {
        root.canonicalize()?
    } else {
        let root = checkout.path().join("source");
        std::fs::create_dir(&root)?;
        for args in [
            vec!["init", "--quiet"],
            vec!["remote", "add", "origin", "https://github.com/TinyuengKwan/sail-lsp.git"],
            vec![
                "fetch",
                "--quiet",
                "--depth",
                "1",
                "origin",
                options.revision.as_deref().unwrap_or(if options.version == "latest" {
                    "main"
                } else {
                    &options.version
                }),
            ],
            vec!["checkout", "--quiet", "--detach", "FETCH_HEAD"],
        ] {
            let mut c = Command::new("git");
            c.args(args).current_dir(&root);
            checked(c, Duration::from_secs(300))?;
        }
        root
    };
    anyhow::ensure!(
        root.join("crates/sail-lsp/Cargo.toml").is_file(),
        "Not a sail-lsp checkout: {}",
        root.display()
    );
    if let Some(revision) = &options.revision {
        anyhow::ensure!(!revision.starts_with('-'), "Invalid revision");
        let mut c = Command::new("git");
        c.args(["rev-parse", "--verify", &format!("{revision}^{{commit}}")]).current_dir(&root);
        let expected = checked(c, Duration::from_secs(10))?.stdout;
        let mut c = Command::new("git");
        c.args(["rev-parse", "HEAD"]).current_dir(&root);
        let current = checked(c, Duration::from_secs(10))?.stdout;
        anyhow::ensure!(
            expected == current,
            "Local checkout does not match --rev; check out the requested revision first"
        );
        let mut c = Command::new("git");
        c.args(["status", "--porcelain", "--untracked-files=normal"]).current_dir(&root);
        anyhow::ensure!(checked(c,Duration::from_secs(10))?.stdout.is_empty(),"--rev requires a clean pinned checkout; omit --rev to install local working-tree changes");
    }
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .map(|p| if p.is_absolute() { p } else { root.join(p) })
        .unwrap_or(root.join("target"));
    let build_target = std::env::var_os("CARGO_BUILD_TARGET").or_else(|| {
        if cfg!(windows) {
            Some(std::ffi::OsString::from(env!("SAIL_LSP_BUILD_TARGET")))
        } else {
            None
        }
    });
    let c = source_build_command(&root, &target, build_target.as_deref());
    #[cfg(windows)]
    let mut c = c;
    #[cfg(windows)]
    {
        let flags = std::env::var("RUSTFLAGS").unwrap_or_default();
        if !flags.contains("target-feature=+crt-static") {
            c.env("RUSTFLAGS", format!("{flags} -C target-feature=+crt-static"));
        }
        for key in ["CFLAGS", "CXXFLAGS"] {
            c.env(key, format!("{} /utf-8", std::env::var(key).unwrap_or_default()));
        }
    }
    checked(c, Duration::from_secs(3600))
        .context("Source build failed; Rust, CMake and a C++20 compiler with std::format support (GCC 13+ or current MSVC/Xcode) are required")?;
    let binary = build_target
        .map(|triple| target.join(triple))
        .unwrap_or(target)
        .join("release")
        .join(executable_name());
    let mut c = Command::new(&binary);
    c.args(["--version", "--json"]);
    let meta: serde_json::Value =
        serde_json::from_slice(&checked(c, Duration::from_secs(15))?.stdout)?;
    anyhow::ensure!(meta["solver"] == "z3-vendored", "Source install must statically link Z3");
    let directory = tempfile::tempdir()?;
    std::fs::copy(&binary, directory.path().join(executable_name()))?;
    let manifest = Manifest {
        schema_version: 1,
        version: meta["version"].as_str().context("Missing build version")?.into(),
        tag: format!("v{}", meta["version"].as_str().unwrap()),
        revision: meta["revision"].as_str().context("Missing build revision")?.into(),
        target: env!("SAIL_LSP_BUILD_TARGET").into(),
        stdlib_revision: meta["stdlibRevision"]
            .as_str()
            .context("Missing stdlib provenance")?
            .into(),
        solver: "z3-vendored".into(),
        binary: executable_name().into(),
        binary_sha256: sha256(&binary)?,
        crt_static: meta["crtStatic"].as_bool().unwrap_or(false),
        requirements: serde_json::json!({"sourceBuild":true}),
    };
    verify_version(options, &manifest)?;
    std::fs::write(directory.path().join("manifest.json"), serde_json::to_vec_pretty(&manifest)?)?;
    editors::copy_source_assets(&root, directory.path())?;
    if options.editors.iter().any(|e| e == "vscode") && options.vsix.is_none() {
        editors::build_source_vsix(&root, directory.path(), &binary, &manifest)?;
    }
    Ok(Payload { directory, manifest })
}

fn source_build_command(
    root: &Path,
    target_dir: &Path,
    build_target: Option<&std::ffi::OsStr>,
) -> Command {
    let mut command = Command::new("cargo");
    command.arg("build");
    if let Some(triple) = build_target {
        command.arg("--target").arg(triple);
    }
    command
        .args(["--release", "--locked", "--features", "vendored", "--bin", "sail-lsp"])
        .arg("--target-dir")
        .arg(target_dir)
        .current_dir(root);
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_build_command_respects_cargo_subcommand_options() {
        let target_dir = tempfile::tempdir().unwrap();
        let triple = std::ffi::OsStr::new(env!("SAIL_LSP_BUILD_TARGET"));
        for build_target in [None, Some(triple)] {
            let output = source_build_command(
                Path::new(env!("CARGO_MANIFEST_DIR")),
                target_dir.path(),
                build_target,
            )
            .arg("--help")
            .output()
            .expect("Cargo must be available to run installer tests");
            assert!(
                output.status.success(),
                "Cargo rejected source-build arguments: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}
