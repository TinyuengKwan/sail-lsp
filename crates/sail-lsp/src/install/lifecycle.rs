use super::*;
use distribution::{Manifest, Payload};
use std::{collections::BTreeMap, fs};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Installed {
    manifest: Manifest,
    files: BTreeMap<String, String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct State {
    schema_version: u32,
    active: String,
    previous: Option<String>,
    versions: BTreeMap<String, Installed>,
    pub changes: Vec<editors::Change>,
    pub user_path_added: bool,
}
impl State {
    fn load(prefix: &Path) -> Result<Option<Self>> {
        let path = prefix.join("install-state.json");
        if !path.is_file() {
            return Ok(None);
        }
        let state: Self = serde_json::from_slice(&fs::read(path)?)?;
        anyhow::ensure!(state.schema_version == 1, "Unsupported installation state schema");
        for (id, version) in &state.versions {
            anyhow::ensure!(safe_id(id), "Invalid installed version ID");
            for file in version.files.keys() {
                let relative = Path::new(file);
                anyhow::ensure!(
                    relative.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                    "Invalid owned file path"
                );
                check_owned_path(prefix, &Path::new("versions").join(id).join(relative))?;
            }
        }
        check_owned_path(prefix, Path::new("bin"))?;
        anyhow::ensure!(
            state.versions.contains_key(&state.active)
                && state.previous.as_ref().is_none_or(|id| state.versions.contains_key(id)),
            "Invalid installation state version references"
        );
        Ok(Some(state))
    }
    fn save(&self, prefix: &Path) -> Result<()> {
        atomic_write(&prefix.join("install-state.json"), &serde_json::to_vec_pretty(self)?)
    }
}
fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && id != "."
        && id != ".."
        && id.len() < 160
        && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
}
// An edited installation may replace a directory with a link. Never follow it
// while validating, activating or deleting files recorded in our inventory.
fn check_owned_path(prefix: &Path, relative: &Path) -> Result<()> {
    let mut path = prefix.to_path_buf();
    for part in relative.components() {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) => anyhow::ensure!(
                !meta.file_type().is_symlink(),
                "Managed path was replaced by a symlink: {}; restore it before continuing",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}
fn binary(prefix: &Path, id: &str) -> PathBuf {
    prefix.join("versions").join(id).join(executable_name())
}
fn verify_active(prefix: &Path, state: Option<&State>) -> Result<()> {
    let entry = prefix.join("bin").join(executable_name());
    if fs::symlink_metadata(&entry).is_err() {
        return Ok(());
    }
    let state=state.context("An unmanaged executable already exists at the installation prefix; choose another --prefix")?;
    #[cfg(unix)]
    anyhow::ensure!(
        fs::read_link(&entry)? == binary(prefix, &state.active),
        "The managed launcher was replaced; refusing to overwrite it"
    );
    #[cfg(windows)]
    anyhow::ensure!(
        sha256(&entry)? == state.versions[&state.active].manifest.binary_sha256,
        "The managed executable was replaced; refusing to overwrite it"
    );
    Ok(())
}
fn activate(prefix: &Path, id: &str, manifest: &Manifest) -> Result<()> {
    let source = binary(prefix, id);
    anyhow::ensure!(
        sha256(&source)? == manifest.binary_sha256,
        "Installed binary checksum changed; reinstall this version"
    );
    fs::create_dir_all(prefix.join("bin"))?;
    let temporary = prefix.join("bin").join(format!(".sail-lsp-next-{}", std::process::id()));
    let _ = fs::remove_file(&temporary);
    #[cfg(unix)]
    std::os::unix::fs::symlink(&source, &temporary)?;
    #[cfg(windows)]
    {
        fs::copy(&source, &temporary)?;
    }
    if let Err(error) = fs::rename(&temporary, prefix.join("bin").join(executable_name())) {
        let _ = fs::remove_file(&temporary);
        return Err(error).context("Cannot activate the new server; stop running Sail language servers and retry. The previous version is retained");
    }
    Ok(())
}
fn inventory(dir: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let ty = entry.file_type()?;
            anyhow::ensure!(!ty.is_symlink(), "Distribution contains a symlink");
            if ty.is_dir() {
                visit(root, &path, out)?;
            } else {
                out.insert(
                    path.strip_prefix(root)?
                        .to_str()
                        .context("Non-UTF-8 distribution path")?
                        .replace('\\', "/"),
                    sha256(&path)?,
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(dir, dir, &mut out)?;
    Ok(out)
}
fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&path, &dest)?;
        } else {
            fs::copy(path, dest)?;
        }
    }
    Ok(())
}
fn store(prefix: &Path, payload: &Payload, id: &str) -> Result<Installed> {
    let files = inventory(payload.directory.path())?;
    let versions = prefix.join("versions");
    fs::create_dir_all(&versions)?;
    let final_dir = versions.join(id);
    if final_dir.exists() {
        anyhow::ensure!(
            inventory(&final_dir)? == files,
            "Installed version contents were changed; refusing to overwrite them"
        );
    } else {
        let staging = tempfile::tempdir_in(&versions)?;
        copy_tree(payload.directory.path(), staging.path())?;
        fs::rename(staging.path(), &final_dir)?;
    }
    Ok(Installed { manifest: payload.manifest.clone(), files })
}
#[allow(clippy::print_stdout)]
pub(super) fn run(options: Options) -> Result<()> {
    let prefix = &options.prefix;
    if options.uninstall || options.rollback || options.list {
        anyhow::ensure!(
            prefix.join("install-state.json").is_file(),
            "No managed Sail installation at {}",
            prefix.display()
        );
    }
    fs::create_dir_all(prefix)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(prefix.join(".install.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("Another Sail installation is in progress")?;
    let old = State::load(prefix)?;
    verify_active(prefix, old.as_ref())?;
    anyhow::ensure!(!cancelled(), "Installation cancelled");
    if options.list {
        let state = old.unwrap();
        for (id, version) in state.versions {
            println!(
                "{} {} {} {}",
                if id == state.active { "*" } else { " " },
                version.manifest.version,
                version.manifest.revision,
                id
            );
        }
        return Ok(());
    }
    if options.uninstall {
        return uninstall(prefix, &old.unwrap());
    }
    if options.rollback {
        let mut state = old.unwrap();
        let id = if options.version == "latest" {
            state.previous.clone().context("There is no previous installed version")?
        } else {
            let ids: Vec<_> = state
                .versions
                .iter()
                .filter(|(id, v)| {
                    **id == options.version
                        || v.manifest.tag == options.version
                        || v.manifest.version == options.version.trim_start_matches('v')
                })
                .map(|(id, _)| id.clone())
                .collect();
            anyhow::ensure!(
                ids.len() == 1,
                "Rollback version is absent or ambiguous; use --list and select its full ID"
            );
            ids[0].clone()
        };
        let previous = state.active.clone();
        anyhow::ensure!(!cancelled(), "Installation cancelled");
        activate(prefix, &id, &state.versions[&id].manifest)?;
        state.previous = Some(previous.clone());
        state.active = id;
        if let Err(error) = state.save(prefix) {
            let _ = activate(prefix, &previous, &state.versions[&previous].manifest);
            return Err(error);
        }
        println!("Active: {} ({})", state.versions[&state.active].manifest.version, state.active);
        return Ok(());
    }
    let payload = distribution::acquire(&options)?;
    let mut changes = editors::plan(
        &options,
        payload.directory.path(),
        &payload.manifest,
        old.as_ref().map(|s| s.changes.as_slice()).unwrap_or_default(),
    )?;
    // A source-built VSIX is only an input to the external editor installer.
    // Do not retain its timestamp-dependent ZIP bytes as a separate version.
    let vsix = payload.directory.path().join(format!("sail-lsp-{}.vsix", payload.manifest.target));
    if vsix.is_file() {
        fs::remove_file(vsix)?;
    }
    use sha2::{Digest, Sha256};
    let content_hash =
        format!("{:x}", Sha256::digest(serde_json::to_vec(&inventory(payload.directory.path())?)?));
    let id = format!("{}-{}", payload.manifest.version, &content_hash[..24]);
    anyhow::ensure!(safe_id(&id), "Version name cannot be used as an installation ID");
    check_owned_path(prefix, Path::new("versions"))?;
    check_owned_path(prefix, Path::new("bin"))?;
    let new_directory = !prefix.join("versions").join(&id).exists();
    let installed = store(prefix, &payload, &id)?;
    let cleanup = || {
        let directory = prefix.join("versions").join(&id);
        if new_directory && inventory(&directory).ok().as_ref() == Some(&installed.files) {
            let _ = fs::remove_dir_all(directory);
        }
    };
    let mut state = old.clone().unwrap_or(State {
        schema_version: 1,
        active: id.clone(),
        previous: None,
        versions: BTreeMap::new(),
        changes: Vec::new(),
        user_path_added: false,
    });
    if state.active != id {
        state.previous = Some(state.active.clone());
    }
    state.active = id.clone();
    state.versions.insert(id.clone(), installed.clone());
    let applied = match editors::apply(&mut changes) {
        Ok(applied) => applied,
        Err(error) => {
            cleanup();
            return Err(error);
        }
    };
    let activation = if cancelled() {
        Err(anyhow::anyhow!("Installation cancelled"))
    } else {
        activate(prefix, &id, &state.versions[&id].manifest)
    };
    if let Err(error) = activation {
        editors::restore(&applied);
        cleanup();
        return Err(error);
    }
    for change in applied {
        if let Some(index) = state.changes.iter().position(|c| c.identity() == change.identity()) {
            state.changes[index] = change;
        } else {
            state.changes.push(change);
        }
    }
    #[cfg(windows)]
    let mut path_added = false;
    #[cfg(windows)]
    let commit = if options.no_modify_path {
        state.save(prefix)
    } else {
        editors::windows_path(&prefix.join("bin"), true).and_then(|added| {
            path_added = added;
            state.user_path_added |= added;
            state.save(prefix)
        })
    };
    #[cfg(not(windows))]
    let commit = state.save(prefix);
    if let Err(error) = commit {
        if let Some(old) = &old {
            let _ = activate(prefix, &old.active, &old.versions[&old.active].manifest);
        } else {
            let _ = fs::remove_file(prefix.join("bin").join(executable_name()));
        }
        #[cfg(windows)]
        if path_added {
            let _ = editors::windows_path(&prefix.join("bin"), false);
        }
        editors::restore(&changes);
        cleanup();
        return Err(error);
    }
    println!(
        "Installed sail-lsp {} ({})\nExecutable: {}",
        payload.manifest.version,
        payload.manifest.revision,
        prefix.join("bin").join(executable_name()).display()
    );
    for editor in &options.editors {
        if editor == "zed" {
            println!(
                "Zed: run `zed: install dev extension` and select {}",
                prefix.join("editors/zed").display()
            );
        }
    }
    println!("Restart the language server to use this version. Run `{} doctor` to inspect the active runtime.",prefix.join("bin").join(executable_name()).display());
    Ok(())
}
#[allow(clippy::print_stdout)]
fn uninstall(prefix: &Path, state: &State) -> Result<()> {
    #[cfg(windows)]
    {
        // Fail before removing anything when an editor still holds an executable.
        for path in std::iter::once(prefix.join("bin").join(executable_name()))
            .chain(state.versions.keys().map(|id| binary(prefix, id)))
        {
            fs::OpenOptions::new().write(true).open(&path).with_context(||format!("Stop running Sail servers before uninstalling {} (use install.ps1 to avoid running the installed executable itself)",path.display()))?;
        }
    }
    #[cfg(windows)]
    if state.user_path_added {
        editors::windows_path(&prefix.join("bin"), false)?;
    }
    editors::undo(&state.changes);
    let bin = prefix.join("bin").join(executable_name());
    if fs::symlink_metadata(&bin).is_ok() {
        fs::remove_file(bin)?;
    }
    for (id, version) in &state.versions {
        let root = prefix.join("versions").join(id);
        for (file, hash) in &version.files {
            let relative = Path::new(file);
            anyhow::ensure!(
                relative.components().all(|c| matches!(c, std::path::Component::Normal(_))),
                "Invalid owned file path"
            );
            let path = root.join(relative);
            if path.is_file() && sha256(&path)? == *hash {
                fs::remove_file(&path)?;
            } else if path.exists() {
                println!("Preserved modified file {}", path.display());
            }
        }
        remove_empty_dirs(&root)?;
    }
    for dir in [prefix.join("versions"), prefix.join("bin")] {
        let _ = fs::remove_dir(dir);
    }
    fs::remove_file(prefix.join("install-state.json"))?;
    println!("Uninstalled managed Sail server; unrelated or user-modified files were retained. Installation lock: {}",prefix.join(".install.lock").display());
    Ok(())
}
fn remove_empty_dirs(dir: &Path) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_empty_dirs(&entry.path())?;
        }
    }
    let _ = fs::remove_dir(dir);
    Ok(())
}
