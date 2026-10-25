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

use hir::callgraph::SourceFileInfo;
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
    targets: &[ide::NavigationTarget],
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

/// The line index of the file a request is about.
///
/// Every position conversion needs one, which is the point: the pair this
/// replaced took only a `Position` and assigned `character` to a byte
/// column. Rebuilt per call rather than cached — a request handles one
/// position or a few, and the index is linear in the file it is about.
fn line_index_for(
    snap: &GlobalStateSnapshot,
    uri: &lsp_types::Url,
) -> ide_db::line_index::LineIndex {
    let text = snap.analysis.file(uri).map(|sf| sf.text().to_string()).unwrap_or_default();
    ide_db::line_index::LineIndex::new(&text)
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
        .map(|t: &ide::NavigationTarget| crate::to_proto::document_symbol(&line_index, t))
        .collect();
    // A client that did not say it understands the nested form gets the
    // flat one. This always sent `Nested`, and
    // `hierarchical_symbols_support` — which says whether that is allowed —
    // had no caller: an editor without the capability sees an empty
    // outline, and nothing here would have said why.
    if snap.client_caps.hierarchical_symbols_support() {
        return Ok(Some(DocumentSymbolResponse::Nested(tree)));
    }
    Ok(Some(DocumentSymbolResponse::Flat(flatten_symbols(&tree, uri))))
}

/// The nested outline as the flat list the older protocol form wants.
#[allow(deprecated)]
fn flatten_symbols(
    tree: &[lsp_types::DocumentSymbol],
    uri: &Url,
) -> Vec<lsp_types::SymbolInformation> {
    fn walk(
        out: &mut Vec<lsp_types::SymbolInformation>,
        nodes: &[lsp_types::DocumentSymbol],
        uri: &Url,
        container: Option<&str>,
    ) {
        for node in nodes {
            out.push(lsp_types::SymbolInformation {
                name: node.name.clone(),
                kind: node.kind,
                tags: node.tags.clone(),
                deprecated: None,
                location: lsp_types::Location::new(uri.clone(), node.range),
                container_name: container.map(str::to_owned),
            });
            if let Some(children) = &node.children {
                walk(out, children, uri, Some(&node.name));
            }
        }
    }
    let mut out = Vec::new();
    walk(&mut out, tree, uri, None);
    out
}

/// `experimental/matchingBrace` — where the other bracket is.
///
/// A position that is not on a bracket comes back unchanged, so the reply
/// lines up with the request one for one; that is rust-analyzer's contract
/// and what makes the client able to move each cursor independently.
pub(crate) fn handle_matching_brace(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::MatchingBraceParams,
) -> anyhow::Result<Vec<lsp_types::Position>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(params.positions);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    params
        .positions
        .into_iter()
        .map(|pos| {
            let offset = crate::from_proto::offset(&line_index, pos);
            match snap.analysis.matching_brace(uri, offset)? {
                Some(target) => Ok(crate::to_proto::position_in(&line_index, target)),
                None => Ok(pos),
            }
        })
        .collect()
}

/// `experimental/joinLines` — collapse the selected lines into one.
pub(crate) fn handle_join_lines(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::JoinLinesParams,
) -> anyhow::Result<Vec<TextEdit>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(Vec::new());
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let mut edits = Vec::new();
    for range in params.ranges {
        let range = crate::from_proto::text_range(&line_index, range);
        edits.extend(snap.analysis.join_lines(uri, range)?);
    }
    Ok(ide_edits_to_lsp(edits, &sf))
}

/// `experimental/onEnter` — what to insert when Enter is pressed.
pub(crate) fn handle_on_enter(
    snap: GlobalStateSnapshot,
    params: lsp_types::TextDocumentPositionParams,
) -> anyhow::Result<Option<Vec<crate::lsp_ext::SnippetTextEdit>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), params.position);
    let Some(edits) = snap.analysis.on_enter(uri, lc)? else {
        return Ok(None);
    };
    Ok(Some(snippet_edits(edits, &sf, snap.client_caps.snippet_text_edit_support())))
}

/// `experimental/moveItem` — swap a definition with its sibling.
pub(crate) fn handle_move_item(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::MoveItemParams,
) -> anyhow::Result<Vec<crate::lsp_ext::SnippetTextEdit>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(Vec::new());
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let offset = crate::from_proto::offset(&line_index, params.range.start);
    let direction = match params.direction {
        crate::lsp_ext::MoveItemDirection::Up => ide::MoveDirection::Up,
        crate::lsp_ext::MoveItemDirection::Down => ide::MoveDirection::Down,
    };
    let edits = snap.analysis.move_item(uri, offset, direction)?;
    Ok(snippet_edits(edits, &sf, snap.client_caps.snippet_text_edit_support()))
}

