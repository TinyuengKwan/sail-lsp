//! Standalone CLI: no live database or LSP state. Uses the same pure engine.
use super::resolve;
use anyhow::{bail, Context};
use ide::formatting::FormatStatus;
use ide_db::ide_types::FormatOptions;
use std::path::{Path, PathBuf};

pub(crate) fn run(args: &[String]) -> Result<(), String> {
    cli_inner(args).map_err(|error| format!("{error:#}"))
}
fn cli_inner(args: &[String]) -> anyhow::Result<()> {
    use std::io::{Read, Write};
    let mut check = false;
    let mut explain = false;
    let mut config = None;
    let mut stdin_path = None;
    let mut paths = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--check" => check = true,
            "--explain" => explain = true,
            "--config" => {
                config = Some(PathBuf::from(args.next().context("--config requires a path")?))
            }
            "--stdin-filepath" => {
                stdin_path =
                    Some(PathBuf::from(args.next().context("--stdin-filepath requires a path")?))
            }
            "--help" | "-h" => {
                println!("Usage: sail-lsp fmt [--check] [--explain] [--config PATH] <file-or-directory>...\n       sail-lsp fmt [--check] [--config PATH] --stdin-filepath PATH");
                return Ok(());
            }
            value if value.starts_with('-') => bail!("Unknown formatter option {value}"),
            path => paths.push(PathBuf::from(path)),
        }
    }
    let cwd = std::env::current_dir()?;
    let absolute = |p: PathBuf| if p.is_absolute() { p } else { cwd.join(p) };
    let fallback = || FormatOptions { tab_size: 2, ..Default::default() };
    let mut inputs = Vec::new();
    if let Some(path) = stdin_path {
        anyhow::ensure!(paths.is_empty(), "--stdin-filepath cannot be combined with files");
        let mut source = String::new();
        std::io::stdin().read_to_string(&mut source)?;
        inputs.push((absolute(path), source, true));
    } else {
        anyhow::ensure!(!paths.is_empty(), "Usage: sail-lsp fmt [--check] <file-or-directory>...");
        let mut files = std::collections::BTreeMap::<PathBuf, PathBuf>::new();
        let mut add_file = |path: &Path| -> anyhow::Result<()> {
            let canonical =
                path.canonicalize().with_context(|| format!("Cannot open {}", path.display()))?;
            if let Some(previous) = files.get(&canonical) {
                let a = resolve(previous, fallback(), config.as_deref())?.options;
                let b = resolve(path, fallback(), config.as_deref())?.options;
                anyhow::ensure!(
                    (a.tab_size, a.insert_spaces, a.max_line_width)
                        == (b.tab_size, b.insert_spaces, b.max_line_width),
                    "Conflicting formatter styles for aliases {} and {}",
                    previous.display(),
                    path.display()
                );
            } else {
                files.insert(canonical, path.to_path_buf());
            }
            Ok(())
        };
        for path in paths.into_iter().map(absolute) {
            if path.is_dir() {
                for entry in walkdir::WalkDir::new(&path).into_iter().filter_entry(|e| {
                    !matches!(e.file_name().to_str(), Some(".git" | "target" | "node_modules"))
                }) {
                    let entry = entry?;
                    if entry.file_type().is_file()
                        && entry.path().extension().is_some_and(|e| e == "sail")
                    {
                        add_file(entry.path())?;
                    }
                }
            } else {
                add_file(&path)?;
            }
        }
        anyhow::ensure!(!files.is_empty(), "No Sail files selected");
        for path in files.into_values() {
            inputs.push((path.clone(), std::fs::read_to_string(&path)?, false));
        }
    }
    // Validate all selected inputs before writing any of them.
    let mut outputs = Vec::new();
    let mut changed = false;
    for (path, source, stdin) in inputs {
        let resolved = resolve(&path, fallback(), config.as_deref())?;
        let result = ide::formatting::format_text(&source, &resolved.options);
        if explain {
            eprintln!(
                "{}",
                serde_json::json!({"file":path,"status":result.status.as_str(),"details":result.details,"configFile":resolved.path,"styleVersion":1})
            );
        }
        anyhow::ensure!(
            !matches!(result.status, FormatStatus::ParseError | FormatStatus::SafetyCheckFailed),
            "{}: {}: {}",
            path.display(),
            result.status.as_str(),
            result.details.join("; ")
        );
        if result.status == FormatStatus::Changed {
            changed = true;
            if check {
                eprintln!("Would format {}", path.display());
            }
        }
        outputs.push((path, result, stdin, source));
    }
    if check {
        anyhow::ensure!(!changed, "Formatting differs; run sail-lsp fmt without --check");
        return Ok(());
    }
    for (path, result, stdin, source) in outputs {
        if stdin {
            std::io::stdout().write_all(result.text.as_bytes())?;
        } else if result.status == FormatStatus::Changed {
            let destination = path.canonicalize()?;
            anyhow::ensure!(
                std::fs::read_to_string(&destination)? == source,
                "{} changed while formatting; no replacement written",
                path.display()
            );
            let mut temp = tempfile::NamedTempFile::new_in(
                destination.parent().context("File has no parent")?,
            )?;
            temp.as_file_mut().set_permissions(destination.metadata()?.permissions())?;
            temp.write_all(result.text.as_bytes())?;
            temp.flush()?;
            temp.persist(&destination)
                .with_context(|| format!("Cannot replace {}", path.display()))?;
        }
    }
    Ok(())
}
