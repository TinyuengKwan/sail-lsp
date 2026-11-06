//! Notification handler implementations.
//! Each handler is a plain `fn` pointer with signature:
//! ```ignore
//! pub(crate) fn handle_NAME(
//!     state: &mut GlobalState,
//!     params: N::Params,
//! ) -> anyhow::Result<()>
//! ```
//!
//! Dispatched via `NotificationDispatcher::on_sync_mut()` in main_loop.rs.

use lsp_types::*;

use crate::global_state::GlobalState;
use crate::mem_docs::DocumentData;

pub(crate) fn handle_did_open_text_document(
    state: &mut GlobalState,
    params: DidOpenTextDocumentParams,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;
    let text = params.text_document.text;
    if crate::manifest_editing::is_manifest(&uri) {
        let saved = uri.to_file_path().ok().and_then(|path| std::fs::read_to_string(path).ok());
        if saved.as_ref() != Some(&text) {
            state.dirty_documents.insert(uri.clone());
        } else {
            state.dirty_documents.remove(&uri);
        }
        let mut doc = DocumentData::new(params.text_document.version, text);
        doc.saved_manifest_text = saved;
        std::sync::Arc::make_mut(&mut state.manifest_texts).insert(uri, doc);
        return Ok(());
    }

    state.sync_to_salsa(&uri, &text);
    state
        .mem_docs
        .insert(uri.clone(), DocumentData::new(params.text_document.version, text.clone()));

    state.log_message(MessageType::INFO, format!("opened: {uri}"));
    Ok(())
}

pub(crate) fn handle_did_change_text_document(
    state: &mut GlobalState,
    params: DidChangeTextDocumentParams,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;
    if crate::manifest_editing::is_manifest(&uri) {
        let texts = std::sync::Arc::make_mut(&mut state.manifest_texts);
        if let Some(doc) = texts.get_mut(&uri) {
            if params.text_document.version > doc.version {
                doc.apply_changes(&params.content_changes);
                doc.version = params.text_document.version;
                if doc.saved_manifest_text.as_ref() != Some(&doc.data) {
                    state.dirty_documents.insert(uri.clone());
                } else {
                    state.dirty_documents.remove(&uri);
                }
            }
        }
        return Ok(());
    }

    if let Some(doc) = state.mem_docs.get_mut(&uri) {
        if params.text_document.version <= doc.version {
            return Ok(());
        }
        doc.apply_changes(&params.content_changes);
        doc.version = params.text_document.version;

        let text = doc.data.clone();
        if !params.content_changes.is_empty() {
            state.dirty_documents.insert(uri.clone());
        }
        state.sync_to_salsa(&uri, &text);
    }
    Ok(())
}

pub(crate) fn handle_did_close_text_document(
    state: &mut GlobalState,
    params: DidCloseTextDocumentParams,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;
    if crate::manifest_editing::is_manifest(&uri) {
        std::sync::Arc::make_mut(&mut state.manifest_texts).remove(&uri);
        state.dirty_documents.remove(&uri);
        return Ok(());
    }
    state.mem_docs.remove(&uri);
    state.dirty_documents.remove(&uri);

    // The buffer stops being authoritative here, so what salsa holds goes
    // back to the file's own text — rust-analyzer invalidates the path in
    // its loader for exactly this (`notification.rs:148`). Without it the
    // last unsaved edit stayed in the database for the rest of the session:
    // a closed file kept reporting errors from text that exists nowhere,
    // and every *other* file resolved against it too.
    //
    // After `mem_docs.remove`, so the open-document rule lets the write
    // through.
    state.restore_disk_text(&uri);

    // A closed document's cached semantic tokens describe a buffer nobody
    // is looking at. rust-analyzer removes them here (`notification.rs:145`).
    state.semantic_tokens_cache.invalidate(&uri);

    // A file the workspace does not contain has no reason to keep
    // reporting once the editor stops showing it. `clear_native_for` says
    // in its own doc that it is "used when a file is closed" and carried
    // an `#[expect(dead_code)]`: nothing cleared anything, so a scratch
    // file opened, diagnosed and closed left its errors in the problems
    // panel for the rest of the session.
    //
    // A workspace file keeps them, which is rust-analyzer's rule and the
    // useful one: errors in files you are not looking at are the reason
    // that panel exists.
    let file_id = state.vfs.file_id_for_url(&uri);
    let in_workspace =
        base_db::preprocess::project_membership(state.analysis_host.raw_database(), file_id)
            == Some(true);
    if !in_workspace {
        state.diagnostics.clear_native_for(file_id);
    }
    Ok(())
}

