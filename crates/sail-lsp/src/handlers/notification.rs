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

    if let Some(doc) = state.mem_docs.get_mut(&uri) {
        doc.apply_changes(&params.content_changes);
        doc.version = params.text_document.version;

        let text = doc.data.clone();
        state.sync_to_salsa(&uri, &text);
    }
    Ok(())
}

pub(crate) fn handle_did_close_text_document(
    state: &mut GlobalState,
    params: DidCloseTextDocumentParams,
) -> anyhow::Result<()> {
    let uri = params.text_document.uri;
    state.mem_docs.remove(&uri);

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
    _params: DidSaveTextDocumentParams,
) -> anyhow::Result<()> {
    // On save, as rust-analyzer runs `cargo check` on save: upstream
    // re-reads whole files from disk, so running it against a buffer the
    // editor has not written would report the previous save.
    state.request_flycheck();
    Ok(())
}

pub(crate) fn handle_did_change_watched_files(
    state: &mut GlobalState,
    params: DidChangeWatchedFilesParams,
) -> anyhow::Result<()> {
    let mut needs_reload = false;
    for change in &params.changes {
        let uri = &change.uri;
        let path_str = uri.path();
        let is_sail = path_str.ends_with(".sail");
        let is_project_file =
            path_str.ends_with(".sail_project") || path_str.ends_with("/sail.proj");

        match change.typ {
            FileChangeType::CREATED => {
                if is_sail || is_project_file {
                    needs_reload = true;
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
                    needs_reload = true;
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
                needs_reload = true;
                if is_sail {
                    state.sync_disk_text(uri, "");
                }
            }
            _ => {}
        }
    }
    if needs_reload {
        state.fetch_workspaces_queue.request_op("file created/deleted/project changed".into(), ());
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
    state.fetch_workspaces_queue.request_op("workspace folders changed".into(), ());
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
        state.update_configuration(crate::config::SailLspConfig::from_json(&params.settings));
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
                    let new_config = crate::config::SailLspConfig::from_json(&settings);
                    state.update_configuration(new_config);
                    state.log_message(
                        MessageType::INFO,
                        format!(
                            "Configuration updated: diagnostics={}, z3_timeout={}ms",
                            state.config.diagnostics.enable, state.config.z3.timeout_ms
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
    state.fetch_workspaces_queue.request_op("manual reload".into(), ());
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
