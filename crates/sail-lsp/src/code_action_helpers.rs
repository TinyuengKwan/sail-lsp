//! LSP code action protocol helpers.
//! Moved from ide-assists to isolate lsp_types to the binary crate.

use lsp_types::CodeActionKind;

pub fn code_action_kind_allowed(
    requested: &Option<Vec<CodeActionKind>>,
    action_kind: &CodeActionKind,
) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let action = action_kind.as_str();
    requested
        .iter()
        .any(|kind| action.starts_with(kind.as_str()) || kind.as_str().starts_with(action))
}

/// The edit an unresolved code action carries until the client asks for it.
///
/// It used to be `{uri, edits}` — one file — because a `CodeAction` carried
/// one file's edits. It is the whole `WorkspaceEdit` now, so an assist that
/// touches two files survives the round trip.
pub fn lazy_code_action_data(edit: &crate::lsp::ext::SnippetWorkspaceEdit) -> serde_json::Value {
    serde_json::json!({ "workspaceEdit": edit })
}

pub fn resolve_code_action_edit_from_data(
    data: &serde_json::Value,
) -> Option<crate::lsp::ext::SnippetWorkspaceEdit> {
    serde_json::from_value(data.get("workspaceEdit")?.clone()).ok()
}

pub fn sail_source_fix_all_kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.sail")
}

pub fn default_code_action_format_options() -> ide_db::ide_types::FormatOptions {
    ide_db::ide_types::FormatOptions { tab_size: 4, insert_spaces: true, ..Default::default() }
}