pub(crate) fn handle_did_save_text_document(
    state: &mut GlobalState,
    params: DidSaveTextDocumentParams,
) -> anyhow::Result<()> {
    state.dirty_documents.remove(&params.text_document.uri);
    if let Some(doc) =
        std::sync::Arc::make_mut(&mut state.manifest_texts).get_mut(&params.text_document.uri)
    {
        doc.saved_manifest_text = Some(doc.data.clone());
    }
    // On save, as rust-analyzer runs `cargo check` on save: upstream
    // re-reads whole files from disk, so running it against a buffer the
    // editor has not written would report the previous save.
    if let Ok(path) = params.text_document.uri.to_file_path() {
        state.request_flycheck(&path);
    }
    Ok(())
}

pub(crate) fn handle_did_change_watched_files(
    state: &mut GlobalState,
    params: DidChangeWatchedFilesParams,
) -> anyhow::Result<()> {
    let mut reload_scope: Option<crate::reload::ReloadScope> = None;
    let merge_scope = |current: &mut Option<crate::reload::ReloadScope>, next| {
        if let Some(current) = current {
            current.merge(next);
        } else {
            *current = Some(next);
        }
    };
    for change in &params.changes {
        let uri = &change.uri;
        let path_str = uri.path();
        let is_sail = path_str.ends_with(".sail");
        let is_project_file =
            path_str.ends_with(".sail_project") || path_str.ends_with("/sail.proj");

        match change.typ {
            FileChangeType::CREATED => {
                if is_sail {
                    merge_scope(&mut reload_scope, crate::reload::ReloadScope::All);
                } else if is_project_file {
                    // Atomic replacement can recreate an already-loaded
                    // manifest. Only genuinely new manifests can change
                    // discovery precedence for unrelated projects.
                    let known = uri
                        .to_file_path()
                        .ok()
                        .map(|path| syntax::preprocess::normalize_path(&path))
                        .filter(|path| state.projects.values().any(|p| &p.manifest == path));
                    let scope = known
                        .map(crate::reload::ReloadScope::project)
                        .unwrap_or(crate::reload::ReloadScope::All);
                    merge_scope(&mut reload_scope, scope);
                }
                if !is_sail {
                    continue;
                }
                if let Ok(path) = change.uri.to_file_path() {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        // A path can be "created" while the editor already
                        // holds it — `git checkout` of a deleted file, or a
                        // watcher that reports the first write of a buffer
                        // saved to a new name. The buffer still wins.
                        state.sync_disk_text(uri, &text);
                    }
                }
            }
            FileChangeType::CHANGED => {
                // .sail_project changes always trigger reload.
                if is_project_file {
                    if let Ok(path) = uri.to_file_path() {
                        merge_scope(&mut reload_scope, crate::reload::ReloadScope::project(path));
                    } else {
                        merge_scope(&mut reload_scope, crate::reload::ReloadScope::All);
                    }
                    continue;
                }
                if !is_sail {
                    continue;
                }
                // Skip excluded files.
                if let Ok(path) = change.uri.to_file_path() {
                    let vfs_path = base_db::VfsPath::new(paths::AbsPathBuf::assert_utf8(path));
                    if let Some((_id, vfs::FileExcluded::Yes)) = state.vfs.file_id(&vfs_path) {
                        continue;
                    }
                }
                // Open files have authoritative in-memory content from
                // didChange; disk changes would overwrite unsaved edits.
                // The rule used to be written out here and nowhere else,
                // which is how the other three disk paths came to break it.
                if let Ok(path) = change.uri.to_file_path() {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        state.sync_disk_text(uri, &text);
                    }
                }
            }
            FileChangeType::DELETED if (is_sail || is_project_file) => {
                if is_project_file {
                    if let Ok(path) = uri.to_file_path() {
                        merge_scope(&mut reload_scope, crate::reload::ReloadScope::project(path));
                    } else {
                        merge_scope(&mut reload_scope, crate::reload::ReloadScope::All);
                    }
                } else {
                    merge_scope(&mut reload_scope, crate::reload::ReloadScope::All);
                }
                if is_sail {
                    state.sync_disk_text(uri, "");
                }
            }
            _ => {}
        }
    }
    if let Some(scope) = reload_scope {
        state.request_workspace_reload("file created/deleted/project changed", scope);
    }
    Ok(())
}