/// Convert ide edits to the snippet form, stripping placeholders for a
/// client that did not advertise `experimental.snippetTextEdit`.
fn snippet_edits(
    edits: Vec<ide_db::ide_types::IdeTextEdit>,
    file: &dyn FileDb,
    snippets: bool,
) -> Vec<crate::lsp_ext::SnippetTextEdit> {
    let line_index = ide_db::line_index::LineIndex::new(file.text());
    edits
        .into_iter()
        .map(|e| {
            let edit = crate::to_proto::text_edit(&line_index, &e);
            crate::lsp_ext::SnippetTextEdit {
                range: edit.range,
                new_text: edit.new_text,
                insert_text_format: snippets.then_some(lsp_types::InsertTextFormat::SNIPPET),
            }
        })
        .collect()
}

pub(crate) fn handle_selection_range(
    snap: GlobalStateSnapshot,
    params: SelectionRangeParams,
) -> anyhow::Result<Option<Vec<SelectionRange>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let ranges = params
        .positions
        .iter()
        .filter_map(|pos| {
            let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), *pos);
            let sr = snap.analysis.selection_range(uri, lc).ok().flatten()?;
            Some(crate::to_proto::selection_range(&line_index, &sr))
        })
        .collect::<Vec<_>>();
    Ok(Some(ranges))
}

pub(crate) fn handle_formatting(
    snap: GlobalStateSnapshot,
    params: DocumentFormattingParams,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let opts = crate::from_proto::format_options(&params.options);
    // Cache the client's format options so code actions can reuse them.
    if let Ok(mut cached) = snap.last_format_options.lock() {
        *cached = opts.clone();
    }
    let edits = snap.analysis.format_document(file_id, &opts).ok().flatten();
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
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let range = crate::from_proto::text_range(&line_index, params.range);
    let Some(tokens) = snap.analysis.semantic_tokens_range(uri, range)? else {
        return Ok(None);
    };
    Ok(Some(SemanticTokensRangeResult::Tokens(crate::to_proto::semantic_tokens(&tokens))))
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
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);

    let ranges = snap.analysis.highlight_related(uri, lc)?;
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
                    crate::to_proto::position_from_line_col(
                        &line_index_for(&snap, uri),
                        sf.position_at(base_db::range_start(hr.range)),
                    ),
                    crate::to_proto::position_from_line_col(
                        &line_index_for(&snap, uri),
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
    if !snap.config.semantic_tokens.enable {
        return Ok(None);
    }
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
    // Not for the bundled library: the user cannot fix those files, and a
    // false positive there would sit in the problems panel forever.
    if snap.is_library(file_id) {
        return Ok(empty_report());
    }
    let diag_config = snap.config.diagnostics.to_ide_config();

    // Main file diagnostics
    let ide_diags = snap.analysis.file_diagnostics(file_id, &diag_config)?;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(empty_report());
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let mut items: Vec<Diagnostic> = ide_diags
        .iter()
        .map(|d| crate::to_proto::diagnostic_with_analysis(&snap.analysis, &line_index, uri, d))
        .collect();
    crate::diagnostics::explain_unlinked_file(&mut items, snap.project_error.as_deref());

    // Related documents — files that $include this file.
    // When this file's types change, dependent files may get new errors.
    let supports_related = snap.client_caps.text_document_diagnostic_related_document_support();
    let mut related_documents: std::collections::HashMap<Url, DocumentDiagnosticReportKind> =
        std::collections::HashMap::new();

    if supports_related {
        for &dep_fid in snap.include_graph.included_by(file_id) {
            if let Some(dep_url) = snap.analysis.url_for_file_id(dep_fid) {
                let dep_diags = snap.analysis.file_diagnostics(dep_fid, &diag_config)?;
                if let Some(dep_sf) = snap.analysis.file(dep_url) {
                    let dep_li = ide_db::line_index::LineIndex::new(dep_sf.text());
                    let mut dep_items: Vec<Diagnostic> = dep_diags
                        .iter()
                        .map(|d| {
                            crate::to_proto::diagnostic_with_analysis(
                                &snap.analysis,
                                &dep_li,
                                dep_url,
                                d,
                            )
                        })
                        .collect();
                    crate::diagnostics::explain_unlinked_file(
                        &mut dep_items,
                        snap.project_error.as_deref(),
                    );
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
        // A changed loading cause can keep the same diagnostic count.
        serde_json::to_vec(&items)?.hash(&mut hasher);
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
    let opts = crate::from_proto::format_options(&params.options);
    match snap.analysis.range_format(uri, range, &opts)? {
        Some(edits) => Ok(Some(ide_edits_to_lsp(edits, &sf))),
        None => Ok(None),
    }
}

pub(crate) fn handle_linked_editing_range(
    snap: GlobalStateSnapshot,
    params: LinkedEditingRangeParams,
) -> anyhow::Result<Option<LinkedEditingRanges>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, uri),
        params.text_document_position_params.position,
    );
    let Some(result) = snap.analysis.linked_editing_ranges(uri, lc)? else {
        return Ok(None);
    };
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(crate::to_proto::linked_editing_ranges(&line_index, &result)))
}

