//! Editor features for project manifests, and the fix that adds a file to one.
//!
//! A manifest is not Sail: it never enters the analysis database, and its
//! features read its text through `project_model::editing`. rust-analyzer
//! leaves Cargo.toml to other extensions; Sail has none, so the server
//! answers for its own manifest format.
use std::path::Path;

use paths::relative_path;

use lsp_types::{
    CompletionItem, CompletionItemKind, DocumentSymbol, DocumentSymbolResponse, Hover,
    HoverContents, Location, MarkupContent, MarkupKind, Position, Range, SymbolKind, TextEdit, Url,
    WorkspaceEdit,
};
use project_model::editing::{self, ManifestCompletion, ManifestSymbol};

use crate::global_state::GlobalStateSnapshot;

pub(crate) fn is_manifest(uri: &Url) -> bool {
    uri.to_file_path().is_ok_and(|path| crate::reload::is_project_file(&path))
}

/// The manifest as the editor has it, else as it is on disk.
pub(crate) fn text(snap: &GlobalStateSnapshot, uri: &Url) -> Option<String> {
    if let Some(text) = snap.manifest_texts.get(uri) {
        return Some(text.data.clone());
    }
    std::fs::read_to_string(uri.to_file_path().ok()?).ok()
}

fn offset(snap: &GlobalStateSnapshot, uri: &Url, position: Position) -> Option<(String, usize)> {
    let text = text(snap, uri)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    Some((text, crate::from_proto::offset(&index, position)))
}

pub(crate) fn project_options(
    snap: &GlobalStateSnapshot,
    uri: &Url,
) -> project_model::ProjectOptions {
    let path = uri.to_file_path().ok();
    let project = snap.projects.values().find(|p| Some(&p.manifest) == path.as_ref());
    let mut options = project.map_or(&snap.config.workspace, |p| &p.config).project_options();
    let root = project
        .map(|p| p.workspace_root.clone())
        .or_else(|| path.as_ref().and_then(|p| p.parent().map(Path::to_path_buf)));
    if let Some(root) = root {
        for path in options.virtual_files.values_mut() {
            *path = syntax::preprocess::normalize_path(&root.join(&*path));
        }
    }
    if let Some(dir) = &snap.workspace_status.sail_dir {
        options
            .virtual_files
            .entry("corelib.sail".into())
            .or_insert_with(|| dir.join("lib/core/corelib.sail"));
    }
    options
}

fn range(index: &ide_db::line_index::LineIndex, r: std::ops::Range<usize>) -> Range {
    let at = |o| crate::lsp::to_proto::position_in(index, o);
    Range::new(at(r.start), at(r.end))
}

/// A module reference goes to its header, a `$variable` to its declaration,
/// a path to the file.
pub(crate) fn goto_definition(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
) -> Option<Vec<Location>> {
    let text = text(snap, uri)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    let offset = crate::from_proto::offset(&index, position);
    let declared = |found: Vec<(String, std::ops::Range<usize>)>, name: &str| {
        found
            .into_iter()
            .filter(|(n, _)| n == name)
            .map(|(_, r)| Location::new(uri.clone(), range(&index, r)))
            .collect()
    };
    Some(match editing::symbol_at(&text, offset)? {
        ManifestSymbol::Module { name, .. } => declared(editing::module_declarations(&text), &name),
        ManifestSymbol::Variable { name, .. } => {
            declared(editing::variable_declarations(&text), &name)
        }
        ManifestSymbol::File { range, .. } => {
            let dir = uri.to_file_path().ok()?.parent()?.to_path_buf();
            project_model::ProjectManifest::file_paths_at(
                &text,
                // Symbol lookup accepts a cursor just after the path, while
                // expression ranges are half-open. Keep both APIs consistent.
                offset.min(range.end.saturating_sub(1)),
                &project_options(snap, uri),
            )
            .into_iter()
            .filter_map(|path| {
                let target = syntax::preprocess::normalize_path(&dir.join(path));
                Some(Location::new(Url::from_file_path(target).ok()?, Range::default()))
            })
            .collect()
        }
    })
}