pub(crate) fn handle_did_change_workspace_folders(
    state: &mut GlobalState,
    params: DidChangeWorkspaceFoldersParams,
) -> anyhow::Result<()> {
    // Workspace folder changes trigger a full reload.
    for added in &params.event.added {
        state.workspace_folders.insert(added.uri.clone());
    }
    for removed in &params.event.removed {
        state.workspace_folders.remove(&removed.uri);
    }
    state.request_workspace_reload("workspace folders changed", crate::reload::ReloadScope::All);
    Ok(())
}

pub(crate) fn handle_did_change_configuration(
    state: &mut GlobalState,
    params: DidChangeConfigurationParams,
) -> anyhow::Result<()> {
    // A client that cannot answer `workspace/configuration` has nothing to
    // pull from, and its `settings` are all there is.
    // `workspace_configuration_support` is what says which — it had no
    // caller, so the request went out either way and a client without the
    // capability would answer with an error and keep its old settings.
    if !state.client_caps.workspace_configuration_support() {
        state.apply_client_settings(&params.settings);
        return Ok(());
    }
    // Otherwise the parameters are meaningless — the protocol says so
    // (microsoft/language-server-protocol#676) and clients act on it: VS
    // Code sends `null`. Reading them, which is what this did, meant a
    // settings change reached the server as an empty config or not at all.
    // Ask for the settings instead.
    state.send_request::<lsp_types::request::WorkspaceConfiguration>(
        lsp_types::ConfigurationParams {
            items: vec![lsp_types::ConfigurationItem {
                scope_uri: None,
                section: Some("sail-lsp".to_owned()),
            }],
        },
        |state, response| {
            match (response.error, response.result) {
                (Some(err), _) => {
                    log::error!("failed to fetch the server settings: {}", err.message)
                }
                (None, Some(json)) => {
                    // One item asked for, one item back.
                    let settings = json.get(0).cloned().unwrap_or(json);
                    state.apply_client_settings(&settings);
                    state.log_message(
                        MessageType::INFO,
                        format!(
                            "Configuration updated: diagnostics={}",
                            state.config.diagnostics.enable
                        ),
                    );
                }
                (None, None) => log::error!("empty response to workspace/configuration"),
            }
        },
    );
    Ok(())
}

/// Handle `$/cancelRequest` — the client no longer wants an answer.
///
/// This used to do nothing, on the reasoning that salsa cancels in-flight
/// queries. It does — when a new revision lands. A request that is merely
/// slow is not interrupted by anything, and the client was left waiting
/// for an answer it had already given up on. The request queue knows which
/// ids are outstanding, so cancelling one answers it with
/// `RequestCancelled` as the protocol asks.
pub(crate) fn handle_cancel(
    state: &mut GlobalState,
    params: lsp_types::CancelParams,
) -> anyhow::Result<()> {
    let id: lsp_server::RequestId = match params.id {
        lsp_types::NumberOrString::Number(id) => id.into(),
        lsp_types::NumberOrString::String(id) => id.into(),
    };
    state.cancel(id);
    Ok(())
}

/// Handle `sail-lsp/reloadWorkspace` — trigger full workspace reload.
pub(crate) fn handle_reload_workspace(state: &mut GlobalState, _params: ()) -> anyhow::Result<()> {
    state.request_workspace_reload("manual reload", crate::reload::ReloadScope::All);
    state.log_message(lsp_types::MessageType::INFO, "Workspace reload requested".to_string());
    Ok(())
}

