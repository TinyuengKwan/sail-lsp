//! User-scoped installation. Bootstrap scripts only fetch/verify the first
//! executable; archive validation, lifecycle and editor ownership live here.
mod distribution;
mod editors;
mod lifecycle;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

static CANCELLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub(crate) fn configure_cli_cancellation() -> Result<()> {
    ctrlc::set_handler(|| CANCELLED.store(true, std::sync::atomic::Ordering::Relaxed))
        .context("Cannot configure installer cancellation")
}
pub(crate) fn cancelled() -> bool {
    CANCELLED.load(std::sync::atomic::Ordering::Relaxed)
}

pub(crate) const RELEASE_BASE: &str = "https://github.com/TinyuengKwan/sail-lsp/releases";
#[derive(Debug, Clone)]
struct Options {
    version: String,
    prefix: PathBuf,
    config_home: PathBuf,
    editor_root: Option<PathBuf>,
    archive: Option<PathBuf>,
    sha256: Option<String>,
    from_source: bool,
    source: Option<PathBuf>,
    revision: Option<String>,
    editors: Vec<String>,
    no_modify_path: bool,
    uninstall: bool,
    rollback: bool,
    list: bool,
    release_base: String,
    vsix: Option<PathBuf>,
    code: PathBuf,
    profile: Option<PathBuf>,
}
impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut o = Self {
            version: "latest".into(),
            prefix: default_prefix()?,
            config_home: default_config_home()?,
            editor_root: None,
            archive: None,
            sha256: None,
            from_source: false,
            source: None,
            revision: None,
            editors: Vec::new(),
            no_modify_path: false,
            uninstall: false,
            rollback: false,
            list: false,
            release_base: std::env::var("SAIL_LSP_RELEASE_BASE_URL")
                .unwrap_or_else(|_| RELEASE_BASE.into()),
            vsix: None,
            code: PathBuf::from("code"),
            profile: None,
        };
        let mut i = 0;
        while i < args.len() {
            let key = args[i].as_str();
            let takes_value = matches!(
                key,
                "--version"
                    | "--prefix"
                    | "--config-home"
                    | "--editor-root"
                    | "--archive"
                    | "--sha256"
                    | "--path"
                    | "--rev"
                    | "--editor"
                    | "--release-base-url"
                    | "--vsix"
                    | "--code-command"
                    | "--profile"
            );
            let value = if takes_value {
                i += 1;
                Some(args.get(i).with_context(|| format!("{key} requires a value"))?.as_str())
            } else {
                None
            };
            match key {
                "--version" => o.version = value.unwrap().into(),
                "--prefix" => o.prefix = value.unwrap().into(),
                "--config-home" => o.config_home = value.unwrap().into(),
                "--editor-root" => o.editor_root = Some(value.unwrap().into()),
                "--archive" => o.archive = Some(value.unwrap().into()),
                "--sha256" => o.sha256 = Some(value.unwrap().into()),
                "--from-source" => o.from_source = true,
                "--path" => {
                    o.source = Some(value.unwrap().into());
                    o.from_source = true;
                }
                "--rev" => {
                    o.revision = Some(value.unwrap().into());
                    o.from_source = true;
                }
                "--editor" => o.editors.extend(value.unwrap().split(',').map(str::to_owned)),
                "--no-modify-path" => o.no_modify_path = true,
                "--uninstall" => o.uninstall = true,
                "--rollback" => o.rollback = true,
                "--list" => o.list = true,
                "--release-base-url" => {
                    o.release_base = value.unwrap().trim_end_matches('/').into()
                }
                "--vsix" => o.vsix = Some(value.unwrap().into()),
                "--code-command" => o.code = value.unwrap().into(),
                "--profile" => o.profile = Some(value.unwrap().into()),
                "--help" | "-h" => bail!("help"),
                _ => bail!("Unknown installer option: {key}"),
            }
            i += 1;
        }
        anyhow::ensure!(!o.from_source || o.archive.is_none(), "Choose --from-source or --archive");
        anyhow::ensure!(
            [o.uninstall, o.rollback, o.list].into_iter().filter(|v| *v).count() <= 1,
            "Choose one lifecycle operation"
        );
        anyhow::ensure!(
            o.editors
                .iter()
                .all(|e| ["vscode", "zed", "neovim", "helix", "emacs"].contains(&e.as_str())),
            "Supported editors: vscode, zed, neovim, helix, emacs"
        );
        let mut seen = std::collections::BTreeSet::new();
        o.editors.retain(|editor| seen.insert(editor.clone()));
        for path in [&mut o.prefix, &mut o.config_home]
            .into_iter()
            .chain(o.editor_root.iter_mut())
            .chain(o.profile.iter_mut())
        {
            if path.is_relative() {
                *path = std::env::current_dir()?.join(&*path);
            }
        }
        anyhow::ensure!(
            o.prefix.parent().is_some() && o.prefix.file_name().is_some(),
            "Installation prefix cannot be a filesystem root"
        );
        Ok(o)
    }
}

