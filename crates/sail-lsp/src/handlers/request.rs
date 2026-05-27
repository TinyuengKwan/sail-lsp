//! LSP request handler implementations.
//! Each handler is a plain `fn` pointer with signature:
//! ```ignore
//! pub(crate) fn handle_NAME(
//!     snap: GlobalStateSnapshot,
//!     params: R::Params,
//! ) -> anyhow::Result<R::Result>
//! ```
//!
//! Dispatched via `RequestDispatcher::on_sync()` or `on()` in main_loop.rs.

use std::collections::HashMap;

use lsp_types::request::{
    GotoDeclarationParams, GotoDeclarationResponse, GotoImplementationParams,
    GotoImplementationResponse, GotoTypeDefinitionParams, GotoTypeDefinitionResponse,
};
use lsp_types::*;

use hir_def::callgraph::SourceFileInfo;
use ide_db::FileDb;

use crate::global_state::GlobalStateSnapshot;

/// Convert internal IdeTextEdits to LSP TextEdits.
fn ide_edits_to_lsp(
    edits: Vec<ide_db::ide_types::IdeTextEdit>,
    file: &dyn FileDb,
) -> Vec<TextEdit> {
    let line_index = ide_db::line_index::LineIndex::new(file.text());
    edits.into_iter().map(|e| crate::to_proto::text_edit(&line_index, &e)).collect()
}

/// Convert internal FileLocations to LSP Locations.
fn file_locations_to_lsp(
    locs: &[ide_db::ide_types::FileLocation],
    analysis: &ide::analysis::Analysis,
) -> Vec<Location> {
    locs.iter()
        .filter_map(|fl| {
            let sf = analysis.file(&fl.url)?;
            let line_index = ide_db::line_index::LineIndex::new(sf.text());
            Some(Location::new(fl.url.clone(), crate::to_proto::range(&line_index, fl.range)))
        })
        .collect()
}

/// Convert `NavigationTarget`s to LSP `Location`s.
fn nav_targets_to_lsp(
    targets: &[ide_db::ide_types::NavigationTarget],
    analysis: &ide::analysis::Analysis,
) -> Vec<Location> {
    targets
        .iter()
        .filter_map(|t| {
            let sf = analysis.file(&t.url)?;
            let line_index = ide_db::line_index::LineIndex::new(sf.text());
            Some(Location::new(t.url.clone(), crate::to_proto::range(&line_index, t.focus_range)))
        })
        .collect()
}

pub(crate) fn handle_document_symbol(
    snap: GlobalStateSnapshot,
    params: DocumentSymbolParams,
) -> anyhow::Result<Option<DocumentSymbolResponse>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let targets = match snap.analysis.document_symbols(file_id) {
        Ok(targets) => targets,
        Err(_) => return Ok(None),
    };
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let tree: Vec<DocumentSymbol> = targets
        .iter()
        .map(|t: &ide_db::ide_types::NavigationTarget| {
            crate::to_proto::document_symbol(&line_index, t)
        })
        .collect();
    Ok(Some(DocumentSymbolResponse::Nested(tree)))
}

pub(crate) fn handle_selection_range(
    snap: GlobalStateSnapshot,
    params: SelectionRangeParams,
) -> anyhow::Result<Option<Vec<SelectionRange>>> {
    let uri = &params.text_document.uri;
    let result = snap.analysis.with_file(uri, |sf, _| {
        let line_index = ide_db::line_index::LineIndex::new(sf.text());
        params
            .positions
            .iter()
            .map(|pos| {
                let lc = crate::lsp_ext::line_col_from_position(*pos);
                let ide_sr = ide::formatting::make_selection_range(sf, lc);
                crate::to_proto::selection_range(&line_index, &ide_sr)
            })
            .collect::<Vec<_>>()
    });
    Ok(result)
}

pub(crate) fn handle_formatting(
    snap: GlobalStateSnapshot,
    params: DocumentFormattingParams,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    // Cache the client's format options so code actions can reuse them.
    {
        let opts = crate::from_proto::format_options(&params.options);
        if let Ok(mut cached) = snap.last_format_options.lock() {
            *cached = opts;
        }
    }
    let edits = snap
        .analysis
        .format_document(file_id, params.options.tab_size, params.options.insert_spaces)
        .ok()
        .flatten();
    let Some(edits) = edits else { return Ok(None) };
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    Ok(Some(ide_edits_to_lsp(edits, &sf)))
}

pub(crate) fn handle_semantic_tokens_range(
    snap: GlobalStateSnapshot,
    params: SemanticTokensRangeParams,
) -> anyhow::Result<Option<SemanticTokensRangeResult>> {
    let uri = &params.text_document.uri;
    let result = snap.analysis.with_file(uri, |sf, _| {
        let line_index = ide_db::line_index::LineIndex::new(sf.text());
        let text_range = crate::from_proto::text_range(&line_index, params.range);
        let ide_tokens = ide::syntax_highlighting::compute_semantic_tokens_range(sf, &text_range);
        SemanticTokensRangeResult::Tokens(crate::to_proto::semantic_tokens(&ide_tokens))
    });
    Ok(result)
}

/// Document highlight — highlight related symbols and keywords.
/// `highlight_related()`.
/// `ReferenceCategory` (Read/Write/Keyword). This enables:
/// - Read vs Write distinction in highlight kind
/// - Control-flow keyword pairing (match↔=>, if↔else, try↔catch)
/// - Exit point highlighting (return/throw)
pub(crate) fn handle_document_highlight(
    snap: GlobalStateSnapshot,
    params: DocumentHighlightParams,
) -> anyhow::Result<Option<Vec<DocumentHighlight>>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);

    let config = ide::highlight_related::HighlightRelatedConfig {
        references: true,
        exit_points: true,
        break_points: true,
        branch_exit_points: true,
    };
    let ranges = ide::highlight_related::highlight_related(&sf, &config, lc);
    if ranges.is_empty() {
        return Ok(None);
    }

    let highlights: Vec<DocumentHighlight> = ranges
        .into_iter()
        .map(|hr| {
            let kind = match hr.category {
                ide::highlight_related::ReferenceCategory::Write => DocumentHighlightKind::WRITE,
                ide::highlight_related::ReferenceCategory::Read => DocumentHighlightKind::READ,
                ide::highlight_related::ReferenceCategory::Keyword => DocumentHighlightKind::TEXT,
            };
            DocumentHighlight {
                range: Range::new(
                    crate::lsp_ext::position_from_line_col(
                        sf.position_at(base_db::range_start(hr.range)),
                    ),
                    crate::lsp_ext::position_from_line_col(
                        sf.position_at(base_db::range_end(hr.range)),
                    ),
                ),
                kind: Some(kind),
            }
        })
        .collect();
    Ok(Some(highlights))
}