pub(crate) fn handle_document_link(
    snap: GlobalStateSnapshot,
    params: DocumentLinkParams,
) -> anyhow::Result<Option<Vec<DocumentLink>>> {
    let uri = params.text_document.uri;
    let links = snap.analysis.document_links(&uri)?;
    if links.is_empty() {
        return Ok(None);
    }
    let Some(sf) = snap.analysis.file(&uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(links.iter().map(|l| crate::to_proto::document_link(&line_index, l)).collect()))
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
    let index = line_index_for(&snap, uri);
    let lc = crate::from_proto::line_col(&index, params.position);
    let Some((placeholder, start, end)) = snap.analysis.prepare_rename(uri, lc)? else {
        return Ok(None);
    };
    Ok(Some(PrepareRenameResponse::RangeWithPlaceholder {
        range: Range::new(
            crate::to_proto::position_from_line_col(&index, start),
            crate::to_proto::position_from_line_col(&index, end),
        ),
        placeholder,
    }))
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
    if !snap.config.semantic_tokens.enable {
        return Ok(None);
    }
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
    let ide_folds = snap.analysis.folding_ranges(uri)?;
    if ide_folds.is_empty() {
        return Ok(None);
    }
    let ranges: Vec<FoldingRange> = ide_folds.iter().map(crate::to_proto::folding_range).collect();
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
        if let Some(title) = _snap.analysis.code_lens_title(data)? {
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
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, uri),
        params.text_document_position.position,
    );
    let opts = crate::from_proto::format_options(&params.options);
    match snap.analysis.on_type_formatting(uri, lc, &params.ch, &opts)? {
        Some(edits) => Ok(Some(ide_edits_to_lsp(edits, &sf))),
        None => Ok(None),
    }
}

pub(crate) fn handle_inlay_hint_resolve(
    snap: GlobalStateSnapshot,
    params: InlayHint,
) -> anyhow::Result<InlayHint> {
    let line_index = ide_db::line_index::LineIndex::new("");
    let mut hint = crate::from_proto::inlay_hint(&line_index, &params);
    snap.analysis.resolve_inlay_hint(&mut hint)?;
    let mut result = params;
    result.tooltip = hint.tooltip.map(InlayHintTooltip::String);
    Ok(result)
}

