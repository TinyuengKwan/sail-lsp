//! LSP code action protocol helpers.
//! Moved from ide-assists to isolate lsp_types to the binary crate.

use lsp_types::{CodeActionKind, TextEdit, Url};

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

pub fn lazy_code_action_data(uri: &Url, edits: &[TextEdit]) -> serde_json::Value {
    serde_json::json!({
        "uri": uri.as_str(),
        "edits": edits,
    })
}

pub fn resolve_code_action_edit_from_data(
    data: &serde_json::Value,
) -> Option<(Url, Vec<TextEdit>)> {
    let uri = data.get("uri")?.as_str().and_then(|s| Url::parse(s).ok())?;
    let edits_value = data.get("edits")?.clone();
    let edits = serde_json::from_value::<Vec<TextEdit>>(edits_value).ok()?;
    Some((uri, edits))
}

pub fn sail_source_fix_all_kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.sail")
}

pub fn default_code_action_format_options() -> ide_db::ide_types::FormatOptions {
    ide_db::ide_types::FormatOptions { tab_size: 4, insert_spaces: true, ..Default::default() }
}