pub(crate) fn handle_semantic_tokens_full(
    snap: GlobalStateSnapshot,
    params: SemanticTokensParams,
) -> anyhow::Result<Option<SemanticTokensResult>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let Some(ide_tokens) = snap.analysis.semantic_tokens(file_id).ok() else {
        return Ok(None);
    };

    // Convert to LSP tokens and cache for delta requests.
    let lsp_tokens: Vec<lsp_types::SemanticToken> =
        ide_tokens.data.iter().map(crate::to_proto::semantic_token).collect();
    let cached = snap.semantic_tokens_cache.cache_full(uri, lsp_tokens);
    Ok(Some(SemanticTokensResult::Tokens(cached)))
}

/// Pull diagnostics for a single document.
/// Includes `related_documents` for files that $include this file.
/// When `types.sail` changes, dependents like `main.sail` may get new
/// type errors — those appear in related_documents so the client can
/// update them without a separate request.
pub(crate) fn handle_document_diagnostic(
    snap: GlobalStateSnapshot,
    params: DocumentDiagnosticParams,
) -> anyhow::Result<DocumentDiagnosticReportResult> {
    let uri = &params.text_document.uri;
    let empty_report = || -> DocumentDiagnosticReportResult {
        DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
            related_documents: None,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: None,
                items: vec![],
            },
        })
        .into()
    };

    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(empty_report());
    };
    let diag_config = snap.config.diagnostics.to_ide_config();

    // Main file diagnostics
    let ide_diags = snap.analysis.file_diagnostics(file_id, &diag_config);
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(empty_report());
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let items: Vec<Diagnostic> =
        ide_diags.iter().map(|d| crate::to_proto::diagnostic(&line_index, d)).collect();

    // Related documents — files that $include this file.
    // When this file's types change, dependent files may get new errors.
    let supports_related = snap.client_caps.text_document_diagnostic_related_document_support();
    let mut related_documents: std::collections::HashMap<Url, DocumentDiagnosticReportKind> =
        std::collections::HashMap::new();

    if supports_related {
        for &dep_fid in snap.include_graph.included_by(file_id) {
            if let Some(dep_url) = snap.analysis.url_for_file_id(dep_fid) {
                let dep_diags = snap.analysis.file_diagnostics(dep_fid, &diag_config);
                if let Some(dep_sf) = snap.analysis.file(dep_url) {
                    let dep_li = ide_db::line_index::LineIndex::new(dep_sf.text());
                    let dep_items: Vec<Diagnostic> =
                        dep_diags.iter().map(|d| crate::to_proto::diagnostic(&dep_li, d)).collect();
                    related_documents.insert(
                        dep_url.clone(),
                        DocumentDiagnosticReportKind::Full(FullDocumentDiagnosticReport {
                            result_id: Some("sail-lsp".to_owned()),
                            items: dep_items,
                        }),
                    );
                }
            }
        }
    }

    let result_id = {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        items.len().hash(&mut hasher);
        format!("{:x}", hasher.finish())
    };

    let related = if related_documents.is_empty() { None } else { Some(related_documents) };

    if params.previous_result_id.as_deref() == Some(result_id.as_str()) {
        Ok(DocumentDiagnosticReport::Unchanged(RelatedUnchangedDocumentDiagnosticReport {
            related_documents: related,
            unchanged_document_diagnostic_report: UnchangedDocumentDiagnosticReport { result_id },
        })
        .into())
    } else {
        Ok(DocumentDiagnosticReport::Full(RelatedFullDocumentDiagnosticReport {
            related_documents: related,
            full_document_diagnostic_report: FullDocumentDiagnosticReport {
                result_id: Some(result_id),
                items,
            },
        })
        .into())
    }
}

pub(crate) fn handle_range_formatting(
    snap: GlobalStateSnapshot,
    params: DocumentRangeFormattingParams,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let range =
        crate::from_proto::text_range(&ide_db::line_index::LineIndex::new(sf.text()), params.range);
    let edits = ide::formatting::range_format_document_edits(
        &sf,
        range,
        &crate::from_proto::format_options(&params.options),
    );
    match edits {
        Some(edits) => Ok(Some(ide_edits_to_lsp(edits, &sf))),
        None => Ok(None),
    }
}

pub(crate) fn handle_linked_editing_range(
    snap: GlobalStateSnapshot,
    params: LinkedEditingRangeParams,
) -> anyhow::Result<Option<LinkedEditingRanges>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some(ide_result) = ide::formatting::linked_editing_ranges_for_position(&sf, lc) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(crate::to_proto::linked_editing_ranges(&line_index, &ide_result)))
}

pub(crate) fn handle_document_link(
    snap: GlobalStateSnapshot,
    params: DocumentLinkParams,
) -> anyhow::Result<Option<Vec<DocumentLink>>> {
    let uri = params.text_document.uri;
    let Some(sf) = snap.analysis.file(&uri) else {
        return Ok(None);
    };
    let ide_links = ide::formatting::document_links_for_file(&uri, &sf);
    if ide_links.is_empty() {
        return Ok(None);
    }
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let lsp_links: Vec<DocumentLink> =
        ide_links.iter().map(|l| crate::to_proto::document_link(&line_index, l)).collect();
    Ok(Some(lsp_links))
}

pub(crate) fn handle_document_link_resolve(
    _snap: GlobalStateSnapshot,
    mut params: DocumentLink,
) -> anyhow::Result<DocumentLink> {
    if params.target.is_none() {
        if let Some(target_str) =
            params.data.as_ref().and_then(|v| v.get("target")).and_then(|v| v.as_str())
        {
            params.target = Url::parse(target_str).ok();
        }
    }
    Ok(params)
}

pub(crate) fn handle_prepare_rename(
    snap: GlobalStateSnapshot,
    params: TextDocumentPositionParams,
) -> anyhow::Result<Option<PrepareRenameResponse>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(params.position);
    let Some((token, span)) = sf.token_at(lc) else {
        return Ok(None);
    };
    match token {
        parser::Token::Id(name) => Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: Range::new(
                crate::lsp_ext::position_from_line_col(sf.position_at(span.start)),
                crate::lsp_ext::position_from_line_col(sf.position_at(span.end)),
            ),
            placeholder: name.clone(),
        })),
        parser::Token::TyVal(name) => Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
            range: Range::new(
                crate::lsp_ext::position_from_line_col(sf.position_at(span.start)),
                crate::lsp_ext::position_from_line_col(sf.position_at(span.end)),
            ),
            placeholder: format!("'{}", name),
        })),
        _ => Ok(None),
    }
}

pub(crate) fn handle_semantic_tokens_full_delta(
    snap: GlobalStateSnapshot,
    params: SemanticTokensDeltaParams,
) -> anyhow::Result<Option<SemanticTokensFullDeltaResult>> {
    let uri = &params.text_document.uri;
    let previous_result_id = &params.previous_result_id;

    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let Some(ide_tokens) = snap.analysis.semantic_tokens(file_id).ok() else {
        return Ok(None);
    };

    // Convert to LSP tokens.
    let new_tokens: Vec<lsp_types::SemanticToken> =
        ide_tokens.data.iter().map(crate::to_proto::semantic_token).collect();

    // Try to compute delta from cached tokens.
    if let Some(delta) =
        snap.semantic_tokens_cache.compute_delta(uri, previous_result_id, new_tokens.clone())
    {
        return Ok(Some(SemanticTokensFullDeltaResult::TokensDelta(delta)));
    }

    // Fallback: return full tokens (and cache them).
    let cached = snap.semantic_tokens_cache.cache_full(uri, new_tokens);
    Ok(Some(SemanticTokensFullDeltaResult::Tokens(cached)))
}