pub(crate) fn handle_code_action(
    snap: GlobalStateSnapshot,
    params: CodeActionParams,
) -> anyhow::Result<Option<Vec<crate::lsp::ext::CodeAction>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let ide_range = crate::from_proto::text_range(&line_index, params.range);
    let diagnostics: Vec<_> = params
        .context
        .diagnostics
        .iter()
        .map(|d| crate::from_proto::diagnostic(&line_index, d))
        .collect();
    let format = snap
        .last_format_options
        .lock()
        .map(|o| o.clone())
        .unwrap_or_else(|_| crate::code_action_helpers::default_code_action_format_options());

    // What the client supports and asked for, passed in rather than
    // defaulted: `AssistConfig` had six fields and no readers.
    // `snippet_cap` says whether an assist may leave a tabstop, so it is
    // the *client's* `experimental.snippetTextEdit` that decides it —
    // whether a workspace edit may carry snippet syntax. It used to be
    // read off `completion.snippets`, this server's own setting for
    // whether completion items expand, which answers a different question
    // and is not the client's answer to any.
    let assists = ide_assists::AssistConfig {
        file_creation: snap.client_caps.file_creation_support(),
        snippet_cap: ide_assists::SnippetCap::new(snap.client_caps.snippet_text_edit_support()),
        ..Default::default()
    };
    let source_fingerprint = crate::code_action_helpers::source_fingerprint(sf.text());
    let fix_all_kind = crate::code_action_helpers::sail_source_fix_all_kind();
    let actions: Vec<crate::lsp::ext::CodeAction> = snap
        .analysis
        .code_actions(uri, ide_range, &diagnostics, &format, &assists)?
        .into_iter()
        .filter_map(|action| {
            let kind = if action.is_fix_all {
                fix_all_kind.clone()
            } else {
                assist_kind_to_lsp(action.kind)
            };
            if !crate::code_action_helpers::code_action_kind_allowed(&params.context.only, &kind) {
                return None;
            }
            // The edit an assist produces may be a snippet — `$0` where
            // the cursor should land. A client that opted into
            // `experimental.snippetTextEdit` gets it; one that did not
            // gets the placeholders stripped, inside a plain edit.
            let url = |fid| snap.analysis.url_for_file_id(fid).cloned();
            let line_index =
                |fid| snap.analysis.url_for_file_id(fid).map(|uri| line_index_for(&snap, uri));
            let creates_files = !action.source_change.file_creations.is_empty();
            for creation in &action.source_change.file_creations {
                let ide_db::source_change::FileCreation { anchor, path, .. } = creation;
                let target = url(*anchor)?.join(path).ok()?;
                if snap.analysis.file_id(&target).is_some() || target.to_file_path().ok()?.exists()
                {
                    return None;
                }
            }
            let mut edit = if snap.client_caps.snippet_text_edit_support() && !creates_files {
                crate::lsp::ext::SnippetWorkspaceEdit {
                    document_changes: None,
                    changes: Some(crate::to_proto::snippet_workspace_edit(
                        &action.source_change,
                        &url,
                        &line_index,
                    )),
                }
            } else {
                let plain = crate::to_proto::workspace_edit(
                    &action.source_change,
                    &url,
                    &line_index,
                    false,
                );
                crate::to_proto::snippet_workspace_edit_from_plain(&plain)
            };
            if let Some(DocumentChanges::Operations(operations)) = &mut edit.document_changes {
                for operation in operations {
                    if let DocumentChangeOperation::Edit(edit) = operation {
                        edit.text_document.version =
                            snap.document_versions.get(&edit.text_document.uri).copied();
                    }
                }
            }
            Some(crate::lsp::ext::CodeAction {
                title: action.title,
                kind: Some(kind),
                diagnostics: action
                    .for_diagnostic
                    .and_then(|i| params.context.diagnostics.get(i))
                    .map(|d| vec![d.clone()]),
                edit: None,
                is_preferred: action.is_preferred,
                data: Some({
                    let mut data = if let Some(request) = action.import {
                        serde_json::json!({"autoInclude": crate::to_proto::import_request(&request)})
                    } else {
                        let mut data = crate::code_action_helpers::lazy_code_action_data(&edit);
                        if creates_files {
                            data["extractionSource"] = serde_json::json!({"uri": uri, "text": sf.text()});
                        }
                        data
                    };
                    data["sourceState"] = serde_json::json!({"uri": uri, "fingerprint": source_fingerprint});
                    data
                }),
            })
        })
        .collect();

    if actions.is_empty() {
        Ok(None)
    } else {
        Ok(Some(actions))
    }
}
/// An assist's kind, as the protocol names it.
fn assist_kind_to_lsp(kind: ide_db::assists::AssistKind) -> CodeActionKind {
    use ide_db::assists::AssistKind as K;
    match kind {
        K::QuickFix => CodeActionKind::QUICKFIX,
        K::Refactor | K::Generate => CodeActionKind::REFACTOR,
        K::RefactorExtract => CodeActionKind::REFACTOR_EXTRACT,
        K::RefactorInline => CodeActionKind::REFACTOR_INLINE,
        K::RefactorRewrite => CodeActionKind::REFACTOR_REWRITE,
        K::Source => CodeActionKind::SOURCE,
        K::None => CodeActionKind::EMPTY,
    }
}
pub(crate) fn handle_code_action_resolve(
    snap: GlobalStateSnapshot,
    mut params: crate::lsp::ext::CodeAction,
) -> anyhow::Result<crate::lsp::ext::CodeAction> {
    if let Some(source) = params.data.as_ref().and_then(|data| data.get("sourceState")) {
        let valid = source
            .get("uri")
            .and_then(|uri| serde_json::from_value::<Url>(uri.clone()).ok())
            .and_then(|uri| snap.analysis.file(&uri))
            .is_some_and(|file| {
                source.get("fingerprint").and_then(|value| value.as_str())
                    == Some(crate::code_action_helpers::source_fingerprint(file.text()).as_str())
            });
        if !valid {
            return Err(super::dispatch::RequestError {
                code: lsp_server::ErrorCode::ContentModified,
                message: "The source changed since the code action was requested",
            }
            .into());
        }
    }
    if let Some(source) = params.data.as_ref().and_then(|data| data.get("extractionSource")) {
        let valid = source
            .get("uri")
            .and_then(|uri| serde_json::from_value::<Url>(uri.clone()).ok())
            .and_then(|uri| snap.analysis.file(&uri))
            .is_some_and(|file| {
                source.get("text").and_then(|text| text.as_str()) == Some(file.text())
            });
        if !valid {
            return Err(super::dispatch::RequestError {
                code: lsp_server::ErrorCode::ContentModified,
                message: "The source changed since the extraction was requested",
            }
            .into());
        }
    }
    if let Some(request) = params
        .data
        .as_ref()
        .and_then(|data| data.get("autoInclude"))
        .and_then(crate::from_proto::import_request)
    {
        let Some(change) = snap.analysis.resolve_auto_include(&request)? else {
            return Err(super::dispatch::RequestError {
                code: lsp_server::ErrorCode::ContentModified,
                message: "The include candidate is no longer applicable in this source context",
            }
            .into());
        };
        let edit = crate::to_proto::workspace_edit(
            &change,
            &|id| snap.analysis.url_for_file_id(id).cloned(),
            &|id| snap.analysis.url_for_file_id(id).map(|uri| line_index_for(&snap, uri)),
            false,
        );
        params.edit = Some(crate::to_proto::snippet_workspace_edit_from_plain(&edit));
        return Ok(params);
    }
    if params.edit.is_none() {
        if let Some(data) = params.data.as_ref() {
            params.edit = crate::code_action_helpers::resolve_code_action_edit_from_data(data);
        }
    }
    if let Some(DocumentChanges::Operations(operations)) =
        params.edit.as_ref().and_then(|edit| edit.document_changes.as_ref())
    {
        for operation in operations {
            if let DocumentChangeOperation::Op(ResourceOp::Create(create)) = operation {
                if snap.analysis.file_id(&create.uri).is_some()
                    || create.uri.to_file_path().map_or(true, |path| path.exists())
                {
                    return Err(super::dispatch::RequestError {
                        code: lsp_server::ErrorCode::ContentModified,
                        message: "The extraction destination already exists",
                    }
                    .into());
                }
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
    let line_col = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
    if !snap.config.completion.enable {
        return Ok(None);
    }
    let config = snap.config.completion.to_ide_config();
    let ide_items = snap.analysis.completions(file_id, line_col, &config)?;
    let line_index = line_index_for(&snap, uri);
    if ide_items.is_empty() {
        return Ok(None);
    }
    // A list cut at `completion.limit` is only the best of the names
    // matching what is typed so far, so the client must ask again as the
    // prefix grows instead of filtering what it was given.
    let truncated = config.limit.is_some_and(|limit| ide_items.len() >= limit);
    let lsp_items: Vec<CompletionItem> = {
        let markdown = snap.client_caps.completion_markdown_support();
        let snippets =
            snap.config.completion.snippets && snap.client_caps.completion_snippet_support();
        ide_items
            .iter()
            .filter(|it| {
                it.import.is_none() || snap.client_caps.completion_import_resolve_support()
            })
            .map(|it| crate::to_proto::completion_item(it, snippets, markdown, Some(&line_index)))
            .collect()
    };
    Ok(Some(if truncated {
        CompletionResponse::List(lsp_types::CompletionList {
            is_incomplete: true,
            items: lsp_items,
        })
    } else {
        CompletionResponse::Array(lsp_items)
    }))
}

pub(crate) fn handle_resolve_completion_item(
    snap: GlobalStateSnapshot,
    item: CompletionItem,
) -> anyhow::Result<CompletionItem> {
    let mut ide_item = crate::from_proto::completion_item(&item);
    snap.analysis.resolve_completion_item(&mut ide_item)?;
    if ide_item.import.is_some() && ide_item.additional_text_edits.is_empty() {
        return Err(super::dispatch::RequestError {
            code: lsp_server::ErrorCode::ContentModified,
            message: "The include candidate is no longer applicable in this source context",
        }
        .into());
    }
    let index = ide_item
        .scope_file
        .and_then(|id| snap.analysis.url_for_file_id(id))
        .map(|uri| line_index_for(&snap, uri));
    let mut resolved = crate::to_proto::completion_item(
        &ide_item,
        snap.client_caps.completion_snippet_support(),
        snap.client_caps.completion_markdown_support(),
        index.as_ref(),
    );
    // Completion resolve supplies details and additional edits, not a newly
    // calculated replacement range against a possibly changed document.
    resolved.text_edit = item.text_edit;
    resolved.insert_text = item.insert_text;
    resolved.insert_text_format = item.insert_text_format;
    Ok(resolved)
}

pub(crate) fn handle_inlay_hint(
    snap: GlobalStateSnapshot,
    params: InlayHintParams,
) -> anyhow::Result<Option<Vec<InlayHint>>> {
    let uri = &params.text_document.uri;
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let range = crate::from_proto::text_range(&line_index, params.range);
    // The client's settings reach the feature. They used to be parsed into
    // `SailLspConfig`, converted by `to_ide_config`, and then dropped.
    if !snap.config.inlay_hints.enable {
        return Ok(None);
    }
    let hints =
        snap.analysis.inlay_hints(file_id, range, &snap.config.inlay_hints.to_ide_config())?;
    if hints.is_empty() {
        return Ok(None);
    }
    Ok(Some(hints.iter().map(|h| crate::to_proto::inlay_hint(&line_index, h)).collect()))
}

pub(crate) fn handle_signature_help(
    snap: GlobalStateSnapshot,
    params: SignatureHelpParams,
) -> anyhow::Result<Option<SignatureHelp>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let Some(ide_help) = snap.analysis.signature_help(file_id, lc)? else {
        return Ok(None);
    };
    let lsp_help = SignatureHelp {
        signatures: ide_help
            .signatures
            .into_iter()
            .map(|s| SignatureInformation {
                label: s.label,
                documentation: s.documentation.map(Documentation::String),
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
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);

    // The resolution chain lives in `ide` — this converts the answer.
    let navs = snap.analysis.clone().goto_definition(uri, lc)?;
    if navs.is_empty() {
        return Ok(None);
    }
    let lsp_locs = nav_targets_to_lsp(&navs, &snap.analysis);
    if lsp_locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDefinitionResponse::Array(lsp_locs)))
    }
}

pub(crate) fn handle_goto_declaration(
    snap: GlobalStateSnapshot,
    params: GotoDeclarationParams,
) -> anyhow::Result<Option<GotoDeclarationResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, uri),
        params.text_document_position_params.position,
    );
    let locs = snap.analysis.clone().goto_declaration(uri, lc)?;
    if locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoDeclarationResponse::Array(file_locations_to_lsp(&locs, &snap.analysis))))
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
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);

    let results = snap.analysis.find_all_refs(uri, lc, None)?;

    let Some(results) = results else {
        return Ok(None);
    };

    // Convert to LSP Location list
    let mut locations = Vec::new();
    for result in &results {
        // Include declaration if requested
        if params.context.include_declaration {
            for decl in &result.declarations {
                if let Some(decl_url) = snap.analysis.url_for_file_id(decl.file_id) {
                    if let Some(decl_file) = snap.analysis.file(decl_url) {
                        locations.push(Location {
                            uri: decl_url.clone(),
                            range: Range::new(
                                crate::to_proto::position_from_line_col(
                                    &line_index_for(&snap, decl_url),
                                    decl_file.position_at(base_db::range_start(decl.range)),
                                ),
                                crate::to_proto::position_from_line_col(
                                    &line_index_for(&snap, decl_url),
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
                                crate::to_proto::position_from_line_col(
                                    &line_index_for(&snap, ref_url),
                                    ref_file.position_at(base_db::range_start(*range)),
                                ),
                                crate::to_proto::position_from_line_col(
                                    &line_index_for(&snap, ref_url),
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
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);

    let source_change =
        snap.analysis.rename(uri, lc, &params.new_name)?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(source_change) = source_change else {
        return Ok(None);
    };
    Ok(Some(crate::to_proto::workspace_edit(
        &source_change,
        &|fid| snap.analysis.url_for_file_id(fid).cloned(),
        &|fid| {
            snap.analysis
                .url_for_file_id(fid)
                .and_then(|url| snap.analysis.file(url))
                .map(|f| ide_db::line_index::LineIndex::new(f.text()))
        },
        snap.client_caps.snippet_text_edit_support(),
    )))
}

pub(crate) fn handle_code_lens(
    snap: GlobalStateSnapshot,
    params: CodeLensParams,
) -> anyhow::Result<Option<Vec<CodeLens>>> {
    let uri = &params.text_document.uri;
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    if !snap.config.code_lens.enable {
        return Ok(None);
    }
    let config = ide::annotations::AnnotationConfig {
        annotate_references: snap.config.code_lens.references,
        annotate_impls: snap.config.code_lens.implementations,
        annotate_runnables: snap.config.code_lens.runnables,
        ..Default::default()
    };
    let Some(ide_lenses) = snap.analysis.code_lenses(file_id, &config).ok() else {
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
    if snap.analysis.file(uri).is_none() {
        return Ok(None);
    }
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
    let Some(ide_item) = snap.analysis.prepare_call_hierarchy(uri, lc)? else {
        return Ok(None);
    };
    let Some(target) = snap.analysis.file(&ide_item.url) else { return Ok(None) };
    let li = ide_db::line_index::LineIndex::new(target.text());
    Ok(Some(vec![crate::to_proto::call_hierarchy_item(&li, &ide_item)]))
}

pub(crate) fn handle_call_hierarchy_incoming(
    snap: GlobalStateSnapshot,
    params: CallHierarchyIncomingCallsParams,
) -> anyhow::Result<Option<Vec<CallHierarchyIncomingCall>>> {
    let calls: Vec<_> = snap
        .analysis
        .incoming_calls(params.item.data.as_ref().unwrap_or(&serde_json::Value::Null))?
        .into_iter()
        .filter_map(|(item, ranges)| {
            let sf = snap.analysis.file(&item.url)?;
            let li = ide_db::line_index::LineIndex::new(sf.text());
            Some(CallHierarchyIncomingCall {
                from: crate::to_proto::call_hierarchy_item(&li, &item),
                from_ranges: ranges.iter().map(|r| crate::to_proto::range(&li, *r)).collect(),
            })
        })
        .collect();
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
    let calls: Vec<_> = snap
        .analysis
        .outgoing_calls(params.item.data.as_ref().unwrap_or(&serde_json::Value::Null))?
        .into_iter()
        .filter_map(|(item, ranges)| {
            let sf = snap.analysis.file(&params.item.uri)?;
            let li = ide_db::line_index::LineIndex::new(sf.text());
            Some(CallHierarchyOutgoingCall {
                to: crate::to_proto::call_hierarchy_item(
                    &ide_db::line_index::LineIndex::new(snap.analysis.file(&item.url)?.text()),
                    &item,
                ),
                from_ranges: ranges.iter().map(|r| crate::to_proto::range(&li, *r)).collect(),
            })
        })
        .collect();
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
    let results: Vec<SymbolInformation> = snap
        .analysis
        .workspace_symbols(&params.query)?
        .into_iter()
        .filter_map(|nav| {
            let sf = snap.analysis.file(&nav.url)?;
            // The symbol's own file, not the request's — a workspace
            // search answers about files the client never named.
            let index = line_index_for(&snap, &nav.url);
            let p = crate::to_proto::position_from_line_col(
                &index,
                sf.position_at(base_db::range_start(nav.full_range)),
            );
            #[allow(deprecated)]
            Some(SymbolInformation {
                name: nav.name,
                kind: crate::lsp_ext::lsp_symbol_kind(nav.kind),
                tags: None,
                deprecated: None,
                location: Location::new(nav.url, Range::new(p, p)),
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
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
    let Some(result) = snap.analysis.hover(file_id, lc, &snap.config.hover.to_ide_config())? else {
        return Ok(None);
    };
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok(None);
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    Ok(Some(crate::to_proto::hover(
        &line_index,
        &result,
        snap.client_caps.hover_markdown_support(),
    )))
}

pub(crate) fn handle_goto_implementation(
    snap: GlobalStateSnapshot,
    params: GotoImplementationParams,
) -> anyhow::Result<Option<GotoImplementationResponse>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, uri),
        params.text_document_position_params.position,
    );
    let locs = snap.analysis.clone().goto_implementation(uri, lc)?;
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
    let edits = snap.analysis.will_rename_file_edits(&rename_pairs)?;
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
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, uri),
        params.text_document_position_params.position,
    );
    let locs = snap.analysis.goto_type_definition(uri, lc)?;
    if locs.is_empty() {
        Ok(None)
    } else {
        Ok(Some(GotoTypeDefinitionResponse::Array(file_locations_to_lsp(&locs, &snap.analysis))))
    }
}

pub(crate) fn handle_type_hierarchy_prepare(
    snap: GlobalStateSnapshot,
    params: TypeHierarchyPrepareParams,
) -> anyhow::Result<Option<Vec<TypeHierarchyItem>>> {
    let uri = &params.text_document_position_params.text_document.uri;
    let position = params.text_document_position_params.position;
    if snap.analysis.file(uri).is_none() {
        return Ok(None);
    }
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
    let Some(ide_item) = snap.analysis.prepare_type_hierarchy(uri, lc)? else {
        return Ok(None);
    };
    let Some(target) = snap.analysis.file(&ide_item.url) else { return Ok(None) };
    let li = ide_db::line_index::LineIndex::new(target.text());
    Ok(Some(vec![crate::to_proto::type_hierarchy_item(&li, &ide_item)]))
}

pub(crate) fn handle_type_hierarchy_supertypes(
    snap: GlobalStateSnapshot,
    params: TypeHierarchySupertypesParams,
) -> anyhow::Result<Option<Vec<TypeHierarchyItem>>> {
    let ide_items = snap
        .analysis
        .type_supertypes(params.item.data.as_ref().unwrap_or(&serde_json::Value::Null))?;
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
    let ide_items = snap
        .analysis
        .type_subtypes(params.item.data.as_ref().unwrap_or(&serde_json::Value::Null))?;
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
    Ok(snap
        .analysis
        .view_syntax_tree(&params.text_document.uri)
        .unwrap_or_else(|| "(file not found)".to_string()))
}

/// View HIR at cursor position.
/// Registered as `sail-lsp/viewHir`.
pub(crate) fn handle_view_hir(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewHirParams,
) -> anyhow::Result<String> {
    let lc = crate::from_proto::line_col(
        &line_index_for(&snap, &params.text_document.uri),
        params.position,
    );
    Ok(snap
        .analysis
        .view_hir(&params.text_document.uri, lc)?
        .unwrap_or_else(|| "(file not found)".to_string()))
}

/// View the ItemTree for a file.
/// Registered as `sail-lsp/viewItemTree`.
pub(crate) fn handle_view_item_tree(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewItemTreeParams,
) -> anyhow::Result<String> {
    Ok(snap
        .analysis
        .view_item_tree(&params.text_document.uri)?
        .unwrap_or_else(|| "(file not found)".to_string()))
}

/// Expand $include directives for a file.
/// Registered as `sail-lsp/expandInclude`.
pub(crate) fn handle_expand_include(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ExpandIncludeParams,
) -> anyhow::Result<Option<crate::lsp_ext::ExpandIncludeResult>> {
    let Some(result) = snap
        .analysis
        .clone()
        .with_include_graph_arc(snap.include_graph.clone())
        .expand_includes(&params.text_document.uri)?
    else {
        return Ok(None);
    };
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
    let annotations = snap.analysis.effect_annotations(uri)?;
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
    let counts = snap.analysis.status()?;

    let mut status = String::new();
    stdx::format_to!(status, "sail-lsp status\n");
    stdx::format_to!(status, "  files loaded:     {}\n", counts.files);
    stdx::format_to!(status, "  index symbols:    {}\n", counts.symbols);
    stdx::format_to!(status, "  include graph:    {} nodes\n", counts.include_nodes);
    stdx::format_to!(
        status,
        "  allocator heap:   {} MiB (estimate)\n",
        profile::MemoryUsage::now().allocated.megabytes()
    );
    Ok(status)
}

/// View include graph for a file.
/// Sail-specific debugging aid.
/// Registered as `sail-lsp/viewIncludeGraph`.
pub(crate) fn handle_view_include_graph(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ViewIncludeGraphParams,
) -> anyhow::Result<String> {
    if snap.analysis.file_id(&params.text_document.uri).is_none() {
        return Ok("(file not found)".to_string());
    }
    Ok(snap
        .analysis
        .clone()
        .with_include_graph_arc(snap.include_graph.clone())
        .render_include_graph()?)
}