/// `window/workDoneProgress/cancel`: the user pressed the cancel button.
///
/// The server sends `window/workDoneProgress/create`, reports under the
/// token, and — until now — dropped the client's answer when it came back.
/// `$/cancelRequest` was wired for the same reason and this is its other
/// half: a progress sequence the user can stop.
///
/// The token is checked, not assumed. A client may cancel a token this
/// server no longer owns, and cancelling whatever happens to be running
/// would stop the wrong work.
pub(crate) fn handle_work_done_progress_cancel(
    state: &mut GlobalState,
    params: lsp_types::WorkDoneProgressCancelParams,
) -> anyhow::Result<()> {
    if state.cancellable_progress.as_ref() != Some(&params.token) {
        log::debug!("progress cancel for an unknown token: {:?}", params.token);
        return Ok(());
    }
    // `prime_caches` observes this and ends with `cancelled: true`, which
    // clears the slot and requeues the work.
    state.prime_caches_cancel.cancel();
    Ok(())
}

/// Explicit check request, independent of a workspace reload.
pub(crate) fn handle_recheck(state: &mut GlobalState, _: ()) -> anyhow::Result<()> {
    state.run_flycheck(None);
    Ok(())
}

pub(crate) fn handle_run_flycheck(
    state: &mut GlobalState,
    params: crate::lsp_ext::FlycheckControlParams,
) -> anyhow::Result<()> {
    state.run_flycheck(params.text_document.as_ref());
    Ok(())
}

pub(crate) fn handle_cancel_flycheck(
    state: &mut GlobalState,
    params: crate::lsp_ext::FlycheckControlParams,
) -> anyhow::Result<()> {
    state.cancel_flycheck(params.text_document.as_ref());
    Ok(())
}

pub(crate) fn handle_clear_flycheck(
    state: &mut GlobalState,
    params: crate::lsp_ext::FlycheckControlParams,
) -> anyhow::Result<()> {
    state.clear_flycheck(params.text_document.as_ref());
    Ok(())
}

#[cfg(test)]
mod manifest_status_tests {
    use super::*;

    #[test]
    fn unsaved_manifest_status_tracks_open_edit_save_and_close() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sail.proj");
        let saved = "A {}\n";
        std::fs::write(&path, saved).unwrap();
        let uri = Url::from_file_path(&path).unwrap();
        let (sender, _messages) = crossbeam_channel::unbounded();
        let (tasks, _receiver) = crossbeam_channel::unbounded();
        let mut state = GlobalState::new(sender, tasks);
        let pending = |state: &GlobalState| {
            state.current_status().message.unwrap_or_default().contains("Unsaved project manifest")
        };
        let open = |state: &mut GlobalState, text: &str| {
            handle_did_open_text_document(
                state,
                DidOpenTextDocumentParams {
                    text_document: TextDocumentItem {
                        uri: uri.clone(),
                        language_id: "sail-project".into(),
                        version: 1,
                        text: text.into(),
                    },
                },
            )
            .unwrap();
        };
        open(&mut state, saved);
        assert!(!pending(&state));
        let change = |state: &mut GlobalState, version| {
            handle_did_change_text_document(
                state,
                DidChangeTextDocumentParams {
                    text_document: VersionedTextDocumentIdentifier { uri: uri.clone(), version },
                    content_changes: vec![TextDocumentContentChangeEvent {
                        range: None,
                        range_length: None,
                        text: "B {}\n".into(),
                    }],
                },
            )
            .unwrap();
        };
        change(&mut state, 1);
        assert!(!pending(&state), "ignore stale edits");
        change(&mut state, 2);
        assert!(pending(&state));
        assert!(state.projects.is_empty(), "editing alone must not change the loaded project");
        std::fs::write(&path, "B {}\n").unwrap();
        handle_did_save_text_document(
            &mut state,
            DidSaveTextDocumentParams {
                text_document: TextDocumentIdentifier { uri: uri.clone() },
                text: None,
            },
        )
        .unwrap();
        assert!(!pending(&state));
        let close = |state: &mut GlobalState| {
            handle_did_close_text_document(
                state,
                DidCloseTextDocumentParams {
                    text_document: TextDocumentIdentifier { uri: uri.clone() },
                },
            )
            .unwrap();
        };
        close(&mut state);
        open(&mut state, "restored unsaved text");
        assert!(pending(&state), "restored dirty buffers need a warning too");
        state.project_error = Some("Invalid project".into());
        assert_eq!(state.current_status().health, crate::lsp_ext::Health::Error);
        close(&mut state);
        assert!(!pending(&state));
    }
}