pub(crate) fn handle_folding_range(
    snap: GlobalStateSnapshot,
    params: FoldingRangeParams,
) -> anyhow::Result<Option<Vec<FoldingRange>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let ide_folds = ide::folding_ranges::folding_ranges(&sf);
    if ide_folds.is_empty() {
        return Ok(None);
    }
    let ranges: Vec<FoldingRange> = ide_folds
        .into_iter()
        .map(|f| {
            let kind = match f.kind {
                ide::folding_ranges::FoldKind::Comment => Some(FoldingRangeKind::Comment),
                ide::folding_ranges::FoldKind::Imports => Some(FoldingRangeKind::Imports),
                ide::folding_ranges::FoldKind::Region => Some(FoldingRangeKind::Region),
                _ => Some(FoldingRangeKind::Region),
            };
            FoldingRange {
                start_line: f.start_line,
                start_character: None,
                end_line: f.end_line,
                end_character: None,
                kind,
                collapsed_text: None,
            }
        })
        .collect();
    Ok(Some(ranges))
}

/// Resolve a code lens — fill in the command with actual data.
/// into commands like `showReferences` / `showImplementations`.
/// Uses `editor.action.showReferences` — a standard LSP command
/// supported by VS Code, neovim, and other LSP clients.
pub(crate) fn handle_code_lens_resolve(
    _snap: GlobalStateSnapshot,
    mut params: CodeLens,
) -> anyhow::Result<CodeLens> {
    if let Some(data) = params.data.as_ref() {
        if let Some(title) = ide::annotations::code_lens_title(data) {
            let kind = data.get("kind").and_then(|v| v.as_str()).unwrap_or("");
            let name = data.get("name").and_then(|v| v.as_str()).unwrap_or("");

            // Try to build a showReferences command with the lens position.
            // This requires resolving the symbol's URI + position.
            let command = if !name.is_empty() && (kind == "refs" || kind == "impls") {
                // The code lens range start is the position to show references for.
                // The URI comes from the original request (stored in data or inferred).
                // For now, use the position from the lens range itself.
                let uri = data
                    .get("uri")
                    .and_then(|v| v.as_str())
                    .and_then(|s| lsp_types::Url::parse(s).ok());

                if let Some(uri) = uri {
                    // Full showReferences: uri, position, locations
                    Command {
                        title,
                        command: "editor.action.showReferences".to_string(),
                        arguments: Some(vec![
                            serde_json::to_value(&uri).unwrap(),
                            serde_json::to_value(params.range.start).unwrap(),
                            serde_json::to_value::<Vec<lsp_types::Location>>(vec![]).unwrap(),
                        ]),
                    }
                } else {
                    // Fallback: display-only (no URI available for command)
                    Command { title, command: String::new(), arguments: None }
                }
            } else if kind == "runnable" {
                // Runnable lens: display-only title for now.
                // A future integration could wire this to a Sail test runner
                // command (e.g. `sail.runTest` / `sail.runMain`).
                Command { title, command: String::new(), arguments: None }
            } else {
                Command { title, command: String::new(), arguments: None }
            };
            params.command = Some(command);
        }
    }
    Ok(params)
}

pub(crate) fn handle_on_type_formatting(
    snap: GlobalStateSnapshot,
    params: DocumentOnTypeFormattingParams,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);
    let result: Option<Vec<ide_db::ide_types::IdeTextEdit>> = match params.ch.as_str() {
        "}" | ";" => ide::formatting::format_document_edits(
            &sf,
            &crate::from_proto::format_options(&params.options),
        ),
        "\n" => ide::formatting::on_enter_edits(&sf, lc),
        "=" | ">" => {
            let offset = sf.offset_at(&lc);
            let text = sf.text();
            if offset >= 2 {
                let prev = text.as_bytes().get(offset - 2);
                if matches!(prev, Some(b'-') | Some(b'=')) {
                    ide::formatting::format_document_edits(
                        &sf,
                        &crate::from_proto::format_options(&params.options),
                    )
                } else {
                    None
                }
            } else {
                None
            }
        }
        _ => None,
    };
    match result {
        Some(edits) => Ok(Some(ide_edits_to_lsp(edits, &sf))),
        None => Ok(None),
    }
}

pub(crate) fn handle_inlay_hint_resolve(
    _snap: GlobalStateSnapshot,
    params: InlayHint,
) -> anyhow::Result<InlayHint> {
    let line_index = ide_db::line_index::LineIndex::new("");
    let mut ide_hint = crate::from_proto::inlay_hint(&line_index, &params);
    ide::inlay_hints::resolve_inlay_hint(&mut ide_hint);
    let mut result = params;
    result.tooltip = ide_hint.tooltip.map(|t| InlayHintTooltip::String(t));
    Ok(result)
}

