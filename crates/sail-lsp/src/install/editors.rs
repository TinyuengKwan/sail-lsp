use super::*;
use jsonc_parser::{
    cst::{CstInputValue, CstObject, CstRootNode},
    ParseOptions,
};
use serde_json::{json, Value};
use std::fs;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Kind {
    File,
    Block { block: String },
    Json { keys: Vec<String>, prior: Option<Value>, value: Value },
    Helix { prior: Option<String> },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Change {
    path: PathBuf,
    prior: Option<String>,
    installed: String,
    kind: Kind,
    #[serde(skip)]
    transaction_prior: Option<String>,
}
impl Change {
    pub fn identity(&self) -> String {
        format!(
            "{}:{}",
            self.path.display(),
            match &self.kind {
                Kind::File => "file".into(),
                Kind::Block { .. } => "block".into(),
                Kind::Json { keys, .. } => keys.join("/"),
                Kind::Helix { .. } => "helix".into(),
            }
        )
    }
    fn new(path: PathBuf, installed: String, kind: Kind, old: &[Change]) -> Result<Self> {
        let prior = read(&path)?;
        let mut change =
            Self { path, prior: prior.clone(), installed, kind, transaction_prior: prior };
        if let Some(previous) = old.iter().find(|c| c.identity() == change.identity()) {
            change.prior = previous.prior.clone();
            if let (Kind::Json { prior, .. }, Kind::Json { prior: original, .. }) =
                (&mut change.kind, &previous.kind)
            {
                *prior = original.clone();
            }
            if let (Kind::Helix { prior }, Kind::Helix { prior: original }) =
                (&mut change.kind, &previous.kind)
            {
                *prior = original.clone();
            }
        }
        Ok(change)
    }
}
fn read(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
    }
}
fn input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(input).collect()),
        Value::Object(o) => {
            CstInputValue::Object(o.iter().map(|(k, v)| (k.clone(), input(v))).collect())
        }
    }
}
fn object(root: &CstRootNode, keys: &[String], create: bool) -> Result<Option<CstObject>> {
    let mut current = if create {
        anyhow::ensure!(
            root.value().is_none() || root.object_value().is_some(),
            "Settings must contain a JSON object"
        );
        root.object_value_or_set()
    } else {
        let Some(o) = root.object_value() else { return Ok(None) };
        o
    };
    for key in keys {
        if create {
            if let Some(p) = current.get(key) {
                anyhow::ensure!(
                    p.object_value().is_some(),
                    "Settings key {key} must contain an object"
                );
            }
            current = current.object_value_or_set(key);
        } else {
            let Some(next) = current.object_value(key) else { return Ok(None) };
            current = next;
        }
    }
    Ok(Some(current))
}
fn json_change(path: PathBuf, keys: &[&str], value: Value, old: &[Change]) -> Result<Change> {
    let source = read(&path)?.unwrap_or_else(|| "{}\n".into());
    let root = CstRootNode::parse(&source, &ParseOptions::default())?;
    let keys: Vec<_> = keys.iter().map(|k| k.to_string()).collect();
    let parent = object(&root, &keys[..keys.len() - 1], true)?.unwrap();
    let key = keys.last().unwrap();
    let prior = parent.get(key).and_then(|p| p.to_serde_value());
    if let Some(previous) = old.iter().find(|c| c.path == path) {
        if let Kind::Json { value: expected, .. } = &previous.kind {
            anyhow::ensure!(
                prior.as_ref() == Some(expected) || prior.as_ref() == Some(&value),
                "Managed editor setting was edited in {}; refusing to overwrite it",
                path.display()
            );
        }
    }
    if let Some(prop) = parent.get(key) {
        prop.set_value(input(&value));
    } else {
        parent.append(key, input(&value));
    }
    Change::new(path, root.to_string(), Kind::Json { keys, prior, value }, old)
}
fn file_change(path: PathBuf, text: String, old: &[Change]) -> Result<Change> {
    if path.exists() {
        let current = read(&path)?.unwrap_or_default();
        if let Some(previous) = old.iter().find(|c| c.path == path) {
            anyhow::ensure!(
                current == previous.installed || current == text,
                "Managed editor file was edited: {}; refusing to overwrite it",
                path.display()
            );
        } else {
            anyhow::ensure!(
                current == text,
                "Refusing to replace an unmanaged editor file: {}",
                path.display()
            );
        }
    }
    Change::new(path, text, Kind::File, old)
}
#[cfg(unix)]
fn block_change(path: PathBuf, body: String, old: &[Change]) -> Result<Change> {
    let block = format!("\n# >>> sail-lsp managed >>>\n{body}\n# <<< sail-lsp managed <<<\n");
    let mut text = read(&path)?.unwrap_or_default();
    if let Some(previous) = old.iter().find(|c| c.path == path) {
        if let Kind::Block { block } = &previous.kind {
            anyhow::ensure!(
                text.contains(block),
                "Managed block was edited in {}; preserve it or remove it before reconfiguring",
                path.display()
            );
            text = text.replace(block, "");
        }
    } else {
        anyhow::ensure!(
            !text.contains("# >>> sail-lsp managed >>>"),
            "Another installation manages {}; uninstall it first",
            path.display()
        );
    }
    text.push_str(&block);
    Change::new(path, text, Kind::Block { block }, old)
}
fn editor_root(options: &Options) -> Result<PathBuf> {
    if let Some(path) = &options.editor_root {
        return Ok(path.clone());
    }
    if cfg!(windows) {
        Ok(std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or(home()?.join("AppData/Roaming")))
    } else {
        Ok(std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".config")))
    }
}
#[cfg(unix)]
fn shell_quote(path: &Path) -> Result<String> {
    Ok(format!(
        "'{}'",
        path.to_str().context("Non-UTF-8 installation path")?.replace('\'', "'\\''")
    ))
}
/// The editor changes to make, and the verified VS Code extension package
/// that `apply` installs once they are all in place. Planning only reads.
pub(super) fn plan(
    options: &Options,
    payload: &Path,
    manifest: &distribution::Manifest,
    old: &[Change],
) -> Result<(Vec<Change>, Option<Vsix>)> {
    let mut changes = Vec::new();
    let mut vsix = None;
    let server = options.prefix.join("bin").join(executable_name());
    let root = editor_root(options)?;
    let server_json = serde_json::to_string(server.to_str().context("Non-UTF-8 server path")?)?;
    for editor in &options.editors {
        match editor.as_str() {
            "vscode" => {
                vsix = Some(Vsix::prepare(options, payload, manifest)?);
                changes.push(file_change(
                    options.config_home.join("vscode.json"),
                    format!(
                        "{}\n",
                        serde_json::to_string_pretty(
                            &json!({"schemaVersion":1,"path":server,"installation":options.prefix})
                        )?
                    ),
                    old,
                )?);
            }
            "neovim" => {
                let template = include_str!("../../../../editors/templates/neovim.lua");
                let nvim_root = if cfg!(windows) && options.editor_root.is_none() {
                    std::env::var_os("XDG_CONFIG_HOME")
                        .or_else(|| std::env::var_os("LOCALAPPDATA"))
                        .map(PathBuf::from)
                        .unwrap_or(home()?.join("AppData/Local"))
                } else {
                    root.clone()
                };
                let app = std::env::var("NVIM_APPNAME").unwrap_or_else(|_| "nvim".into());
                anyhow::ensure!(
                    Path::new(&app).components().count() == 1
                        && ![".", ".."].contains(&app.as_str()),
                    "Invalid NVIM_APPNAME"
                );
                changes.push(file_change(
                    nvim_root.join(app).join("plugin/sail_lsp_managed.lua"),
                    template.replace("@SERVER_JSON@", &server_json),
                    old,
                )?);
            }
            "emacs" => {
                let template = include_str!("../../../../editors/templates/emacs.el");
                let config = options.config_home.join("emacs.el");
                changes.push(file_change(
                    config.clone(),
                    template.replace("@SERVER_JSON@", &server_json),
                    old,
                )?);
                let init = if options.editor_root.is_some() {
                    root.join("emacs/init.el")
                } else if home()?.join(".emacs").is_file() {
                    home()?.join(".emacs")
                } else if root.join("emacs/init.el").is_file() {
                    root.join("emacs/init.el")
                } else {
                    home()?.join(".emacs.d/init.el")
                };
                let block = format!(
                    "\n;; >>> sail-lsp managed >>>\n(load {} nil t)\n;; <<< sail-lsp managed <<<\n",
                    serde_json::to_string(config.to_str().context("Non-UTF-8 editor path")?)?
                );
                let mut source = read(&init)?.unwrap_or_default();
                if let Some(previous) = old.iter().find(|c| c.path == init) {
                    if let Kind::Block { block } = &previous.kind {
                        anyhow::ensure!(source.contains(block), "Emacs managed block was edited");
                        source = source.replace(block, "");
                    }
                } else {
                    anyhow::ensure!(
                        !source.contains(";; >>> sail-lsp managed >>>"),
                        "Another installation manages Emacs"
                    );
                }
                source.push_str(&block);
                changes.push(Change::new(init, source, Kind::Block { block }, old)?);
            }
            "helix" => {
                changes.push(helix_change(root.join("helix/languages.toml"), &server, old)?);
            }
            "zed" => {
                let source = payload.join("editors/zed");
                anyhow::ensure!(
                    source.join("extension.toml").is_file(),
                    "Distribution is missing the pinned Zed extension sources"
                );
                for file in files(&source)? {
                    let text = fs::read_to_string(source.join(&file))?;
                    changes.push(file_change(
                        options.prefix.join("editors/zed").join(file),
                        text,
                        old,
                    )?);
                }
                changes.push(json_change(
                    root.join("zed/settings.json"),
                    &["lsp", "sail_lsp", "binary", "path"],
                    json!(server),
                    old,
                )?);
            }
            _ => unreachable!(),
        }
    }
    #[cfg(unix)]
    if !options.no_modify_path {
        let bin = shell_quote(&options.prefix.join("bin"))?;
        let body = format!(
            "case \":$PATH:\" in\n  *:{bin}:*) ;;\n  *) export PATH={bin}:\"$PATH\" ;;\nesac"
        );
        if let Some(profile) = &options.profile {
            changes.push(block_change(profile.clone(), body, old)?);
        } else {
            let shell = std::env::var("SHELL").unwrap_or_default();
            if shell.ends_with("fish") {
                let text = format!(
                    "# Managed by sail-lsp\nfish_add_path --prepend -- {}\n",
                    shell_quote(&options.prefix.join("bin"))?
                );
                changes.push(file_change(root.join("fish/conf.d/sail-lsp.fish"), text, old)?);
            } else {
                let profiles = if shell.ends_with("zsh") {
                    let root = std::env::var_os("ZDOTDIR").map(PathBuf::from).unwrap_or(home()?);
                    vec![root.join(".zprofile"), root.join(".zshrc")]
                } else if shell.ends_with("bash") {
                    vec![
                        if home()?.join(".bash_profile").is_file() {
                            home()?.join(".bash_profile")
                        } else {
                            home()?.join(".profile")
                        },
                        home()?.join(".bashrc"),
                    ]
                } else {
                    vec![home()?.join(".profile")]
                };
                for profile in profiles {
                    changes.push(block_change(profile, body.clone(), old)?);
                }
            }
        }
    }
    Ok((changes, vsix))
}
fn files(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, path: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            if entry.file_name() == "target" || entry.file_name() == ".git" {
                continue;
            }
            anyhow::ensure!(
                !entry.file_type()?.is_symlink(),
                "Editor assets cannot contain symlinks"
            );
            if entry.file_type()?.is_dir() {
                visit(root, &entry.path(), out)?;
            } else {
                out.push(entry.path().strip_prefix(root)?.to_path_buf());
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    visit(root, root, &mut out)?;
    out.sort();
    Ok(out)
}
pub(super) fn copy_source_assets(root: &Path, dest: &Path) -> Result<()> {
    let source = root.join("editors/zed");
    for file in files(&source)? {
        let target = dest.join("editors/zed").join(&file);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::copy(source.join(&file), target)?;
    }
    Ok(())
}
fn helix_change(path: PathBuf, server: &Path, old: &[Change]) -> Result<Change> {
    use toml_edit::{value, Array, ArrayOfTables, DocumentMut, Item, Table};
    let original = read(&path)?;
    let mut doc = original.as_deref().unwrap_or("").parse::<DocumentMut>()?;
    let owned = old.iter().find(|c| c.path == path);
    if let Some(previous) = owned {
        let expected = previous.installed.parse::<DocumentMut>()?;
        anyhow::ensure!(
            doc.get("language-server")
                .and_then(|t| t.get("sail-lsp-managed"))
                .map(ToString::to_string)
                == expected
                    .get("language-server")
                    .and_then(|t| t.get("sail-lsp-managed"))
                    .map(ToString::to_string),
            "Managed Helix server setting was edited; refusing to overwrite it"
        );
    }
    anyhow::ensure!(owned.is_some()||doc.get("language-server").and_then(|t|t.get("sail-lsp-managed")).is_none(),"Helix already defines sail-lsp-managed; choose another configuration or remove that definition");
    doc["language-server"]["sail-lsp-managed"]["command"] =
        value(server.to_str().context("Non-UTF-8 server path")?);
    if doc.get("language").is_none() {
        doc["language"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let languages = doc["language"]
        .as_array_of_tables_mut()
        .context("Helix language must be an array of tables")?;
    // Both languages the server speaks, as the VS Code extension declares
    // them: Sail sources, and the project manifests the server answers
    // completion, definition and diagnostics in. Helix matches file types
    // by extension or by glob; a workspace root is a file name.
    let mut manifest_types = Array::new();
    for glob in ["*.sail_project", "sail.proj"] {
        let mut entry = toml_edit::InlineTable::new();
        entry.insert("glob", glob.into());
        manifest_types.push(entry);
    }
    let mut source_types = Array::new();
    source_types.push("sail");
    for (name, scope, types) in [
        ("sail", "source.sail", source_types),
        ("sail-project", "source.sail-project", manifest_types),
    ] {
        let index =
            languages.iter().position(|t| t.get("name").and_then(Item::as_str) == Some(name));
        let index = if let Some(index) = index {
            index
        } else {
            let mut table = Table::new();
            table["name"] = value(name);
            table["scope"] = value(scope);
            table["file-types"] = value(types);
            let mut roots = Array::new();
            roots.push("sail.proj");
            roots.push(".git");
            table["roots"] = value(roots);
            languages.push(table);
            languages.len() - 1
        };
        let language = languages.get_mut(index).unwrap();
        let mut servers =
            language.get("language-servers").and_then(Item::as_array).cloned().unwrap_or_default();
        if !servers.iter().any(|v| v.as_str() == Some("sail-lsp-managed")) {
            servers.insert(0, "sail-lsp-managed");
        }
        language["language-servers"] = value(servers);
    }
    Change::new(path, doc.to_string(), Kind::Helix { prior: original }, old)
}
pub(super) fn apply(changes: &mut [Change], vsix: Option<&mut Vsix>) -> Result<Vec<Change>> {
    let mut applied = Vec::new();
    for change in changes {
        let current = read(&change.path)?;
        if current != change.transaction_prior {
            restore(&applied);
            bail!("Configuration changed during install: {}", change.path.display());
        }
        if let Err(error) = write_editor(&change.path, change.installed.as_bytes()) {
            restore(&applied);
            return Err(error);
        }
        applied.push(change.clone());
    }
    // The extension goes in last: it is the one change another process owns.
    if let Some(vsix) = vsix {
        if let Err(error) = vsix.install() {
            restore(&applied);
            return Err(error);
        }
    }
    Ok(applied)
}
pub(super) fn restore(changes: &[Change]) {
    for change in changes.iter().rev() {
        let _ = restore_text(&change.path, change.transaction_prior.as_deref());
    }
}
// Dotfile managers commonly symlink editor settings. Replace the target file
// atomically while keeping that user-owned link and directory layout intact.
fn write_editor(path: &Path, bytes: &[u8]) -> Result<()> {
    let target = if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        path.canonicalize()?
    } else {
        path.to_path_buf()
    };
    atomic_write(&target, bytes)
}
fn restore_text(path: &Path, text: Option<&str>) -> Result<()> {
    if let Some(text) = text {
        write_editor(path, text.as_bytes())
    } else {
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
#[allow(clippy::print_stdout)]
pub(super) fn undo(changes: &[Change]) {
    for change in changes.iter().rev() {
        if let Err(error) = undo_one(change) {
            println!("Preserved {}: {error:#}", change.path.display());
        }
    }
}
fn undo_one(change: &Change) -> Result<()> {
    let Some(current) = read(&change.path)? else { return Ok(()) };
    match &change.kind {
        Kind::File => {
            anyhow::ensure!(current == change.installed, "file has user changes");
            restore_text(&change.path, change.prior.as_deref())?;
        }
        Kind::Block { block } => {
            anyhow::ensure!(current.contains(block), "managed block was edited");
            let restored = current.replace(block, "");
            if restored.is_empty() && change.prior.is_none() {
                restore_text(&change.path, None)?;
            } else {
                write_editor(&change.path, restored.as_bytes())?;
            }
        }
        Kind::Json { keys, prior, value } => {
            let root = CstRootNode::parse(&current, &ParseOptions::default())?;
            if let Some(parent) = object(&root, &keys[..keys.len() - 1], false)? {
                if let Some(prop) = parent.get(keys.last().unwrap()) {
                    anyhow::ensure!(
                        prop.to_serde_value().as_ref() == Some(value),
                        "managed setting was edited"
                    );
                    if let Some(prior) = prior {
                        prop.set_value(input(prior));
                    } else {
                        prop.remove();
                    }
                }
            }
            let original: Value = jsonc_parser::parse_to_serde_value(
                change.prior.as_deref().unwrap_or("{}"),
                &ParseOptions::default(),
            )?;
            for depth in (1..keys.len()).rev() {
                let existed =
                    keys[..depth].iter().try_fold(&original, |value, key| value.get(key)).is_some();
                if !existed {
                    if let Some(parent) = object(&root, &keys[..depth - 1], false)? {
                        if let Some(prop) = parent.get(&keys[depth - 1]) {
                            if prop.object_value().is_some_and(|o| o.properties().is_empty())
                                && !prop.to_string().contains("//")
                                && !prop.to_string().contains("/*")
                            {
                                prop.remove();
                            }
                        }
                    }
                }
            }
            let restored = root.to_string();
            if change.prior.is_none()
                && root.object_value().is_some_and(|o| o.properties().is_empty())
                && !restored.contains("//")
                && !restored.contains("/*")
            {
                restore_text(&change.path, None)?;
            } else {
                write_editor(&change.path, restored.as_bytes())?;
            }
        }
        Kind::Helix { prior } => {
            use toml_edit::{DocumentMut, Item};
            let mut doc = current.parse::<DocumentMut>()?;
            let original = prior.as_deref().unwrap_or("").parse::<DocumentMut>()?;
            let expected = change.installed.parse::<DocumentMut>()?;
            if doc
                .get("language-server")
                .and_then(|v| v.get("sail-lsp-managed"))
                .map(ToString::to_string)
                == expected
                    .get("language-server")
                    .and_then(|v| v.get("sail-lsp-managed"))
                    .map(ToString::to_string)
            {
                if let Some(table) =
                    doc.get_mut("language-server").and_then(Item::as_table_like_mut)
                {
                    table.remove("sail-lsp-managed");
                }
            }
            for name in ["sail", "sail-project"] {
                if let Some(languages) =
                    doc.get_mut("language").and_then(Item::as_array_of_tables_mut)
                {
                    for language in languages.iter_mut() {
                        if language.get("name").and_then(Item::as_str) == Some(name) {
                            let previous = original
                                .get("language")
                                .and_then(Item::as_array_of_tables)
                                .and_then(|a| {
                                    a.iter().find(|t| {
                                        t.get("name").and_then(Item::as_str) == Some(name)
                                    })
                                })
                                .and_then(|t| t.get("language-servers"));
                            if let Some(servers) =
                                language.get_mut("language-servers").and_then(Item::as_array_mut)
                            {
                                servers.retain(|v| v.as_str() != Some("sail-lsp-managed"));
                                if servers.is_empty() {
                                    if let Some(previous) = previous {
                                        language["language-servers"] = previous.clone();
                                    } else {
                                        language.remove("language-servers");
                                    }
                                }
                            }
                        }
                    }
                }
                // Remove a language table created by us only if every remaining field
                // still matches the generated table. Keep user additions and edits.
                let had_language =
                    original.get("language").and_then(Item::as_array_of_tables).is_some_and(|a| {
                        a.iter().any(|t| t.get("name").and_then(Item::as_str) == Some(name))
                    });
                if !had_language {
                    let generated = expected
                        .get("language")
                        .and_then(Item::as_array_of_tables)
                        .and_then(|a| {
                            a.iter().find(|t| t.get("name").and_then(Item::as_str) == Some(name))
                        })
                        .cloned();
                    if let (Some(languages), Some(mut generated)) =
                        (doc.get_mut("language").and_then(Item::as_array_of_tables_mut), generated)
                    {
                        generated.remove("language-servers");
                        languages.retain(|t| {
                            t.get("name").and_then(Item::as_str) != Some(name)
                                || t.to_string() != generated.to_string()
                        });
                        if languages.is_empty() {
                            doc.remove("language");
                        }
                    }
                }
            }
            if original.get("language-server").is_none()
                && doc
                    .get("language-server")
                    .and_then(Item::as_table_like)
                    .is_some_and(|t| t.is_empty())
            {
                doc.remove("language-server");
            }
            let restored = doc.to_string();
            if change.prior.is_none() && restored.trim().is_empty() {
                restore_text(&change.path, None)?;
            } else {
                write_editor(&change.path, restored.as_bytes())?;
            }
        }
    }
    Ok(())
}
const EXTENSION_ID: &str = "Lunarwwww.sail-lsp";
/// A verified VS Code extension package waiting to be installed.
pub(super) struct Vsix {
    path: PathBuf,
    code: PathBuf,
    /// Keeps a downloaded package until it is installed.
    _download: tempfile::TempDir,
    /// The extension was absent before this run, so removing it on rollback
    /// undoes only this installer's work.
    fresh: bool,
    installed: bool,
}
impl Vsix {
    fn prepare(
        options: &Options,
        payload: &Path,
        manifest: &distribution::Manifest,
    ) -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let asset = format!("sail-lsp-{}.vsix", manifest.target);
        let bundled = payload.join(&asset);
        let vsix = if let Some(vsix) = &options.vsix {
            vsix.clone()
        } else if bundled.is_file() {
            // The payload drops its package before it is stored; keep a copy.
            let target = temp.path().join(&asset);
            fs::copy(bundled, &target)?;
            target
        } else {
            let target = temp.path().join(&asset);
            let checksum = temp.path().join("checksum");
            download(&release_url(&options.release_base, &manifest.tag, &asset)?, &target)?;
            download(
                &release_url(&options.release_base, &manifest.tag, &format!("{asset}.sha256"))?,
                &checksum,
            )?;
            anyhow::ensure!(
                sha256(&target)?
                    == fs::read_to_string(checksum)?
                        .split_whitespace()
                        .next()
                        .context("Empty VSIX checksum")?,
                "VSIX checksum mismatch"
            );
            target
        };
        let mut zip = zip::ZipArchive::new(fs::File::open(&vsix)?)?;
        let package: Value = serde_json::from_reader(zip.by_name("extension/package.json")?)?;
        anyhow::ensure!(
            package["name"] == "sail-lsp"
                && package["publisher"] == "Lunarwwww"
                && package["version"] == manifest.version,
            "VSIX identity/version does not match the server release"
        );
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut server = zip.by_name(&format!("extension/server/{}", executable_name()))?;
        let mut bytes = Vec::new();
        server.read_to_end(&mut bytes)?;
        anyhow::ensure!(
            format!("{:x}", Sha256::digest(bytes)) == manifest.binary_sha256,
            "VSIX contains a different server binary"
        );
        let listed = checked(
            code_command(&options.code, &["--list-extensions".as_ref()]),
            Duration::from_secs(60),
        )
        .context(
            "Cannot list VS Code extensions; install the `code` command or specify --code-command",
        )?;
        let fresh = !String::from_utf8_lossy(&listed.stdout)
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case(EXTENSION_ID));
        Ok(Self {
            path: vsix,
            code: options.code.clone(),
            _download: temp,
            fresh,
            installed: false,
        })
    }
    fn install(&mut self) -> Result<()> {
        let command = code_command(
            &self.code,
            &["--install-extension".as_ref(), self.path.as_os_str(), "--force".as_ref()],
        );
        checked(command,Duration::from_secs(180)).context("VS Code extension installation failed; install the `code` command or specify --code-command")?;
        self.installed = true;
        Ok(())
    }
    /// Remove the extension if this run installed it where there was none.
    /// An upgraded extension stays: its predecessor cannot be put back.
    pub(super) fn undo(&self) {
        if self.installed && self.fresh {
            let command = code_command(
                &self.code,
                &["--uninstall-extension".as_ref(), EXTENSION_ID.as_ref()],
            );
            let _ = checked(command, Duration::from_secs(180));
        }
    }
}
/// `code <args>`; on Windows the CLI is a .cmd that needs a shell.
fn code_command(code: &Path, args: &[&std::ffi::OsStr]) -> Command {
    #[cfg(not(windows))]
    {
        let mut c = Command::new(code);
        c.args(args);
        c
    }
    #[cfg(windows)]
    {
        let mut c = Command::new("powershell.exe");
        let mut script = String::from("& $env:SAIL_LSP_CODE_COMMAND");
        for (i, arg) in args.iter().enumerate() {
            let key = format!("SAIL_LSP_CODE_ARG{i}");
            script.push_str(&format!(" $env:{key}"));
            c.env(key, arg);
        }
        script.push_str("; exit $LASTEXITCODE");
        c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
        c.env("SAIL_LSP_CODE_COMMAND", code);
        c
    }
}
pub(super) fn build_source_vsix(
    root: &Path,
    dest: &Path,
    binary: &Path,
    manifest: &distribution::Manifest,
) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let ext = temp.path().join("code");
    fs::create_dir(&ext)?;
    fn copy(from: &Path, to: &Path) -> Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let name = entry.file_name();
            if ["node_modules", "out", "server", ".vscode-test", ".git"].iter().any(|n| name == *n)
                || name.to_string_lossy().ends_with(".vsix")
            {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs::create_dir_all(to.join(&name))?;
                copy(&entry.path(), &to.join(&name))?;
            } else {
                fs::copy(entry.path(), to.join(name))?;
            }
        }
        Ok(())
    }
    copy(&root.join("editors/code"), &ext)?;
    fs::create_dir(ext.join("server"))?;
    fs::copy(binary, ext.join("server").join(executable_name()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            ext.join("server").join(executable_name()),
            fs::Permissions::from_mode(0o755),
        )?;
    }
    for args in [vec!["ci"], vec!["run", "vscode:prepublish"]] {
        let mut c = Command::new(if cfg!(windows) { "npm.cmd" } else { "npm" });
        c.args(args).current_dir(&ext);
        checked(c, Duration::from_secs(600))?;
    }
    let target = match env!("SAIL_LSP_BUILD_TARGET") {
        "x86_64-unknown-linux-gnu" => "linux-x64",
        "aarch64-unknown-linux-gnu" => "linux-arm64",
        "x86_64-pc-windows-msvc" => "win32-x64",
        "aarch64-pc-windows-msvc" => "win32-arm64",
        "x86_64-apple-darwin" => "darwin-x64",
        "aarch64-apple-darwin" => "darwin-arm64",
        _ => bail!("No VSIX target mapping for this host"),
    };
    let mut c = Command::new(if cfg!(windows) { "npx.cmd" } else { "npx" });
    c.args(["--no-install", "vsce", "package", "--target", target, "--out"])
        .arg(dest.join(format!("sail-lsp-{}.vsix", manifest.target)))
        .args(["--allow-missing-repository"])
        .current_dir(&ext);
    checked(c, Duration::from_secs(300))?;
    Ok(())
}
/// PowerShell that adds `$env:SAIL_LSP_MANAGED_BIN` to the user Path, or
/// removes it. The value is read unexpanded and written back with its own
/// registry type (REG_EXPAND_SZ when new), so `%USERPROFILE%\.cargo\bin`
/// entries stay variable; .NET's Get/SetEnvironmentVariable would expand
/// them and store the result as REG_SZ. Explorer and new consoles learn of
/// the change from WM_SETTINGCHANGE, which .NET broadcast on our behalf and
/// a plain registry write does not; the P/Invoke signature arrives in
/// `$env:SAIL_LSP_SETTINGCHANGE` so the script holds no double quotes.
#[cfg(any(windows, test))]
fn windows_path_script(add: bool) -> String {
    const OPEN: &str = "$k=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment'); $p=[string]$k.GetValue('Path','',[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames); $kind=if($k.GetValueNames() -contains 'Path'){$k.GetValueKind('Path')}else{[Microsoft.Win32.RegistryValueKind]::ExpandString};";
    const CLOSE: &str = "$k.Close(); try { $n=Add-Type -Namespace SailLsp -Name Native -PassThru -MemberDefinition $env:SAIL_LSP_SETTINGCHANGE; $r=[UIntPtr]::Zero; [void]$n::SendMessageTimeout([IntPtr]0xffff,0x1A,[UIntPtr]::Zero,'Environment',2,5000,[ref]$r) } catch {}";
    let edit = if add {
        "$a=@($p -split ';' | Where-Object { $_ }); if($a -notcontains $env:SAIL_LSP_MANAGED_BIN){ $k.SetValue('Path',(($a + $env:SAIL_LSP_MANAGED_BIN) -join ';'),$kind); Write-Output 'added' };"
    } else {
        "$k.SetValue('Path',(($p -split ';' | Where-Object { $_ -and $_ -ine $env:SAIL_LSP_MANAGED_BIN }) -join ';'),$kind);"
    };
    format!("{OPEN} {edit} {CLOSE}")
}
#[cfg(windows)]
pub(super) fn windows_path(bin: &Path, add: bool) -> Result<bool> {
    const SETTINGCHANGE: &str = "[DllImport(\"user32.dll\", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);";
    let mut c = Command::new("powershell.exe");
    c.args(["-NoProfile", "-NonInteractive", "-Command", &windows_path_script(add)])
        .env("SAIL_LSP_MANAGED_BIN", bin)
        .env("SAIL_LSP_SETTINGCHANGE", SETTINGCHANGE);
    // Add-Type compiles the broadcast helper; allow for a cold compiler.
    Ok(String::from_utf8_lossy(&checked(c, Duration::from_secs(60))?.stdout).contains("added"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use sha2::Digest;
    #[cfg(unix)]
    use std::io::Write;

    /// A `code` CLI that records its arguments and lists what `listing` holds.
    #[cfg(unix)]
    fn fake_code(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let (code, log, listing) = (dir.join("code"), dir.join("code.log"), dir.join("listing"));
        fs::write(&listing, "").unwrap();
        fs::write(
            &code,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\n[ \"$1\" = --list-extensions ] && cat '{}'\nexit 0\n",
                log.display(),
                listing.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&code, fs::Permissions::from_mode(0o755)).unwrap();
        (code, log, listing)
    }
    #[cfg(unix)]
    fn fake_vsix(dir: &Path, manifest: &distribution::Manifest, server: &[u8]) -> PathBuf {
        let path = dir.join("sail-lsp.vsix");
        let mut zip = zip::ZipWriter::new(fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("extension/package.json", options).unwrap();
        zip.write_all(
            json!({"name":"sail-lsp","publisher":"Lunarwwww","version":manifest.version})
                .to_string()
                .as_bytes(),
        )
        .unwrap();
        zip.start_file(format!("extension/server/{}", executable_name()), options).unwrap();
        zip.write_all(server).unwrap();
        zip.finish().unwrap();
        path
    }
    #[cfg(unix)]
    fn manifest(server: &[u8]) -> distribution::Manifest {
        distribution::Manifest {
            schema_version: 1,
            version: "0.0.1".into(),
            tag: "v0.0.1".into(),
            revision: "0000000".into(),
            target: env!("SAIL_LSP_BUILD_TARGET").into(),
            stdlib_revision: "0000000".into(),
            solver: "z3-vendored".into(),
            binary: executable_name().into(),
            binary_sha256: format!("{:x}", sha2::Sha256::digest(server)),
            crt_static: false,
            requirements: json!({}),
        }
    }
    #[cfg(unix)]
    fn log(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    #[cfg(unix)]
    fn the_extension_is_installed_only_once_every_editor_change_is_in_place() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let (code, code_log, listing) = fake_code(root);
        let server = b"not really a server";
        let manifest = manifest(server);
        let vsix = fake_vsix(root, &manifest, server);
        let args: Vec<String> = [
            "--prefix",
            "prefix",
            "--config-home",
            "config",
            "--editor-root",
            "editors",
            "--editor",
            "vscode,helix",
            "--no-modify-path",
            "--vsix",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain([vsix.display().to_string(), "--code-command".into(), code.display().to_string()])
        .collect();
        let args: Vec<String> = args
            .into_iter()
            .map(|a| match a.as_str() {
                "prefix" | "config" | "editors" => root.join(a).display().to_string(),
                _ => a,
            })
            .collect();
        let options = Options::parse(&args).unwrap();
        let payload = root.join("payload");
        fs::create_dir(&payload).unwrap();

        // Helix refuses the plan; VS Code, planned first, must not have acted.
        let languages = root.join("editors/helix/languages.toml");
        fs::create_dir_all(languages.parent().unwrap()).unwrap();
        fs::write(&languages, "[language-server.sail-lsp-managed]\ncommand = \"theirs\"\n")
            .unwrap();
        let error = plan(&options, &payload, &manifest, &[]).err().expect("Helix conflict");
        assert!(error.to_string().contains("Helix already defines"), "{error:#}");
        assert!(!log(&code_log).contains("--install-extension"), "{}", log(&code_log));

        // With the conflict gone, planning still only reads; applying writes
        // every file and installs the extension last.
        fs::remove_file(&languages).unwrap();
        let (mut changes, mut vsix) = plan(&options, &payload, &manifest, &[]).unwrap();
        assert!(!log(&code_log).contains("--install-extension"), "{}", log(&code_log));
        let applied = apply(&mut changes, vsix.as_mut()).unwrap();
        assert_eq!(applied.len(), 2);
        assert!(root.join("config/vscode.json").is_file() && languages.is_file());
        let installs =
            |log: &str| log.lines().filter(|line| line.starts_with("--install-extension ")).count();
        assert_eq!(installs(&log(&code_log)), 1, "{}", log(&code_log));
        // Rolling back removes an extension nobody had before this run...
        vsix.unwrap().undo();
        assert!(log(&code_log).contains("--uninstall-extension Lunarwwww.sail-lsp"));
        restore(&applied);
        // ...and keeps one that was already there: its predecessor is gone.
        fs::write(&listing, "lunarwwww.sail-lsp\n").unwrap();
        let (mut changes, mut vsix) = plan(&options, &payload, &manifest, &[]).unwrap();
        apply(&mut changes, vsix.as_mut()).unwrap();
        let before = log(&code_log);
        assert_eq!(installs(&before), 2);
        vsix.unwrap().undo();
        assert_eq!(log(&code_log), before);
    }

    /// Helix is told about both languages the server speaks — the manifests
    /// too, which it would otherwise open as plain text with no server —
    /// and an existing `sail` entry keeps its other servers ahead of ours.
    #[test]
    fn helix_gets_the_manifest_language_next_to_the_source_language() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("languages.toml");
        fs::write(
            &path,
            "[[language]]\nname = \"sail\"\nscope = \"source.sail\"\nfile-types = [\"sail\"]\nlanguage-servers = [\"other\"]\n",
        )
        .unwrap();
        let change = helix_change(path, Path::new("/opt/sail lsp/bin/sail-lsp"), &[]).unwrap();
        let doc: toml_edit::DocumentMut = change.installed.parse().unwrap();
        assert_eq!(
            doc["language-server"]["sail-lsp-managed"]["command"].as_str(),
            Some("/opt/sail lsp/bin/sail-lsp")
        );
        let languages = doc["language"].as_array_of_tables().unwrap();
        let by_name = |name: &str| {
            languages
                .iter()
                .find(|l| l["name"].as_str() == Some(name))
                .unwrap_or_else(|| panic!("no {name} language in {}", change.installed))
        };
        let strings = |item: &toml_edit::Item| -> Vec<String> {
            item.as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    v.as_str()
                        .or_else(|| v.as_inline_table()?.get("glob")?.as_str())
                        .unwrap()
                        .to_owned()
                })
                .collect()
        };
        let sail = by_name("sail");
        assert_eq!(strings(&sail["language-servers"]), ["sail-lsp-managed", "other"]);
        let manifests = by_name("sail-project");
        assert_eq!(manifests["scope"].as_str(), Some("source.sail-project"));
        assert_eq!(strings(&manifests["language-servers"]), ["sail-lsp-managed"]);
        assert_eq!(strings(&manifests["file-types"]), ["*.sail_project", "sail.proj"]);
        assert_eq!(strings(&manifests["roots"]), ["sail.proj", ".git"]);
    }

    #[test]
    fn helix_uninstall_removes_both_generated_languages() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("languages.toml");
        let mut changes =
            vec![helix_change(path.clone(), Path::new("/sail/bin/sail-lsp"), &[]).unwrap()];
        apply(&mut changes, None).unwrap();
        undo_one(&changes[0]).unwrap();
        assert!(
            read(&path).unwrap().is_none(),
            "{}",
            fs::read_to_string(&path).unwrap_or_default()
        );
    }

    #[test]
    fn helix_uninstall_preserves_existing_languages_and_manifest_customizations() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("languages.toml");
        let original = "[[language]]\nname = \"sail\"\nlanguage-servers = [\"other\"]\n\n[[language]]\nname = \"sail-project\"\nlanguage-servers = [\"manifest-server\"]\n";
        fs::write(&path, original).unwrap();
        let mut changes =
            vec![helix_change(path.clone(), Path::new("/sail/bin/sail-lsp"), &[]).unwrap()];
        apply(&mut changes, None).unwrap();
        let mut installed: toml_edit::DocumentMut =
            fs::read_to_string(&path).unwrap().parse().unwrap();
        installed["language"].as_array_of_tables_mut().unwrap().get_mut(1).unwrap()["formatter"]
            ["command"] = toml_edit::value("my-formatter");
        fs::write(&path, installed.to_string()).unwrap();
        undo_one(&changes[0]).unwrap();
        let restored = fs::read_to_string(&path).unwrap();
        assert!(!restored.contains("sail-lsp-managed"), "{restored}");
        let doc: toml_edit::DocumentMut = restored.parse().unwrap();
        let languages = doc["language"].as_array_of_tables().unwrap();
        assert_eq!(languages.len(), 2);
        assert_eq!(languages.get(0).unwrap()["language-servers"][0].as_str(), Some("other"));
        let manifest = languages.get(1).unwrap();
        assert_eq!(manifest["language-servers"][0].as_str(), Some("manifest-server"));
        assert_eq!(manifest["formatter"]["command"].as_str(), Some("my-formatter"));
    }

    #[test]
    fn the_user_path_is_edited_without_expanding_or_retyping_it() {
        for add in [true, false] {
            let script = windows_path_script(add);
            assert!(
                script.contains("DoNotExpandEnvironmentNames") && script.contains("ExpandString"),
                "{script}"
            );
            assert!(!script.contains("EnvironmentVariable"), "{script}");
        }
    }
}
