//! Filesystem/configuration boundary for the syntax-only IDE formatter.
use anyhow::{bail, Context};
use ide::formatting::FormatResult;
mod command;
pub(crate) use command::run as cli;
use ide_db::{ide_types::FormatOptions, SourceFileInfo};
use std::path::{Path, PathBuf};

pub(crate) struct Resolved {
    pub options: FormatOptions,
    pub path: Option<PathBuf>,
}

/// Nearest project style wins over editor indentation. No filesystem reads
/// occur in IDE queries; the CLI and LSP resolve the same file here.
pub(crate) fn resolve(
    path: &Path,
    mut options: FormatOptions,
    explicit: Option<&Path>,
) -> anyhow::Result<Resolved> {
    let paths: Vec<_> = if let Some(explicit) = explicit {
        vec![explicit.to_path_buf()]
    } else {
        path.parent()
            .into_iter()
            .flat_map(Path::ancestors)
            .map(|p| p.join(".sailfmt.toml"))
            .collect()
    };
    for path in paths {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                continue
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("Cannot read formatter config {}", path.display()))
            }
        };
        let doc: toml_edit::DocumentMut =
            text.parse().with_context(|| format!("Invalid formatter config {}", path.display()))?;
        for (key, value) in doc.iter() {
            let integer = |min, max| {
                value.as_integer().filter(|v| (min..=max).contains(v)).ok_or_else(|| {
                    anyhow::anyhow!("{}: {key} must be an integer in {min}..={max}", path.display())
                })
            };
            match key {
                "style_version" => {
                    integer(1, 1)?;
                }
                "line_width" => options.max_line_width = Some(integer(1, 10000)? as u32),
                "tab_size" => options.tab_size = integer(1, 32)? as u32,
                "insert_spaces" => {
                    options.insert_spaces = value.as_bool().ok_or_else(|| {
                        anyhow::anyhow!("{}: insert_spaces must be a boolean", path.display())
                    })?
                }
                _ => bail!("{}: unknown formatter option {key}", path.display()),
            }
        }
        return Ok(Resolved { options, path: Some(path) });
    }
    Ok(Resolved { options, path: None })
}

pub(crate) fn options(
    snap: &crate::global_state::GlobalStateSnapshot,
    uri: &url::Url,
    client: Option<&lsp_types::FormattingOptions>,
) -> anyhow::Result<Resolved> {
    let mut options = client.map(crate::from_proto::format_options).unwrap_or_else(|| {
        snap.last_format_options
            .lock()
            .ok()
            .and_then(|m| m.get(uri).cloned())
            .unwrap_or_else(crate::code_action_helpers::default_code_action_format_options)
    });
    options.max_line_width = Some(snap.config.formatting.line_width.max(1));
    let client_options = options.clone();
    let resolved = if let Ok(path) = uri.to_file_path() {
        resolve(&path, options, None)?
    } else {
        Resolved { options, path: None }
    };
    if client.is_some() {
        if let Ok(mut cached) = snap.last_format_options.lock() {
            cached.insert(uri.clone(), client_options);
        }
    }
    Ok(resolved)
}

pub(crate) fn result(
    snap: &crate::global_state::GlobalStateSnapshot,
    uri: &url::Url,
    options: &FormatOptions,
    range: Option<lsp_types::Range>,
) -> anyhow::Result<Option<FormatResult>> {
    let result = if let Some(range) = range {
        let Some(file) = snap.analysis.file(uri) else { return Ok(None) };
        let index = ide_db::line_index::LineIndex::new(file.text());
        snap.analysis.range_format(uri, crate::from_proto::text_range(&index, range), options)?
    } else {
        let Some(file) = snap.analysis.file_id(uri) else { return Ok(None) };
        snap.analysis.format_document(file, options)?
    };
    if let Some(result) = &result {
        tracing::debug!(target:"sail_lsp::formatting", %uri, status=result.status.as_str(), details=?result.details, "formatting result");
    }
    Ok(result)
}

pub(crate) fn explain(
    snap: crate::global_state::GlobalStateSnapshot,
    params: crate::lsp_ext::ExplainFormattingParams,
) -> anyhow::Result<serde_json::Value> {
    let resolved = options(&snap, &params.text_document.uri, params.options.as_ref())?;
    let result = result(&snap, &params.text_document.uri, &resolved.options, params.range)?;
    Ok(
        serde_json::json!({"status":result.as_ref().map(|r|r.status.as_str()).unwrap_or("unavailable"),
        "details":result.as_ref().map(|r|&r.details), "configFile":resolved.path,
        "styleVersion":1, "options":{"lineWidth":resolved.options.max_width(),"tabSize":resolved.options.tab_size,"insertSpaces":resolved.options.insert_spaces}}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nearest_project_style_overrides_editor_and_is_reloaded_without_restart() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            dir.path().join(".sailfmt.toml"),
            "style_version = 1\nline_width = 60\ntab_size = 4\ninsert_spaces = false\n",
        )
        .unwrap();
        let source = nested.join("file.sail");
        let options = resolve(&source, FormatOptions::default(), None).unwrap();
        assert_eq!(options.options.max_width(), 60);
        assert_eq!(options.options.tab_size, 4);
        assert!(options.options.hard_tabs());
        std::fs::write(nested.join(".sailfmt.toml"), "tab_size = 2\n").unwrap();
        let options = resolve(&source, FormatOptions::default(), None).unwrap();
        assert_eq!(options.options.tab_size, 2);
        assert_eq!(options.options.max_width(), 100);
        std::fs::write(nested.join(".sailfmt.toml"), "tab_size = 8\n").unwrap();
        assert_eq!(resolve(&source, FormatOptions::default(), None).unwrap().options.tab_size, 8);
    }
    #[test]
    fn invalid_style_never_silently_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(".sailfmt.toml");
        for value in [
            "style_version=2",
            "tab_size=0",
            "tab_size=\"four\"",
            "insert_spaces=1",
            "line_wdth=60",
            "broken = [",
        ] {
            std::fs::write(&path, value).unwrap();
            let error = resolve(&dir.path().join("main.sail"), FormatOptions::default(), None)
                .err()
                .unwrap();
            assert!(error.to_string().contains(".sailfmt.toml"), "{error}");
        }
    }
}