pub(crate) fn handle_code_action(
    snap: GlobalStateSnapshot,
    params: CodeActionParams,
) -> anyhow::Result<Option<Vec<CodeActionOrCommand>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let requested_kinds = params.context.only.clone();
    let mut actions: Vec<CodeActionOrCommand> = Vec::new();
    let line_index = ide_db::line_index::LineIndex::new(sf.text());

    // --- Quick fixes for diagnostics ---
    for diagnostic in &params.context.diagnostics {
        let ide_diag = crate::from_proto::diagnostic(&line_index, diagnostic);
        if let Some((title, ide_edit, is_preferred)) =
            ide_assists::quick_fix_for_diagnostic(&sf, &ide_diag)
        {
            let kind = CodeActionKind::QUICKFIX;
            if crate::code_action_helpers::code_action_kind_allowed(&requested_kinds, &kind) {
                let lsp_edit = crate::to_proto::text_edit(&line_index, &ide_edit);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title,
                    kind: Some(kind),
                    diagnostics: Some(vec![diagnostic.clone()]),
                    edit: None,
                    command: None,
                    is_preferred: Some(is_preferred),
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &[lsp_edit])),
                }));
            }
        }
    }

    // --- Auto-include quick fix ---
    // Offer `$include "file.sail"` when the cursor is on an unresolved
    // identifier that is defined in another workspace file.
    if crate::code_action_helpers::code_action_kind_allowed(
        &requested_kinds,
        &CodeActionKind::QUICKFIX,
    ) {
        let offset = {
            let pos = params.range.start;
            line_index.offset(ide_db::LineCol { line: pos.line, col: pos.character })
        };
        let owned = snap.analysis.all_salsa_files();
        let all_files: Vec<(&Url, &dyn FileDb)> =
            owned.iter().map(|(u, s)| (u, s as &dyn FileDb)).collect();
        if let Some((title, ide_edit)) =
            ide_assists::auto_include_edits(&sf, offset, all_files.iter().copied(), Some(uri))
        {
            let lsp_edit = crate::to_proto::text_edit(&line_index, &ide_edit);
            actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                title,
                kind: Some(CodeActionKind::QUICKFIX),
                diagnostics: None,
                edit: None,
                command: None,
                is_preferred: Some(true),
                disabled: None,
                data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &[lsp_edit])),
            }));
        }
    }

    // --- Refactoring assists ---
    let ide_range = crate::from_proto::text_range(&line_index, params.range);

    let range_assists: &[(
        &str,
        CodeActionKind,
        fn(
            &dyn ide_db::FileDb,
            ide_db::line_index::TextRange,
        ) -> Option<Vec<ide_db::ide_types::IdeTextEdit>>,
    )] = &[
        ("Invert if", CodeActionKind::REFACTOR_REWRITE, ide_assists::invert_if_edits),
        (
            "Flip binary expression",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::flip_binexpr_edits,
        ),
        (
            "Apply De Morgan's law",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::apply_demorgan_edits,
        ),
        (
            "Extract to variable",
            CodeActionKind::REFACTOR_EXTRACT,
            ide_assists::extract_local_let_edits,
        ),
        ("Extract function", CodeActionKind::REFACTOR_EXTRACT, ide_assists::extract_function_edits),
        ("Inline variable", CodeActionKind::REFACTOR_INLINE, ide_assists::inline_variable_edits),
        (
            "Generate doc template",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::generate_doc_template_edits,
        ),
        ("Unwrap block", CodeActionKind::REFACTOR_REWRITE, ide_assists::unwrap_block_edits),
        (
            "Pull assignment up",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::pull_assignment_up_edits,
        ),
        (
            "Convert to guarded return",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::guarded_return_edits,
        ),
        ("Sort items", CodeActionKind::REFACTOR_REWRITE, ide_assists::sort_items_edits),
        (
            "Line to block comment",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::line_to_block_comment_edits,
        ),
        (
            "Block to line comment",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::block_to_line_comment_edits,
        ),
        (
            "Toggle doc comment",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::toggle_doc_comment_edits,
        ),
        (
            "Generate bitfield accessors",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::bitfield_accessor_edits,
        ),
        (
            "Evaluate constant",
            CodeActionKind::REFACTOR_REWRITE,
            ide_assists::evaluate_constant_edits,
        ),
        ("Simplify boolean", CodeActionKind::REFACTOR_REWRITE, ide_assists::simplify_boolean_edits),
    ];

    for (title, kind, assist_fn) in range_assists {
        if crate::code_action_helpers::code_action_kind_allowed(&requested_kinds, kind) {
            if let Some(edits) = assist_fn(&sf, ide_range) {
                if !edits.is_empty() {
                    let lsp_edits = ide_edits_to_lsp(edits, &sf);
                    actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                        title: title.to_string(),
                        kind: Some(kind.clone()),
                        diagnostics: None,
                        edit: None,
                        command: None,
                        is_preferred: None,
                        disabled: None,
                        data: Some(crate::code_action_helpers::lazy_code_action_data(
                            uri, &lsp_edits,
                        )),
                    }));
                }
            }
        }
    }

    // Offset-based assists
    let offset = base_db::range_start(ide_range);
    if crate::code_action_helpers::code_action_kind_allowed(
        &requested_kinds,
        &CodeActionKind::REFACTOR,
    ) {
        if let Some(edits) = ide_assists::generate_function_edits(&sf, offset) {
            if !edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Generate function".to_string(),
                    kind: Some(CodeActionKind::REFACTOR),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
        if let Some(edits) = ide_assists::inline_call_edits(&sf, offset) {
            if !edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Inline call".to_string(),
                    kind: Some(CodeActionKind::REFACTOR_INLINE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
    }

    // Literal format conversions
    if crate::code_action_helpers::code_action_kind_allowed(
        &requested_kinds,
        &CodeActionKind::REFACTOR_REWRITE,
    ) {
        for (title, edits) in ide_assists::convert_literal_format_edits(&sf, ide_range) {
            if !edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title,
                    kind: Some(CodeActionKind::REFACTOR_REWRITE),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
    }

    // --- Source actions ---
    let source_kind = CodeActionKind::SOURCE;
    if crate::code_action_helpers::code_action_kind_allowed(&requested_kinds, &source_kind) {
        if let Some(edits) = ide_assists::organize_imports_edits(&sf) {
            if !edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Organize imports".to_string(),
                    kind: Some(source_kind.clone()),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
        if let Some(edits) = ide_assists::remove_unused_imports_edits(&sf) {
            if !edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Remove unused imports".to_string(),
                    kind: Some(source_kind.clone()),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
    }

    // Format fix-all
    let source_fix_all_kind = crate::code_action_helpers::sail_source_fix_all_kind();
    if crate::code_action_helpers::code_action_kind_allowed(&requested_kinds, &source_fix_all_kind)
    {
        let fmt_opts = snap.last_format_options.lock()
            .map(|o| o.clone())
            .unwrap_or_else(|_| crate::code_action_helpers::default_code_action_format_options());
        if let Some(ide_edits) = ide::formatting::format_document_edits(
            &sf,
            &fmt_opts,
        ) {
            if !ide_edits.is_empty() {
                let lsp_edits = ide_edits_to_lsp(ide_edits, &sf);
                actions.push(CodeActionOrCommand::CodeAction(CodeAction {
                    title: "Format document".to_string(),
                    kind: Some(source_fix_all_kind),
                    diagnostics: None,
                    edit: None,
                    command: None,
                    is_preferred: None,
                    disabled: None,
                    data: Some(crate::code_action_helpers::lazy_code_action_data(uri, &lsp_edits)),
                }));
            }
        }
    }

    if actions.is_empty() {
        Ok(None)
    } else {
        Ok(Some(actions))
    }
}

pub(crate) fn handle_code_action_resolve(
    _snap: GlobalStateSnapshot,
    mut params: CodeAction,
) -> anyhow::Result<CodeAction> {
    if params.edit.is_none() {
        if let Some(data) = params.data.as_ref() {
            if let Some((uri, edits)) =
                crate::code_action_helpers::resolve_code_action_edit_from_data(data)
            {
                let mut changes = HashMap::new();
                changes.insert(uri, edits);
                params.edit = Some(WorkspaceEdit { changes: Some(changes), ..Default::default() });
            }
        }
    }
    Ok(params)
}

pub(crate) fn handle_completion(
    snap: GlobalStateSnapshot,
    params: CompletionParams,
) -> anyhow::Result<Option<CompletionResponse>> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let line_col = crate::lsp_ext::line_col_from_position(position);
    let ide_items = snap.analysis.completions(file_id, line_col)?;
    if ide_items.is_empty() {
        return Ok(None);
    }
    let lsp_items: Vec<CompletionItem> =
        ide_items.iter().map(|i| crate::to_proto::completion_item(i)).collect();
    Ok(Some(CompletionResponse::Array(lsp_items)))
}

pub(crate) fn handle_resolve_completion_item(
    snap: GlobalStateSnapshot,
    item: CompletionItem,
) -> anyhow::Result<CompletionItem> {
    let owned = snap.analysis.all_salsa_files();
    let all_files: Vec<(&Url, &dyn FileDb)> =
        owned.iter().map(|(u, s)| (u, s as &dyn FileDb)).collect();
    let mut ide_item = crate::from_proto::completion_item(&item);
    ide::completion::resolve_completion_item_ide(&mut ide_item, &all_files);
    Ok(crate::to_proto::completion_item(&ide_item))
}

pub(crate) fn handle_inlay_hint(
    snap: GlobalStateSnapshot,
    params: InlayHintParams,
) -> anyhow::Result<Option<Vec<InlayHint>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let owned = snap.analysis.all_salsa_files();
    let all_files: Vec<(&Url, &dyn FileDb)> =
        owned.iter().map(|(u, s)| (u, s as &dyn FileDb)).collect();
    let range =
        crate::from_proto::text_range(&ide_db::line_index::LineIndex::new(sf.text()), params.range);
    let ft = snap.analysis.file_id(uri).and_then(|fid| snap.analysis.files_ref().file_text(fid));
    let type_check = ft.map(|ft| hir_ty::query::infer_body(snap.analysis.db(), ft));
    // Use salsa-cached transitive effects for effect hints.
    let transitive_effects =
        ft.map(|ft| hir_ty::query::transitive_effects(snap.analysis.db(), ft));
    let ide_hints = ide::inlay_hints::inlay_hints_ide_with_types(
        &all_files,
        uri,
        &sf,
        range,
        type_check.map(|tc| &*tc.0),
        transitive_effects.map(|te| &*te.0),
    );
    if ide_hints.is_empty() {
        return Ok(None);
    }
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let lsp_hints: Vec<InlayHint> =
        ide_hints.iter().map(|h| crate::to_proto::inlay_hint(&line_index, h)).collect();
    Ok(Some(lsp_hints))
}

pub(crate) fn handle_signature_help(
    snap: GlobalStateSnapshot,
    params: SignatureHelpParams,
) -> anyhow::Result<Option<SignatureHelp>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let owned = snap.analysis.all_salsa_files();
    let all_files: Vec<(&Url, &dyn FileDb)> =
        owned.iter().map(|(u, s)| (u, s as &dyn FileDb)).collect();
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some(ide_help) = ide::calls::signature_help_ide(&all_files, uri, &sf, lc) else {
        return Ok(None);
    };
    let lsp_help = SignatureHelp {
        signatures: ide_help
            .signatures
            .into_iter()
            .map(|s| SignatureInformation {
                label: s.label,
                documentation: s.documentation.map(|d| Documentation::String(d)),
                parameters: Some(
                    s.parameters
                        .into_iter()
                        .map(|p| ParameterInformation {
                            label: ParameterLabel::Simple(p.label),
                            documentation: None,
                        })
                        .collect(),
                ),
                active_parameter: None,
            })
            .collect(),
        active_signature: ide_help.active_signature.map(|v| v as u32),
        active_parameter: ide_help.active_parameter.map(|v| v as u32),
    };
    Ok(Some(lsp_help))
}

/// Semantic-first resolution with index fallback.
/// `Semantics::new(db)` → `IdentClass::classify_node(sema, &parent)`
/// (`ide/src/goto_definition.rs:43-150`).
/// We use `goto_definition_semantic` for local-scope resolution
/// (handles shadowing, let bindings), then fall back to the workspace
/// index for cross-file definitions.
pub(crate) fn handle_goto_definition(
    snap: GlobalStateSnapshot,
    params: GotoDefinitionParams,
) -> anyhow::Result<Option<GotoDefinitionResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);

    // 1. Semantic path: scope-aware local resolution.
    // (`ide/src/goto_definition.rs:139-150`).
    if let Some(nav_targets) = ide::goto_definition::goto_definition_semantic(&sf, lc, uri) {
        if !nav_targets.is_empty() {
            let lsp_locs = nav_targets_to_lsp(&nav_targets, &snap.analysis);
            if !lsp_locs.is_empty() {
                return Ok(Some(GotoDefinitionResponse::Array(lsp_locs)));
            }
        }
    }

    // 1b. 投産-4: Field/method resolution via SourceAnalyzer.
    // Uses resolve_field() and resolve_method_call() for struct field
    // access and function call expressions. Requires salsa FileText.
    if let Some(file_id) = snap.analysis.file_id(uri) {
        if let Some(file_text) = snap.analysis.files_ref().file_text(file_id) {
            let offset = sf.offset_at(&lc);
            if let Some(nav_targets) = ide::goto_definition::goto_definition_field_or_method(
                snap.analysis.db(),
                file_text,
                offset,
                uri,
                &snap.workspace_index,
            ) {
                if !nav_targets.is_empty() {
                    let lsp_locs = nav_targets_to_lsp(&nav_targets, &snap.analysis);
                    if !lsp_locs.is_empty() {
                        return Ok(Some(GotoDefinitionResponse::Array(lsp_locs)));
                    }
                }
            }
        }
    }

    // 2. Index fallback: cross-file O(1) lookup.
    // Sail-specific: no CrateDefMap, so we use SymbolIndex.
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(key) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let locs = ide::navigation::definition_locations_indexed(&snap.workspace_index, &key, uri);
    if locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDefinitionResponse::Array(file_locations_to_lsp(&locs, &snap.analysis))))
    }
}

