//! Editor features for project manifests, and the fix that adds a file to one.
//!
//! A manifest is not Sail: it never enters the analysis database, and its
//! features read its text through `project_model::editing`. rust-analyzer
//! leaves Cargo.toml to other extensions; Sail has none, so the server
//! answers for its own manifest format.
use std::path::{Component, Path, PathBuf};

use lsp_types::{CompletionItem, CompletionItemKind, Location, Position, Range, TextEdit, Url};
use project_model::editing::{self, ManifestCompletion, ManifestSymbol};

use crate::global_state::GlobalStateSnapshot;

pub(crate) fn is_manifest(uri: &Url) -> bool {
    uri.to_file_path().is_ok_and(|path| crate::reload::is_project_file(&path))
}

/// The manifest as the editor has it, else as it is on disk.
fn text(snap: &GlobalStateSnapshot, uri: &Url) -> Option<String> {
    if let Some(text) = snap.manifest_texts.get(uri) {
        return Some(text.clone());
    }
    std::fs::read_to_string(uri.to_file_path().ok()?).ok()
}

fn project_options(snap: &GlobalStateSnapshot, uri: &Url) -> project_model::ProjectOptions {
    let mut options = snap.config.workspace.project_options();
    let path = uri.to_file_path().ok();
    let root = snap
        .projects
        .iter()
        .find(|(_, p)| Some(&p.manifest) == path.as_ref())
        .and_then(|(root, _)| root.to_file_path().ok())
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
        ManifestSymbol::File { .. } => {
            let dir = uri.to_file_path().ok()?.parent()?.to_path_buf();
            project_model::ProjectManifest::file_paths_at(
                &text,
                offset,
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

/// `to` relative to the directory `from`, with `..` where it must climb.
fn relative(from: &Path, to: &Path) -> Option<PathBuf> {
    let from: Vec<_> = from.components().collect();
    let to: Vec<_> = to.components().collect();
    let shared = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    if shared == 0 {
        return None;
    }
    let mut out: PathBuf = from[shared..].iter().map(|_| Component::ParentDir).collect();
    out.extend(&to[shared..]);
    Some(out)
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
        let Some(text) = text(snap, &manifest_uri) else { continue };
        let Ok(manifest) = project_model::ProjectManifest::parse(&text) else { continue };
        let Some(written) = relative(manifest_dir, &path) else { continue };
        let written = written.to_string_lossy().replace('\\', "/");
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
            title: title.clone(),
            kind: Some(lsp_types::CodeActionKind::QUICKFIX),
            diagnostics: Some(unlinked.to_vec()),
            edit: Some(workspace_edit(
                snap.client_caps.change_annotation_support(),
                manifest_uri,
                edit,
                title,
            )),
            is_preferred: preferred.then_some(true),
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
    edit: TextEdit,
    label: String,
) -> crate::lsp::ext::SnippetWorkspaceEdit {
    use lsp_types::{AnnotatedTextEdit, ChangeAnnotation, OneOf, TextDocumentEdit};
    if !confirm {
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
        text_document: lsp_types::OptionalVersionedTextDocumentIdentifier { uri, version: None },
        edits: vec![OneOf::Right(AnnotatedTextEdit { text_edit: edit, annotation_id: id.clone() })],
    };
    crate::lsp::ext::SnippetWorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(vec![document])),
        change_annotations: Some([(id, annotation)].into_iter().collect()),
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
        let plain = workspace_edit(false, uri.clone(), edit.clone(), "Add".into());
        assert!(plain.changes.is_some() && plain.change_annotations.is_none());
        let json = serde_json::to_value(workspace_edit(true, uri, edit, "Add".into())).unwrap();
        let id = json["documentChanges"][0]["edits"][0]["annotationId"].as_str().unwrap();
        assert_eq!(json["changeAnnotations"][id]["needsConfirmation"], true);
        assert_eq!(json["documentChanges"][0]["edits"][0]["newText"], ", x.sail");
    }

    #[test]
    fn relative_paths_climb_where_they_must() {
        let rel = |a: &str, b: &str| {
            relative(Path::new(a), Path::new(b)).map(|p| p.display().to_string())
        };
        assert_eq!(rel("/w/model", "/w/model/core/x.sail").as_deref(), Some("core/x.sail"));
        assert_eq!(rel("/w/model", "/w/extra/x.sail").as_deref(), Some("../extra/x.sail"));
    }
}