/// Module names where a module goes, variable names after `$`.
pub(crate) fn completion(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
) -> Option<Vec<CompletionItem>> {
    let text = text(snap, uri)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    let offset = crate::from_proto::offset(&index, position);
    let (kind, replace, names) = match editing::completions_at(&text, offset)? {
        ManifestCompletion::Modules { replace, names } => {
            (CompletionItemKind::MODULE, replace, names)
        }
        ManifestCompletion::Variables { replace, names } => {
            (CompletionItemKind::VARIABLE, replace, names)
        }
        ManifestCompletion::Files { replace, prefix } => {
            let path = uri.to_file_path().ok()?;
            let dir = path.parent()?;
            let (parent, needle) = prefix.rsplit_once('/').unwrap_or(("", prefix.as_str()));
            let search = syntax::preprocess::normalize_path(&dir.join(parent));
            let quoted = replace.start > 0 && text.as_bytes().get(replace.start - 1) == Some(&b'"');
            let mut names = Vec::new();
            for entry in std::fs::read_dir(search).ok()?.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if !name.starts_with(needle) || name.starts_with('.') {
                    continue;
                }
                let Ok(kind) = entry.file_type() else { continue };
                if !kind.is_dir()
                    && entry.path().extension().and_then(|e| e.to_str()) != Some("sail")
                {
                    continue;
                }
                let candidate = if parent.is_empty() { name } else { format!("{parent}/{name}") };
                let candidate = if kind.is_dir() { format!("{candidate}/") } else { candidate };
                names.push(if quoted || kind.is_dir() {
                    candidate
                } else {
                    editing::path_literal(&candidate)
                });
            }
            names.sort();
            (CompletionItemKind::FILE, replace, names)
        }
    };
    let replace = range(&index, replace);
    Some(
        names
            .into_iter()
            .map(|name| CompletionItem {
                label: name.clone(),
                kind: Some(kind),
                text_edit: Some(lsp_types::CompletionTextEdit::Edit(TextEdit::new(replace, name))),
                ..Default::default()
            })
            .collect(),
    )
}

pub(crate) fn hover(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
    markdown: bool,
) -> Option<Hover> {
    let (text, offset) = offset(snap, uri, position)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    let symbol = editing::symbol_at(&text, offset)
        .or_else(|| editing::references_at(&text, offset).map(|references| references.symbol))?;
    let (value, span) = match symbol {
        ManifestSymbol::Module { name, range: span } => {
            let project = snap
                .projects
                .values()
                .find(|project| Url::from_file_path(&project.manifest).ok().as_ref() == Some(uri));
            let detail = project
                .and_then(|project| project.resolved.as_ref())
                .and_then(|resolved| resolved.modules.iter().find(|module| module.name == name))
                .map(|module| {
                    format!(
                        "\n\n{} source file(s), {} direct requirement(s)",
                        module.files.len(),
                        module.requires.len()
                    )
                })
                .unwrap_or_default();
            (format!("Sail project module `{name}`{detail}"), span)
        }
        ManifestSymbol::Variable { name, range: span } => {
            (format!("Sail project variable `${name}`"), span)
        }
        ManifestSymbol::File { path, range: span } => {
            let manifest = uri.to_file_path().ok()?;
            let target = syntax::preprocess::normalize_path(&manifest.parent()?.join(&path));
            let state = if target.is_file() { "exists" } else { "does not exist" };
            (format!("Project source `{}`\n\n{state}", target.display()), span)
        }
    };
    Some(Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: if markdown { MarkupKind::Markdown } else { MarkupKind::PlainText },
            value,
        }),
        range: Some(range(&index, span)),
    })
}

pub(crate) fn references(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
    include_declaration: bool,
) -> Option<Vec<Location>> {
    let (text, offset) = offset(snap, uri, position)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    let references = editing::references_at(&text, offset)?;
    let locations = references
        .occurrences
        .into_iter()
        .filter(|span| include_declaration || *span != references.declaration)
        .map(|span| Location::new(uri.clone(), range(&index, span)))
        .collect();
    Some(locations)
}

pub(crate) fn prepare_rename(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
) -> Option<(Range, String)> {
    let (text, offset) = offset(snap, uri, position)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    let references = editing::references_at(&text, offset)?;
    let name = match references.symbol {
        ManifestSymbol::Module { name, .. } | ManifestSymbol::Variable { name, .. } => name,
        ManifestSymbol::File { .. } => return None,
    };
    let occurrence = references
        .occurrences
        .into_iter()
        .find(|span| span.start <= offset && offset <= span.end)
        .unwrap_or(references.declaration);
    Some((range(&index, occurrence), name))
}