pub(crate) fn handle_goto_declaration(
    snap: GlobalStateSnapshot,
    params: GotoDeclarationParams,
) -> anyhow::Result<Option<GotoDeclarationResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(key) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let locs = ide::navigation::declaration_locations_indexed(&snap.workspace_index, &key, uri);
    if locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDefinitionResponse::Array(file_locations_to_lsp(&locs, &snap.analysis))))
    }
}

/// Find all references — returns structured results with declaration.
/// declaration + per-file references with Read/Write categories.
pub(crate) fn handle_references(
    snap: GlobalStateSnapshot,
    params: ReferenceParams,
) -> anyhow::Result<Option<Vec<Location>>> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);

    // Get FileId for the target file
    let Some(target_file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };

    // Build (FileId, &dyn FileDb) pairs for all workspace files
    let all = snap.analysis.all_salsa_files();
    let file_pairs: Vec<(base_db::FileId, &dyn ide_db::FileDb)> = all
        .iter()
        .filter_map(|(url, salsa_file)| {
            let fid = snap.analysis.file_id(url)?;
            Some((fid, salsa_file as &dyn ide_db::FileDb))
        })
        .collect();

    // Use find_all_refs
    let results = ide::references::find_all_refs(&file_pairs, target_file_id, &sf, lc, None);

    let Some(results) = results else {
        return Ok(None);
    };

    // Convert to LSP Location list
    let mut locations = Vec::new();
    for result in &results {
        // Include declaration if requested
        if params.context.include_declaration {
            if let Some(decl) = &result.declaration {
                if let Some(decl_url) = snap.analysis.url_for_file_id(decl.file_id) {
                    if let Some(decl_file) = snap.analysis.file(decl_url) {
                        locations.push(Location {
                            uri: decl_url.clone(),
                            range: Range::new(
                                crate::lsp_ext::position_from_line_col(
                                    decl_file.position_at(base_db::range_start(decl.range)),
                                ),
                                crate::lsp_ext::position_from_line_col(
                                    decl_file.position_at(base_db::range_end(decl.range)),
                                ),
                            ),
                        });
                    }
                }
            }
        }

        // References
        for (&file_id, refs) in &result.references {
            if let Some(ref_url) = snap.analysis.url_for_file_id(file_id) {
                if let Some(ref_file) = snap.analysis.file(ref_url) {
                    for (range, _category) in refs {
                        locations.push(Location {
                            uri: ref_url.clone(),
                            range: Range::new(
                                crate::lsp_ext::position_from_line_col(
                                    ref_file.position_at(base_db::range_start(*range)),
                                ),
                                crate::lsp_ext::position_from_line_col(
                                    ref_file.position_at(base_db::range_end(*range)),
                                ),
                            ),
                        });
                    }
                }
            }
        }
    }

    if locations.is_empty() {
        Ok(None)
    } else {
        Ok(Some(locations))
    }
}

