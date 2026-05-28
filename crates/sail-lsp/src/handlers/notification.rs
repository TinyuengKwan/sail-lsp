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
        for change in &params.content_changes {
            if let Some(range) = &change.range {
                let start = line_col_to_offset(&doc.data, range.start);
                let end = line_col_to_offset(&doc.data, range.end);
                doc.data.replace_range(start..end, &change.text);
            } else {
                doc.data = change.text.clone();
            }
        }
        doc.version = params.text_document.version;

        let text = doc.data.clone();
        state.sync_to_salsa(&uri, &text);
        state.index_dirty_files.push(uri);
    }
    Ok(())
}

pub(crate) fn handle_did_close_text_document(
    state: &mut GlobalState,
    params: DidCloseTextDocumentParams,
) -> anyhow::Result<()> {
    state.mem_docs.remove(&params.text_document.uri);
    Ok(())
}

pub(crate) fn handle_did_save_text_document(
    _state: &mut GlobalState,
    _params: DidSaveTextDocumentParams,
) -> anyhow::Result<()> {
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
                if let Ok(path) = change.uri.to_file_path() {
                    if let Ok(text) = std::fs::read_to_string(&path) {
                        state.sync_disk_file(uri, &text);
                    }
                }
            }
            FileChangeType::CHANGED => {
                // .sail_project changes always trigger reload.
                if is_project_file {
                    needs_reload = true;
                }
                // Skip excluded files.
                if let Ok(path) = change.uri.to_file_path() {
                    let vfs_path = base_db::VfsPath::new(paths::AbsPathBuf::assert_utf8(path));
                    if let Some((_id, vfs::FileExcluded::Yes)) = state.vfs.file_id(&vfs_path) {
                        continue;
                    }
                }
                // (main_loop.rs:932): `if !self.mem_docs.contains(&path)`.
                // Open files have authoritative in-memory content from
                // didChange; disk changes would overwrite unsaved edits.
                if !state.mem_docs.contains(uri) {
                    if let Ok(path) = change.uri.to_file_path() {
                        if let Ok(text) = std::fs::read_to_string(&path) {
                            state.sync_disk_file(uri, &text);
                            state.index_dirty_files.push(uri.clone());
                        }
                    }
                }
            }
            FileChangeType::DELETED
                if (is_sail || is_project_file) => {
                    needs_reload = true;
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
    let new_config = crate::config::SailLspConfig::from_json(&params.settings);
    // Update_configuration handles diff detection and triggers
    // workspace reload if necessary (reload.rs:92).
    state.update_configuration(new_config);
    state.log_message(
        MessageType::INFO,
        format!(
            "Configuration updated: diagnostics={}, z3_timeout={}ms",
            state.config.diagnostics.enable, state.config.z3.timeout_ms
        ),
    );
    Ok(())
}

/// Handle `$/cancelRequest` — cancel an in-flight request.
///. The actual cancellation is handled by
/// salsa's `Cancelled` mechanism — we mark the request as cancelled
/// so the response can be sent with error code `ContentModified`.
pub(crate) fn handle_cancel(
    _state: &mut GlobalState,
    _params: lsp_types::CancelParams,
) -> anyhow::Result<()> {
    // Cancellation is handled implicitly by salsa: when a new revision
    // is applied (process_changes), in-flight queries are cancelled.
    // The RequestDispatcher catches `salsa::Cancelled` and returns
    // `ErrorCode::ContentModified`.
    //
    // `req_queue.incoming.cancel(id)` to mark pending
    // requests as cancelled. We rely on salsa cancellation instead.
    Ok(())
}

/// Handle `sail-lsp/reloadWorkspace` — trigger full workspace reload.
pub(crate) fn handle_reload_workspace(state: &mut GlobalState, _params: ()) -> anyhow::Result<()> {
    state.fetch_workspaces_queue.request_op("manual reload".into(), ());
    state.log_message(lsp_types::MessageType::INFO, "Workspace reload requested".to_string());
    Ok(())
}

/// Type-safe notification check helper — used for early filtering.
/// Wraps `crate::lsp::utils::notification_is`.
#[allow(dead_code)]
pub(crate) fn is_cancel_notification(notif: &lsp_server::Notification) -> bool {
    crate::lsp::utils::notification_is::<crate::lsp_ext::CancelRequest>(notif)
}

/// Helper: convert LSP Position (line, character) to byte offset in text.
fn line_col_to_offset(text: &str, pos: Position) -> usize {
    let mut line = 0u32;
    let mut offset = 0;
    for (i, ch) in text.char_indices() {
        if line == pos.line {
            let col_offset = text[i..]
                .char_indices()
                .nth(pos.character as usize)
                .map(|(o, _)| o)
                .unwrap_or(text.len() - i);
            return i + col_offset;
        }
        if ch == '\n' {
            line += 1;
        }
        offset = i + ch.len_utf8();
    }
    offset
}