fn valid_name(name: &str) -> bool {
    name.chars().next().is_some_and(|ch| ch == '_' || ch.is_ascii_alphabetic())
        && name.chars().all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

pub(crate) fn rename(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    position: Position,
    new_name: &str,
) -> anyhow::Result<Option<WorkspaceEdit>> {
    if !valid_name(new_name) {
        anyhow::bail!("A project module or variable name must be an identifier");
    }
    let (text, offset) = match offset(snap, uri, position) {
        Some(value) => value,
        None => return Ok(None),
    };
    let index = ide_db::line_index::LineIndex::new(&text);
    let Some(references) = editing::references_at(&text, offset) else { return Ok(None) };
    let edits = references
        .occurrences
        .into_iter()
        .map(|span| TextEdit::new(range(&index, span), new_name.to_owned()))
        .collect();
    Ok(Some(WorkspaceEdit {
        changes: Some([(uri.clone(), edits)].into_iter().collect()),
        ..Default::default()
    }))
}

pub(crate) fn document_symbols(
    snap: &GlobalStateSnapshot,
    uri: &Url,
) -> Option<DocumentSymbolResponse> {
    let text = text(snap, uri)?;
    let index = ide_db::line_index::LineIndex::new(&text);
    #[allow(deprecated)]
    let mut symbols: Vec<DocumentSymbol> = editing::variable_declarations(&text)
        .into_iter()
        .map(|(name, span)| DocumentSymbol {
            name,
            detail: Some("project variable".into()),
            kind: SymbolKind::VARIABLE,
            tags: None,
            deprecated: None,
            range: range(&index, span.clone()),
            selection_range: range(&index, span),
            children: None,
        })
        .collect();
    #[allow(deprecated)]
    symbols.extend(editing::module_declarations(&text).into_iter().map(|(name, span)| {
        DocumentSymbol {
            name,
            detail: Some("project module".into()),
            kind: SymbolKind::MODULE,
            tags: None,
            deprecated: None,
            range: range(&index, span.clone()),
            selection_range: range(&index, span),
            children: None,
        }
    }));
    Some(DocumentSymbolResponse::Nested(symbols))
}

pub(crate) fn formatting(snap: &GlobalStateSnapshot, uri: &Url) -> Option<Vec<TextEdit>> {
    let text = text(snap, uri)?;
    let formatted = editing::format(&text);
    if formatted == text {
        return Some(Vec::new());
    }
    let index = ide_db::line_index::LineIndex::new(&text);
    Some(vec![TextEdit::new(
        Range::new(Position::new(0, 0), crate::lsp::to_proto::position_in(&index, text.len())),
        formatted,
    )])
}

/// For a file no project lists: add it to a module. Each module that already
/// lists files nearest the new one's directory is offered; when only one is,
/// it is the preferred fix. The edit appends to the module's last `files`
/// entry, outside any conditional, so the file loads last in that module.
pub(crate) fn add_to_project(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    unlinked: &[lsp_types::Diagnostic],
) -> Vec<crate::lsp::ext::CodeAction> {
    let Ok(path) = uri.to_file_path() else { return Vec::new() };
    if unlinked.is_empty() || !crate::reload::is_sail_file(&path) {
        return Vec::new();
    }
    let path = syntax::preprocess::normalize_path(&path);
    let dir = path.parent().unwrap_or(&path);
    let inside: Vec<_> = snap
        .projects
        .values()
        .filter(|p| p.manifest.parent().is_some_and(|d| path.starts_with(d)))
        .collect();
    let projects = if inside.is_empty() { snap.projects.values().collect() } else { inside };

    // How close a module's files are to the new one: shared directory depth.
    let mut candidates = Vec::new();
    for project in projects {
        let Some(manifest_dir) = project.manifest.parent() else { continue };
        let Ok(manifest_uri) = Url::from_file_path(&project.manifest) else { continue };
        let Some(source) = text(snap, &manifest_uri) else { continue };
        let Ok(manifest) = project_model::ProjectManifest::parse(&source) else { continue };
        let Ok(resolved) = manifest.resolve(&project_options(snap, &manifest_uri)) else {
            continue;
        };
        for &id in &resolved.module_order {
            let module = &resolved.modules[id];
            let closeness = module
                .files
                .iter()
                .map(|f| syntax::preprocess::normalize_path(&manifest_dir.join(f)))
                .map(|f| {
                    let parent = f.parent().map(Path::to_path_buf).unwrap_or_default();
                    parent.components().zip(dir.components()).take_while(|(a, b)| a == b).count()
                })
                .max()
                .unwrap_or(0);
            candidates.push((closeness, project, manifest_dir, module.name.clone()));
        }
    }
    let best = candidates.iter().map(|c| c.0).max().unwrap_or(0);
    candidates.retain(|c| c.0 == best);
    candidates.truncate(20);
    let preferred = candidates.len() == 1;

    let mut actions = Vec::new();
    for (_, project, manifest_dir, module) in candidates {
        let Ok(manifest_uri) = Url::from_file_path(&project.manifest) else { continue };
        let version = snap.document_versions.get(&manifest_uri).copied();
        if version.is_some() && !snap.client_caps.versioned_edit_support() {
            continue;
        }
        let Some(text) = text(snap, &manifest_uri) else { continue };
        let Ok(manifest) = project_model::ProjectManifest::parse(&text) else { continue };
        let Some(written) = relative_path(manifest_dir, &path) else { continue };
        let Some(written) = paths::slash_path(&written) else { continue };
        let literal = editing::path_literal(&written);
        let Some((at, insert)) = manifest.file_insertion(&text, &module, &literal) else {
            continue;
        };
        let index = ide_db::line_index::LineIndex::new(&text);
        let edit = TextEdit::new(range(&index, at..at), insert);
        let manifest_name =
            project.manifest.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
        let title = format!("Add `{written}` to module `{module}` in {manifest_name}");
        actions.push(crate::lsp::ext::CodeAction {
            command: None,
            title: title.clone(),
            kind: Some(lsp_types::CodeActionKind::QUICKFIX),
            diagnostics: Some(unlinked.to_vec()),
            edit: Some(workspace_edit(
                snap.client_caps.change_annotation_support(),
                manifest_uri,
                version,
                edit,
                title,
            )),
            is_preferred: preferred.then_some(true),
            data: None,
        });
    }
    actions
}

/// Repairs for diagnostics produced while evaluating a project manifest.
/// These stay eager: the edits are tiny and depend on the unsaved manifest
/// text that the request already names.
pub(crate) fn diagnostic_fixes(
    snap: &GlobalStateSnapshot,
    uri: &Url,
    diagnostics: &[lsp_types::Diagnostic],
) -> Vec<crate::lsp::ext::CodeAction> {
    let Some(text) = text(snap, uri) else { return Vec::new() };
    let index = ide_db::line_index::LineIndex::new(&text);
    let mut actions = Vec::new();
    for diagnostic in diagnostics {
        let Some(lsp_types::NumberOrString::String(code)) = diagnostic.code.as_ref() else {
            continue;
        };
        let byte_range = crate::from_proto::text_range(&index, diagnostic.range);
        let start = base_db::range_start(byte_range);
        let symbol = editing::symbol_at(&text, start)
            .or_else(|| editing::references_at(&text, start).map(|references| references.symbol));
        let (title, edit) = match (code.as_str(), symbol) {
            (
                "unknown-module" | "unknown-root-module",
                Some(ManifestSymbol::Module { name, .. }),
            ) if valid_name(&name) => {
                let at = text.len();
                let prefix = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
                let edit = TextEdit::new(
                    Range::new(
                        crate::lsp::to_proto::position_in(&index, at),
                        crate::lsp::to_proto::position_in(&index, at),
                    ),
                    format!("{prefix}{name} {{\n  files []\n}}\n"),
                );
                (format!("Declare project module `{name}`"), Some(edit))
            }
            ("unknown-variable", Some(ManifestSymbol::Variable { name, .. }))
                if valid_name(&name) =>
            {
                let edit = TextEdit::new(
                    Range::new(Position::new(0, 0), Position::new(0, 0)),
                    format!("variable {name} = false\n"),
                );
                (format!("Declare project variable `{name}`"), Some(edit))
            }
            ("missing-file", Some(ManifestSymbol::File { path, .. }))
                if snap.client_caps.file_creation_support() =>
            {
                let Ok(manifest) = uri.to_file_path() else { continue };
                let Some(parent) = manifest.parent() else { continue };
                let target = syntax::preprocess::normalize_path(&parent.join(&path));
                if target.exists() || !target.parent().is_some_and(std::path::Path::is_dir) {
                    continue;
                }
                let Ok(target) = Url::from_file_path(target) else { continue };
                let title = format!("Create missing project source `{path}`");
                actions.push(crate::lsp::ext::CodeAction {
                    command: None,
                    title,
                    kind: Some(lsp_types::CodeActionKind::QUICKFIX),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: Some(crate::lsp::ext::SnippetWorkspaceEdit {
                        document_changes: Some(lsp_types::DocumentChanges::Operations(vec![
                            lsp_types::DocumentChangeOperation::Op(lsp_types::ResourceOp::Create(
                                lsp_types::CreateFile {
                                    uri: target,
                                    options: Some(lsp_types::CreateFileOptions {
                                        overwrite: Some(false),
                                        ignore_if_exists: Some(true),
                                    }),
                                    annotation_id: None,
                                },
                            )),
                        ])),
                        ..Default::default()
                    }),
                    is_preferred: Some(true),
                    data: None,
                });
                continue;
            }
            _ => continue,
        };
        let Some(edit) = edit else { continue };
        actions.push(crate::lsp::ext::CodeAction {
            command: None,
            title,
            kind: Some(lsp_types::CodeActionKind::QUICKFIX),
            diagnostics: Some(vec![diagnostic.clone()]),
            edit: Some(workspace_edit(
                snap.client_caps.change_annotation_support(),
                uri.clone(),
                snap.document_versions.get(uri).copied(),
                edit,
                "Changes the Sail project manifest".into(),
            )),
            is_preferred: Some(true),
            data: None,
        });
    }
    actions
}

/// A manifest edit is a change to the build: a client that can asks the
/// user to confirm it, showing the edit, before applying it.
fn workspace_edit(
    confirm: bool,
    uri: Url,
    version: Option<i32>,
    edit: TextEdit,
    label: String,
) -> crate::lsp::ext::SnippetWorkspaceEdit {
    use lsp_types::{AnnotatedTextEdit, ChangeAnnotation, OneOf, TextDocumentEdit};
    if !confirm && version.is_none() {
        let edit = crate::lsp::ext::SnippetTextEdit {
            range: edit.range,
            new_text: edit.new_text,
            insert_text_format: None,
        };
        return crate::lsp::ext::SnippetWorkspaceEdit {
            changes: Some([(uri, vec![edit])].into_iter().collect()),
            ..Default::default()
        };
    }
    let id = "sail-lsp.addToProject".to_owned();
    let annotation = ChangeAnnotation {
        label,
        needs_confirmation: Some(true),
        description: Some("Changes which files the Sail project loads".to_owned()),
    };
    let document = TextDocumentEdit {
        text_document: lsp_types::OptionalVersionedTextDocumentIdentifier { uri, version },
        edits: vec![if confirm {
            OneOf::Right(AnnotatedTextEdit { text_edit: edit, annotation_id: id.clone() })
        } else {
            OneOf::Left(edit)
        }],
    };
    crate::lsp::ext::SnippetWorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(vec![document])),
        change_annotations: confirm.then(|| [(id, annotation)].into_iter().collect()),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A client that can confirm an edit is asked to: the manifest edit is
    /// annotated as needing confirmation, which VS Code shows as a preview.
    #[test]
    fn a_manifest_edit_asks_for_confirmation_where_it_can() {
        let uri = Url::parse("file:///w/sail.proj").unwrap();
        let edit = TextEdit::new(Range::default(), ", x.sail".into());
        let plain = workspace_edit(false, uri.clone(), None, edit.clone(), "Add".into());
        assert!(plain.changes.is_some() && plain.change_annotations.is_none());
        let json =
            serde_json::to_value(workspace_edit(true, uri, Some(7), edit, "Add".into())).unwrap();
        assert_eq!(json["documentChanges"][0]["textDocument"]["version"], 7);
        let id = json["documentChanges"][0]["edits"][0]["annotationId"].as_str().unwrap();
        assert_eq!(json["changeAnnotations"][id]["needsConfirmation"], true);
        assert_eq!(json["documentChanges"][0]["edits"][0]["newText"], ", x.sail");
    }

    #[test]
    fn relative_paths_climb_where_they_must() {
        let rel = |a: &str, b: &str| {
            relative_path(Path::new(a), Path::new(b)).and_then(|p| paths::slash_path(&p))
        };
        assert_eq!(rel("/w/model", "/w/model/core/x.sail").as_deref(), Some("core/x.sail"));
        assert_eq!(rel("/w/model", "/w/extra/x.sail").as_deref(), Some("../extra/x.sail"));
    }
}