/// Rename a symbol across the workspace.
/// name validation, then `find_name_in_text` for
/// occurrence collection, returning results via `SourceChange`.
/// `Definition::rename()` → `SourceChange`.
pub(crate) fn handle_rename(
    snap: GlobalStateSnapshot,
    params: RenameParams,
) -> anyhow::Result<Option<WorkspaceEdit>> {
    let uri = &params.text_document_position.text_document.uri;
    let position = params.text_document_position.position;
    let new_name = &params.new_name;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);

    // 1. Validate new name via IdentifierKind::classify
    let (_validated_name, _kind) = match ide_db::rename::IdentifierKind::classify(new_name) {
        Ok(v) => v,
        Err(e) => return Err(anyhow::anyhow!("{}", e)),
    };

    // 2. Verify symbol exists at cursor (early exit if not renameable)
    let Some(_symbol) = ide::references::resolve_symbol_at(&sf, lc) else {
        return Ok(None);
    };

    // 3. Handle type variable rename (prefix with ')
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let effective_name =
        match ide::references::normalize_validated_rename(token, new_name, ide_db::SAIL_KEYWORDS) {
            Ok(Some(v)) => v,
            Ok(None) => new_name.to_string(),
            Err(e) => return Err(anyhow::anyhow!("{}", e)),
        };

    // 4. Collect edits via find_all_refs path (FileId-keyed)
    let all = snap.analysis.all_salsa_files();
    let file_pairs: Vec<(base_db::FileId, &dyn ide_db::FileDb)> = all
        .iter()
        .filter_map(|(url, salsa_file)| {
            let fid = snap.analysis.file_id(url)?;
            Some((fid, salsa_file as &dyn ide_db::FileDb))
        })
        .collect();
    let target_file_id = snap.analysis.file_id(uri).unwrap_or(base_db::FileId::from_raw(0));

    let results = ide::references::find_all_refs(&file_pairs, target_file_id, &sf, lc, None);

    let Some(results) = results else {
        return Ok(None);
    };

    // 5. Build SourceChange from results
    let mut source_change = ide_db::source_change::SourceChange::new();
    for result in &results {
        // Include declaration edit
        if let Some(decl) = &result.declaration {
            source_change.insert_source_edit(
                decl.file_id,
                vec![ide_db::text_edit::TextEdit {
                    range: decl.range,
                    new_text: effective_name.clone(),
                }],
            );
        }
        // Reference edits
        for (&file_id, refs) in &result.references {
            let edits: Vec<ide_db::text_edit::TextEdit> = refs
                .iter()
                .map(|(range, _)| ide_db::text_edit::TextEdit {
                    range: *range,
                    new_text: effective_name.clone(),
                })
                .collect();
            if !edits.is_empty() {
                source_change.insert_source_edit(file_id, edits);
            }
        }
    }

    if source_change.is_empty() {
        return Ok(None);
    }

    // 6. Convert SourceChange to LSP WorkspaceEdit via to_proto
    let ws_edit = crate::to_proto::workspace_edit(
        &source_change,
        &|fid| snap.analysis.url_for_file_id(fid).cloned(),
        &|fid| {
            snap.analysis
                .url_for_file_id(fid)
                .and_then(|url| snap.analysis.file(url))
                .map(|f| ide_db::line_index::LineIndex::new(f.text()))
        },
    );

    Ok(Some(ws_edit))
}

pub(crate) fn handle_code_lens(
    snap: GlobalStateSnapshot,
    params: CodeLensParams,
) -> anyhow::Result<Option<Vec<CodeLens>>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let Some(ide_lenses) = snap.analysis.code_lenses(file_id).ok() else {
        return Ok(None);
    };
    if ide_lenses.is_empty() {
        return Ok(None);
    }
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let lsp_lenses: Vec<CodeLens> = ide_lenses
        .into_iter()
        .map(|l| {
            // Inject the file URI into lens data for resolve-time command building.
            let data = l.data.map(|mut d| {
                if let Some(obj) = d.as_object_mut() {
                    obj.insert("uri".to_string(), serde_json::json!(uri.as_str()));
                }
                d
            });
            CodeLens {
                range: crate::to_proto::range(&line_index, l.range),
                command: None, // Filled in during codeLens/resolve
                data,
            }
        })
        .collect();
    Ok(Some(lsp_lenses))
}

pub(crate) fn handle_call_hierarchy_prepare(
    snap: GlobalStateSnapshot,
    params: CallHierarchyPrepareParams,
) -> anyhow::Result<Option<Vec<CallHierarchyItem>>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let all = snap.analysis.all_salsa_files();
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(name) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let Some(ide_item) =
        ide::navigation::call_hierarchy_item(all.iter().map(|(u, s)| (u, s)), uri, &name)
    else {
        return Ok(None);
    };
    let li = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(vec![crate::to_proto::call_hierarchy_item(&li, &ide_item)]))
}

pub(crate) fn handle_call_hierarchy_incoming(
    snap: GlobalStateSnapshot,
    params: CallHierarchyIncomingCallsParams,
) -> anyhow::Result<Option<Vec<CallHierarchyIncomingCall>>> {
    let target_name = params.item.name.clone();
    let all = snap.analysis.all_salsa_files();
    let items = ide::call_hierarchy::incoming_calls(all.iter().map(|(u, s)| (u, s)), &target_name);
    let mut calls = Vec::new();
    for item in &items {
        if let Some(from_item) = ide::call_hierarchy::call_hierarchy_item(
            all.iter().map(|(u, s)| (u, s)),
            &item.caller_uri,
            &item.caller,
        ) {
            let caller_sf = snap.analysis.file(&item.caller_uri);
            let li = caller_sf.map(|s| ide_db::line_index::LineIndex::new(s.text()));
            if let Some(li) = &li {
                calls.push(CallHierarchyIncomingCall {
                    from: crate::to_proto::call_hierarchy_item(li, &from_item),
                    from_ranges: item
                        .ranges
                        .iter()
                        .map(|r| crate::to_proto::range(li, *r))
                        .collect(),
                });
            }
        }
    }
    if calls.is_empty() {
        Ok(None)
    } else {
        Ok(Some(calls))
    }
}

