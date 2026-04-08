mod actions;
mod backend;
mod completion;
mod diagnostics;
mod formatting;
mod handlers;
mod hover;
mod inlay_hints;
mod semantic_tokens;
mod state;
mod symbols;
mod typecheck;

use backend::Backend;
use tower_lsp::{LspService, Server};

// Re-exports used by tests (via `use super::*` in tests.rs).
#[cfg(test)]
pub(crate) use actions::{
    code_action_kind_allowed, lazy_code_action_data, missing_semicolon_fix,
    quick_fix_for_diagnostic, resolve_code_action_edit_from_data, sail_source_fix_all_kind,
};
#[cfg(test)]
pub(crate) use backend::{SAIL_BUILTINS, SAIL_KEYWORDS};
#[cfg(test)]
pub(crate) use completion::{build_completion_items, completion_prefix, completion_trigger_characters};
#[cfg(test)]
pub(crate) use diagnostics::{document_diagnostic_report_for_file, workspace_diagnostic_report};
#[cfg(test)]
pub(crate) use formatting::{
    document_links_for_file, format_document_text, linked_editing_ranges_for_position,
    make_selection_range, range_format_document_edits, range_len,
};
#[cfg(test)]
pub(crate) use hover::hover_for_symbol;
#[cfg(test)]
pub(crate) use inlay_hints::inlay_hints_for_range;
#[cfg(test)]
pub(crate) use state::File;
#[cfg(test)]
pub(crate) use symbols::analysis::Parameter;
#[cfg(test)]
pub(crate) use symbols::{
    call_edges_for_file, code_lens_title, code_lenses_for_file, collect_callable_signatures,
    collect_implementation_counts, collect_reference_counts, find_call_at_position,
    find_callable_signature, function_snippet, implementation_locations, infer_binding_type,
    normalize_validated_rename, parse_named_type, reference_locations, rename_edits,
    resolve_symbol_at, resolve_workspace_symbol, signature_help_for_position,
    symbol_declaration_locations, symbol_definition_locations, symbol_spans_for_file,
    token_symbol_key, type_alias_edges, type_name_candidates_at_position, type_subtypes,
    type_supertypes, typed_bindings, will_rename_file_edits,
};
#[cfg(test)]
pub(crate) use std::collections::hash_map::HashMap;
#[cfg(test)]
pub(crate) use tower_lsp::lsp_types::{
    CodeActionKind, DocumentDiagnosticReport, DocumentDiagnosticReportResult, FormattingOptions,
    OneOf, Range, RenameFilesParams, SymbolKind, TextEdit, Url, WorkspaceDiagnosticReportResult,
    WorkspaceLocation, WorkspaceSymbol,
};

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new_with_client);
    Server::new(stdin, stdout, socket).serve(service).await;
}