#[allow(clippy::print_stdout)]
pub(crate) fn run(args: &[String]) -> std::result::Result<(), String> {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("sail-lsp install [--version TAG] [--prefix DIR] [--editor vscode|zed|neovim|helix|emacs]\n  [--from-source --rev REF | --path CHECKOUT | --archive FILE --sha256 HASH]\n  [--no-modify-path] [--profile FILE] [--config-home DIR] [--editor-root DIR]\n  [--vsix FILE] [--code-command PATH] [--release-base-url URL]\n  [--list | --rollback [--version VERSION] | --uninstall]");
        return Ok(());
    }
    configure_cli_cancellation()
        .and_then(|()| Options::parse(args))
        .and_then(lifecycle::run)
        .map_err(|e| format!("{e:#}"))
}

pub(crate) fn home() -> Result<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .context("Cannot find the user home directory; provide --prefix and configuration paths")
}
pub(crate) fn default_prefix() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SAIL_LSP_HOME") {
        return Ok(path.into());
    }
    if cfg!(windows) {
        Ok(std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or(home()?.join("AppData/Local"))
            .join("sail-lsp"))
    } else {
        Ok(std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".local/share"))
            .join("sail-lsp"))
    }
}
pub(crate) fn default_config_home() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SAIL_LSP_CONFIG_HOME") {
        return Ok(path.into());
    }
    if cfg!(windows) {
        Ok(std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home()?.join("AppData/Roaming"))
            .join("sail-lsp"))
    } else {
        Ok(std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".config"))
            .join("sail-lsp"))
    }
}
pub(crate) fn executable_name() -> &'static str {
    if cfg!(windows) {
        "sail-lsp.exe"
    } else {
        "sail-lsp"
    }
}
fn sha256(path: &Path) -> Result<String> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut b = [0; 65536];
    loop {
        let n = f.read(&mut b)?;
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let parent = path.parent().context("Path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut tmp = tempfile::NamedTempFile::new_in(parent)?;
    tmp.write_all(bytes)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)
        .map_err(|e| e.error)
        .with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}
fn output(mut command: Command, timeout: Duration) -> Result<Output> {
    anyhow::ensure!(!cancelled(), "Command cancelled");
    let stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    command.stdin(Stdio::null()).stdout(stdout.try_clone()?).stderr(stderr.try_clone()?);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child =
        command.spawn().with_context(|| format!("Cannot start {:?}", command.get_program()))?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if cancelled() || start.elapsed() > timeout {
            #[cfg(unix)]
            // SAFETY: process_group(0) creates a group owned by this child;
            // terminate that group so compiler descendants do not survive.
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill.exe")
                    .args(["/F", "/T", "/PID", &child.id().to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = child.kill();
            let _ = child.wait();
            if cancelled() {
                bail!("Command cancelled: {:?}", command.get_program());
            }
            bail!("Command timed out: {:?}", command.get_program());
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    use std::io::{Read, Seek};
    let mut stdout = stdout;
    let mut stderr = stderr;
    stdout.rewind()?;
    stderr.rewind()?;
    let mut a = Vec::new();
    let mut b = Vec::new();
    stdout.take(4 * 1024 * 1024).read_to_end(&mut a)?;
    stderr.take(4 * 1024 * 1024).read_to_end(&mut b)?;
    Ok(Output { status, stdout: a, stderr: b })
}
fn checked(command: Command, timeout: Duration) -> Result<Output> {
    let result = output(command, timeout)?;
    anyhow::ensure!(
        result.status.success(),
        "Command failed ({}): {}",
        result.status,
        String::from_utf8_lossy(&result.stderr)
    );
    Ok(result)
}
fn download(url: &str, path: &Path) -> Result<()> {
    if let Some(local) = url.strip_prefix("file://") {
        std::fs::copy(Path::new(local), path)?;
        return Ok(());
    }
    anyhow::ensure!(
        url.starts_with("https://"),
        "Download URLs must use HTTPS (or explicit file:// offline mirrors)"
    );
    let mut c = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    c.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--tlsv1.2",
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "15",
        "--max-time",
        "300",
        "--output",
    ])
    .arg(path)
    .arg(url);
    checked(c, Duration::from_secs(310))?;
    Ok(())
}
fn release_url(base: &str, version: &str, asset: &str) -> Result<String> {
    anyhow::ensure!(!asset.contains(['/', '\\']), "Invalid asset name");
    if version == "latest" {
        Ok(format!("{base}/latest/download/{asset}"))
    } else {
        anyhow::ensure!(
            !version.starts_with('-')
                && version.chars().all(|c| c.is_ascii_alphanumeric() || ".-_".contains(c)),
            "Invalid release version"
        );
        Ok(format!("{base}/download/{version}/{asset}"))
    }
}

pub(crate) fn probe_command(command: Command) -> Result<Vec<u8>> {
    Ok(checked(command, Duration::from_secs(5))?.stdout)
}