pub(crate) fn handle_call_hierarchy_outgoing(
    snap: GlobalStateSnapshot,
    params: CallHierarchyOutgoingCallsParams,
) -> anyhow::Result<Option<Vec<CallHierarchyOutgoingCall>>> {
    let caller_name = params.item.name.clone();
    let caller_uri = params.item.uri.clone();
    let all = snap.analysis.all_salsa_files();
    let items = ide::call_hierarchy::outgoing_calls(all.iter().map(|(u, s)| (u, s)), &caller_name);
    let mut calls = Vec::new();
    for item in &items {
        if let Some(to_item) = ide::call_hierarchy::call_hierarchy_item(
            all.iter().map(|(u, s)| (u, s)),
            &caller_uri,
            &item.callee,
        ) {
            let callee_sf = snap.analysis.file(&caller_uri);
            let li = callee_sf.map(|s| ide_db::line_index::LineIndex::new(s.text()));
            if let Some(li) = &li {
                calls.push(CallHierarchyOutgoingCall {
                    to: crate::to_proto::call_hierarchy_item(li, &to_item),
                    from_ranges: item
                        .ranges
                        .iter()
                        .map(|r| crate::to_proto::range(li, *r))
                        .collect(),
                });
            }
        }
    }
    if calls.is_empty() {
        Ok(None)
    } else {
        Ok(Some(calls))
    }
}

pub(crate) fn handle_workspace_symbol(
    snap: GlobalStateSnapshot,
    params: WorkspaceSymbolParams,
) -> anyhow::Result<Option<WorkspaceSymbolResponse>> {
    let query = params.query.to_ascii_lowercase();
    let results: Vec<SymbolInformation> = snap
        .workspace_index
        .search(&query)
        .into_iter()
        .filter_map(|entry| {
            let sf = snap.analysis.file(&entry.url)?;
            let p = crate::lsp_ext::position_from_line_col(sf.position_at(entry.span.start));
            #[allow(deprecated)]
            Some(SymbolInformation {
                name: entry.name.clone(),
                kind: crate::lsp_ext::item_kind_to_symbol_kind(entry.kind),
                tags: None,
                deprecated: None,
                location: Location::new(entry.url.clone(), Range::new(p, p)),
                container_name: None,
            })
        })
        .collect();
    if results.is_empty() {
        Ok(None)
    } else {
        Ok(Some(WorkspaceSymbolResponse::Flat(results)))
    }
}

pub(crate) fn handle_hover(
    snap: GlobalStateSnapshot,
    params: HoverParams,
) -> anyhow::Result<Option<Hover>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let all = snap.analysis.all_salsa_files();
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, span)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(key) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let hover_range = base_db::text_range(span.start, span.end);
    let Some(ide_hover) = ide::hover::hover_for_symbol(
        all.iter().map(|(u, s)| (u, s)),
        uri,
        &sf,
        lc,
        hover_range,
        &key,
    ) else {
        return Ok(None);
    };
    // 投産-4: Enrich hover with field/method resolution from SourceAnalyzer.
    // This supplements the structural hover with semantic information
    // when the cursor is on a field access or method call expression.
    if let Some(file_id) = snap.analysis.file_id(uri) {
        if let Some(file_text) = snap.analysis.files_ref().file_text(file_id) {
            let offset = sf.offset_at(&lc);
            if let Some(extra) =
                ide::hover::hover_resolve_field_or_method(snap.analysis.db(), file_text, offset)
            {
                let enriched = ide_db::ide_types::HoverResult {
                    markup: format!("{}\n\n{}", ide_hover.markup, extra),
                    range: ide_hover.range,
                    actions: ide_hover.actions,
                };
                let line_index = ide_db::line_index::LineIndex::new(sf.text());
                return Ok(Some(crate::to_proto::hover(&line_index, &enriched)));
            }
        }
    }

    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(crate::to_proto::hover(&line_index, &ide_hover)))
}

pub(crate) fn handle_goto_implementation(
    snap: GlobalStateSnapshot,
    params: GotoImplementationParams,
) -> anyhow::Result<Option<GotoImplementationResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(key) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let locs = ide::navigation::implementation_locations_indexed(&snap.workspace_index, &key, uri);
    if locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoImplementationResponse::Array(file_locations_to_lsp(&locs, &snap.analysis))))
    }
}

pub(crate) fn handle_will_rename_files(
    snap: GlobalStateSnapshot,
    params: RenameFilesParams,
) -> anyhow::Result<Option<WorkspaceEdit>> {
    let rename_pairs: Vec<(String, String)> =
        params.files.iter().map(|r| (r.old_uri.clone(), r.new_uri.clone())).collect();
    let all = snap.analysis.all_salsa_files();
    let edits =
        ide::navigation::will_rename_file_edits(all.iter().map(|(u, s)| (u, s)), &rename_pairs);
    Ok(edits.map(|changes| {
        let lsp_changes: HashMap<Url, Vec<TextEdit>> = changes
            .into_iter()
            .filter_map(|(uri, ide_edits)| {
                let sf = snap.analysis.file(&uri)?;
                let li = ide_db::line_index::LineIndex::new(sf.text());
                let lsp_edits =
                    ide_edits.iter().map(|e| crate::to_proto::text_edit(&li, e)).collect();
                Some((uri, lsp_edits))
            })
            .collect();
        WorkspaceEdit { changes: Some(lsp_changes), ..Default::default() }
    }))
}

pub(crate) fn handle_goto_type_definition(
    snap: GlobalStateSnapshot,
    params: GotoTypeDefinitionParams,
) -> anyhow::Result<Option<GotoTypeDefinitionResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let all = snap.analysis.all_salsa_files();
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(key) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let bindings = ide::navigation::typed_bindings(&sf);
    if let Some(ty_name) = bindings.get(&key) {
        if let Some(parsed_ty) = ide::navigation::parse_named_type(ty_name) {
            let locs = ide::navigation::type_definition_locations(
                all.iter().map(|(u, s)| (u, s)),
                uri,
                &parsed_ty,
            );
            if !locs.is_empty() {
                return Ok(Some(GotoTypeDefinitionResponse::Array(file_locations_to_lsp(
                    &locs,
                    &snap.analysis,
                ))));
            }
        }
        let locs = ide::navigation::type_definition_locations(
            all.iter().map(|(u, s)| (u, s)),
            uri,
            ty_name,
        );
        if !locs.is_empty() {
            return Ok(Some(GotoTypeDefinitionResponse::Array(file_locations_to_lsp(
                &locs,
                &snap.analysis,
            ))));
        }
    }
    Ok(None)
}

pub(crate) fn handle_type_hierarchy_prepare(
    snap: GlobalStateSnapshot,
    params: TypeHierarchyPrepareParams,
) -> anyhow::Result<Option<Vec<TypeHierarchyItem>>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let all = snap.analysis.all_salsa_files();
    let lc = crate::lsp_ext::line_col_from_position(position);
    let Some((token, _)) = sf.token_at(lc) else {
        return Ok(None);
    };
    let Some(name) = ide_db::token_symbol_key(token) else {
        return Ok(None);
    };
    let Some(ide_item) =
        ide::navigation::type_hierarchy_item(all.iter().map(|(u, s)| (u, s)), uri, &name)
    else {
        return Ok(None);
    };
    let li = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(vec![crate::to_proto::type_hierarchy_item(&li, &ide_item)]))
}

pub(crate) fn handle_type_hierarchy_supertypes(
    snap: GlobalStateSnapshot,
    params: TypeHierarchySupertypesParams,
) -> anyhow::Result<Option<Vec<TypeHierarchyItem>>> {
    let name = &params.item.name;
    let uri = &params.item.uri;
    let all = snap.analysis.all_salsa_files();
    let ide_items = ide::navigation::type_supertypes(all.iter().map(|(u, s)| (u, s)), uri, name);
    if ide_items.is_empty() {
        return Ok(None);
    }
    let lsp_items: Vec<_> = ide_items
        .iter()
        .filter_map(|it| {
            let sf = snap.analysis.file(&it.url)?;
            let li = ide_db::line_index::LineIndex::new(sf.text());
            Some(crate::to_proto::type_hierarchy_item(&li, it))
        })
        .collect();
    Ok(Some(lsp_items))
}

pub(crate) fn handle_type_hierarchy_subtypes(
    snap: GlobalStateSnapshot,
    params: TypeHierarchySubtypesParams,
) -> anyhow::Result<Option<Vec<TypeHierarchyItem>>> {
    let name = &params.item.name;
    let uri = &params.item.uri;
    let all = snap.analysis.all_salsa_files();
    let ide_items = ide::navigation::type_subtypes(all.iter().map(|(u, s)| (u, s)), uri, name);
    if ide_items.is_empty() {
        return Ok(None);
    }
    let lsp_items: Vec<_> = ide_items
        .iter()
        .filter_map(|it| {
            let sf = snap.analysis.file(&it.url)?;
            let li = ide_db::line_index::LineIndex::new(sf.text());
            Some(crate::to_proto::type_hierarchy_item(&li, it))
        })
        .collect();
    Ok(Some(lsp_items))
}

/// Handle `experimental/ssr` — structural search-replace.
pub(crate) fn handle_ssr(
    _snap: GlobalStateSnapshot,
    params: crate::lsp_ext::SsrParams,
) -> anyhow::Result<lsp_types::WorkspaceEdit> {
    let _rule: ide_ssr::SsrRule = params.query.parse()?;
    // SSR infrastructure is in place; return empty edits for now.
    Ok(lsp_types::WorkspaceEdit::default())
}

/// View the concrete syntax tree for a file.
/// Registered as `sail-lsp/viewSyntaxTree`.
pub(crate) fn handle_view_syntax_tree(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewSyntaxTreeParams,
) -> anyhow::Result<String> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok("(file not found)".to_string());
    };
    Ok(ide::view_syntax_tree::view_syntax_tree(&sf))
}

/// View HIR at cursor position.
/// Registered as `sail-lsp/viewHir`.
pub(crate) fn handle_view_hir(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewHirParams,
) -> anyhow::Result<String> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok("(file not found)".to_string());
    };
    let lc = crate::lsp_ext::line_col_from_position(params.position);
    Ok(ide::view_hir::view_hir_at(&sf, lc))
}

/// View the ItemTree for a file.
/// Registered as `sail-lsp/viewItemTree`.
pub(crate) fn handle_view_item_tree(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewItemTreeParams,
) -> anyhow::Result<String> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok("(file not found)".to_string());
    };
    Ok(ide::view_item_tree::view_item_tree(&sf))
}

/// Expand $include directives for a file.
/// Registered as `sail-lsp/expandInclude`.
pub(crate) fn handle_expand_include(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ExpandIncludeParams,
) -> anyhow::Result<Option<crate::lsp_ext::ExpandIncludeResult>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let result = ide::expand_include::expand_includes(
        &snap.include_graph,
        file_id,
        &|fid| {
            snap.analysis
                .url_for_file_id(fid)
                .and_then(|url| snap.analysis.file(url))
                .map(|sf| sf.text().to_string())
        },
        &|fid| {
            snap.analysis
                .url_for_file_id(fid)
                .map(|u| u.path().rsplit('/').next().unwrap_or("?").to_string())
                .unwrap_or_else(|| format!("file:{}", fid.index()))
        },
    );
    Ok(Some(crate::lsp_ext::ExpandIncludeResult { root: result.root, expansion: result.expansion }))
}

/// Effect annotations for all callables in a file.
/// Sail-specific: returns per-function inferred effects.
/// Registered as `sail-lsp/effectAnnotations`.
pub(crate) fn handle_effect_annotations(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::EffectAnnotationsParams,
) -> anyhow::Result<Vec<crate::lsp_ext::EffectAnnotationItem>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(Vec::new());
    };
    let annotations = ide::effect_annotations::effect_annotations(&sf);
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let items: Vec<_> = annotations
        .into_iter()
        .map(|a| {
            let range =
                crate::to_proto::range(&line_index, base_db::text_range(a.span.start, a.span.end));
            crate::lsp_ext::EffectAnnotationItem {
                name: a.name,
                range,
                effects: a.effects.iter().map(|e| format!("{:?}", e)).collect(),
            }
        })
        .collect();
    Ok(items)
}

/// Server analyzer status — internal diagnostics.
/// Registered as `sail-lsp/analyzerStatus`.
pub(crate) fn handle_sail_lsp_status(
    snap: GlobalStateSnapshot,
    _params: (),
) -> anyhow::Result<String> {
    let all = snap.analysis.all_salsa_files();
    let file_count = all.len();
    let index_count = snap.workspace_index.all_entries().count();
    let include_files = snap.include_graph.all_files().len();

    let mut status = String::new();
    stdx::format_to!(status, "sail-lsp status\n");
    stdx::format_to!(status, "  files loaded:     {file_count}\n");
    stdx::format_to!(status, "  index symbols:    {index_count}\n");
    stdx::format_to!(status, "  include graph:    {include_files} nodes\n");
    Ok(status)
}

/// View include graph for a file.
/// Sail-specific debugging aid.
/// Registered as `sail-lsp/viewIncludeGraph`.
pub(crate) fn handle_view_include_graph(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewIncludeGraphParams,
) -> anyhow::Result<String> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok("(file not found)".to_string());
    };
    let graph = ide::include_graph_view::render_include_graph(&snap.include_graph, &|fid| {
        snap.analysis
            .url_for_file_id(fid)
            .map(|u| u.path().rsplit('/').next().unwrap_or("?").to_string())
            .unwrap_or_else(|| format!("file:{}", fid.index()))
    });
    let _ = file_id; // Used for context; graph renders all nodes
    Ok(graph)
}

