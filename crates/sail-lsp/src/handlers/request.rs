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

pub(crate) fn handle_runnables(
    snap: crate::global_state::GlobalStateSnapshot,
    params: crate::lsp_ext::RunnablesParams,
) -> anyhow::Result<Vec<crate::lsp_ext::Runnable>> {
    crate::runnables::discover(&snap, &params.text_document.uri)
}

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
    if crate::manifest_editing::is_manifest(uri) {
        let response = crate::manifest_editing::document_symbols(&snap, uri);
        return Ok(match response {
            Some(DocumentSymbolResponse::Nested(tree))
                if !snap.client_caps.hierarchical_symbols_support() =>
            {
                Some(DocumentSymbolResponse::Flat(flatten_symbols(&tree, uri)))
            }
            response => response,
        });
    }
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
    let options = crate::formatting::options(&snap, uri, None)?;
    let Some(edits) = snap.analysis.on_enter(uri, lc, &options.options)? else {
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
    let mut ranges = Vec::new();
    for pos in params.positions {
        let lc = crate::from_proto::line_col(&line_index, pos);
        if let Some(sr) = snap.analysis.selection_range(uri, lc)? {
            ranges.push(crate::to_proto::selection_range(&line_index, &sr));
        }
    }
    Ok(Some(ranges))
}

pub(crate) fn handle_formatting(
    snap: GlobalStateSnapshot,
    params: DocumentFormattingParams,
) -> anyhow::Result<Option<Vec<TextEdit>>> {
    let uri = &params.text_document.uri;
    if crate::manifest_editing::is_manifest(uri) {
        return Ok(crate::manifest_editing::formatting(&snap, uri));
    }
    let resolved = crate::formatting::options(&snap, uri, Some(&params.options))?;
    let Some(result) = crate::formatting::result(&snap, uri, &resolved.options, None)? else {
        return Ok(None);
    };
    if result.status != ide::formatting::FormatStatus::Changed {
        return Ok(None);
    }
    let Some(sf) = snap.analysis.file(uri) else { return Ok(None) };
    Ok(Some(ide_edits_to_lsp(result.edits, &sf)))
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
    let ide_tokens = snap.analysis.semantic_tokens(file_id)?;

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
    crate::diagnostics::extend_unique(&mut items, snap.check_diagnostics.for_file(file_id));

    // Related documents — files that $include this file.
    // When this file's types change, dependent files may get new errors.
    let supports_related = snap.client_caps.text_document_diagnostic_related_document_support();
    let mut related_documents: std::collections::HashMap<Url, DocumentDiagnosticReportKind> =
        std::collections::HashMap::new();

    if supports_related {
        for &dep_fid in
            snap.analysis.include_graph().into_iter().flat_map(|graph| graph.included_by(file_id))
        {
            if let Some(dep_url) = snap.analysis.url_for_file_id(dep_fid) {
                let config = snap.config_for_uri(dep_url).diagnostics.to_ide_config();
                let dep_diags = snap.analysis.file_diagnostics(dep_fid, &config)?;
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
                    crate::diagnostics::extend_unique(
                        &mut dep_items,
                        snap.check_diagnostics.for_file(dep_fid),
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
    let resolved = crate::formatting::options(&snap, uri, Some(&params.options))?;
    let Some(result) =
        crate::formatting::result(&snap, uri, &resolved.options, Some(params.range))?
    else {
        return Ok(None);
    };
    if result.status != ide::formatting::FormatStatus::Changed {
        return Ok(None);
    }
    Ok(Some(ide_edits_to_lsp(result.edits, &sf)))
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
    if let Some(links) = snap.preview_links.get(&params.text_document.uri) {
        return Ok(Some(links.clone()));
    }
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
    if crate::manifest_editing::is_manifest(uri) {
        return Ok(crate::manifest_editing::prepare_rename(&snap, uri, params.position).map(
            |(range, placeholder)| PrepareRenameResponse::RangeWithPlaceholder {
                range,
                placeholder,
            },
        ));
    }
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
    let ide_tokens = snap.analysis.semantic_tokens(file_id)?;

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
    snap: GlobalStateSnapshot,
    mut params: CodeLens,
) -> anyhow::Result<CodeLens> {
    let Some(data) = params.data.as_ref() else { return Ok(params) };
    let Some((title, sites)) = snap.analysis.resolve_code_lens(data)? else {
        return Ok(params);
    };
    let uri = data.get("uri").and_then(|v| v.as_str()).and_then(|s| Url::parse(s).ok());
    let locations: Vec<lsp_types::Location> = sites
        .into_iter()
        .filter_map(|(file_id, range)| {
            let url = snap.analysis.url_for_file_id(file_id)?.clone();
            let line_index = line_index_for(&snap, &url);
            Some(lsp_types::Location::new(url, crate::to_proto::range(&line_index, range)))
        })
        .collect();
    // `showReferences` with the sites the title counted; a lens with nothing
    // to show is a title alone.
    params.command = Some(match uri {
        Some(uri) if !locations.is_empty() => Command {
            title,
            command: "editor.action.showReferences".to_string(),
            arguments: Some(vec![
                serde_json::to_value(&uri)?,
                serde_json::to_value(params.range.start)?,
                serde_json::to_value(locations)?,
            ]),
        },
        _ => Command { title, command: String::new(), arguments: None },
    });
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
    let options = crate::formatting::options(&snap, uri, Some(&params.options))?;
    match snap.analysis.on_type_formatting(uri, lc, &params.ch, &options.options)? {
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
    let mut compiler_fixes = if crate::code_action_helpers::code_action_kind_allowed(
        &params.context.only,
        &lsp_types::CodeActionKind::QUICKFIX,
    ) {
        crate::flycheck::structured::quick_fixes(
            &snap,
            uri,
            params.range,
            &params.context.diagnostics,
        )
    } else {
        Vec::new()
    };
    if crate::manifest_editing::is_manifest(uri) {
        let quickfix = lsp_types::CodeActionKind::QUICKFIX;
        if !crate::code_action_helpers::code_action_kind_allowed(&params.context.only, &quickfix) {
            return Ok(None);
        }
        compiler_fixes.extend(crate::manifest_editing::diagnostic_fixes(
            &snap,
            uri,
            &params.context.diagnostics,
        ));
        return Ok((!compiler_fixes.is_empty()).then_some(compiler_fixes));
    }
    let Some(sf) = snap.analysis.file(uri) else {
        return Ok((!compiler_fixes.is_empty()).then_some(compiler_fixes));
    };
    let line_index = ide_db::line_index::LineIndex::new(sf.text());
    let ide_range = crate::from_proto::text_range(&line_index, params.range);
    let diagnostics: Vec<_> = params
        .context
        .diagnostics
        .iter()
        .map(|d| crate::from_proto::diagnostic(&line_index, d))
        .collect();
    let format = crate::formatting::options(&snap, uri, None)
        .map(|resolved| resolved.options)
        .map_err(|error| {
            tracing::debug!(%error, "Formatting source action unavailable");
            error
        })
        .ok();

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
    let mut actions: Vec<crate::lsp::ext::CodeAction> = snap
        .analysis
        .code_actions(uri, ide_range, &diagnostics, format.as_ref(), &assists)?
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
                    changes: Some(crate::to_proto::snippet_workspace_edit(
                        &action.source_change,
                        &url,
                        &line_index,
                    )),
                    ..Default::default()
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
                command: None,
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
    // A file no project loads is fixed in the manifest, not in the file.
    let quickfix = lsp_types::CodeActionKind::QUICKFIX;
    if crate::code_action_helpers::code_action_kind_allowed(&params.context.only, &quickfix) {
        let unlinked: Vec<_> = params
            .context
            .diagnostics
            .iter()
            .filter(|d| matches!(&d.code, Some(NumberOrString::String(c)) if c == "unlinked-file"))
            .cloned()
            .collect();
        actions.extend(crate::manifest_editing::add_to_project(&snap, uri, &unlinked));
    }

    actions.extend(compiler_fixes);
    actions.extend(crate::editor_commands::actions(&snap, &params)?);
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
    if let Some(key) = params.data.as_ref().and_then(|data| data.get("compilerFix")) {
        let Some(edit) = crate::flycheck::structured::resolve_fix(&snap, key) else {
            return Err(super::dispatch::RequestError {
                code: lsp_server::ErrorCode::ContentModified,
                message: "The compiler fix or its source changed; run the check again".into(),
            }
            .into());
        };
        params.edit = Some(edit);
        return Ok(params);
    }
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
                message: "The source changed since the code action was requested".into(),
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
                message: "The source changed since the extraction was requested".into(),
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
                message: "The include candidate is no longer applicable in this source context"
                    .into(),
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
                        message: "The extraction destination already exists".into(),
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
    if !snap.config.completion.enable {
        return Ok(None);
    }
    if crate::manifest_editing::is_manifest(uri) {
        let items = crate::manifest_editing::completion(&snap, uri, position);
        return Ok(items.map(CompletionResponse::Array));
    }
    let Some(file_id) = snap.analysis.file_id(uri) else {
        return Ok(None);
    };
    let line_col = crate::from_proto::line_col(&line_index_for(&snap, uri), position);
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
            message: "The include candidate is no longer applicable in this source context".into(),
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
    let mut indices = std::collections::HashMap::new();
    let mut output = Vec::with_capacity(hints.len());
    for hint in &hints {
        let mut converted = crate::to_proto::inlay_hint(&line_index, hint);
        if !hint.label_parts.is_empty() {
            let mut parts = Vec::new();
            for part in &hint.label_parts {
                let mut location = None;
                if let Some(target) = part.location {
                    if let Some(uri) = snap.analysis.url_for_file_id(target.file_id) {
                        if let std::collections::hash_map::Entry::Vacant(entry) =
                            indices.entry(target.file_id)
                        {
                            if let Some(file) = snap.analysis.files_ref().file_text(target.file_id)
                            {
                                entry.insert(ide_db::line_index::LineIndex::new(
                                    file.text(snap.analysis.db()),
                                ));
                            }
                        }
                        if let Some(index) = indices.get(&target.file_id) {
                            location = Some(Location {
                                uri: uri.clone(),
                                range: crate::to_proto::range(index, target.range),
                            });
                        }
                    }
                }
                parts.push(InlayHintLabelPart {
                    value: part.value.clone(),
                    tooltip: None,
                    location,
                    command: None,
                });
            }
            converted.label = InlayHintLabel::LabelParts(parts);
        }
        output.push(converted);
    }
    Ok(Some(output))
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
    if crate::manifest_editing::is_manifest(uri) {
        let found = crate::manifest_editing::goto_definition(&snap, uri, position);
        return Ok(found.filter(|l| !l.is_empty()).map(GotoDefinitionResponse::Array));
    }
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
    if crate::manifest_editing::is_manifest(uri) {
        return Ok(crate::manifest_editing::references(
            &snap,
            uri,
            position,
            params.context.include_declaration,
        )
        .filter(|locations| !locations.is_empty()));
    }
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
    if crate::manifest_editing::is_manifest(uri) {
        return crate::manifest_editing::rename(&snap, uri, position, &params.new_name);
    }
    let lc = crate::from_proto::line_col(&line_index_for(&snap, uri), position);

    let source_change =
        snap.analysis.rename(uri, lc, &params.new_name)?.map_err(|e| anyhow::anyhow!("{e}"))?;
    let Some(source_change) = source_change else {
        return Ok(None);
    };
    let edit = crate::to_proto::workspace_edit(
        &source_change,
        &|fid| snap.analysis.url_for_file_id(fid).cloned(),
        &|fid| {
            snap.analysis
                .url_for_file_id(fid)
                .and_then(|url| snap.analysis.file(url))
                .map(|f| ide_db::line_index::LineIndex::new(f.text()))
        },
        snap.client_caps.snippet_text_edit_support(),
    );
    Ok(Some(crate::to_proto::with_document_versions(
        edit,
        snap.client_caps.versioned_edit_support(),
        |uri| snap.document_versions.get(uri).copied(),
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
        ..Default::default()
    };
    let ide_lenses = snap.analysis.code_lenses(file_id, &config)?;
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
    use ide_db::symbol_index::Query;
    let mut words = Vec::new();
    let mut scope = "all";
    let mut project_filter = None;
    let mut kind = None;
    let mut types = false;
    let mut limit = 128usize;
    for word in params.query.split_whitespace() {
        if let Some(v) = word.strip_prefix("scope:") {
            scope = v;
        } else if let Some(v) = word.strip_prefix("project:") {
            project_filter = Some(v);
        } else if let Some(v) = word.strip_prefix("limit:") {
            limit = v.parse::<usize>().unwrap_or(128).min(1024);
        } else if let Some(v) = word.strip_prefix("kind:") {
            kind = Some(v);
        } else if let Some(v) = word.strip_prefix('#') {
            types = true;
            words.push(v);
        } else {
            words.push(word);
        }
    }
    let mut query = Query::new(words.join("")).limit(limit);
    if types {
        query = query.only_types();
    }
    if let Some(kind) = kind {
        let kind = match kind {
            "function" => hir::DefKind::Function,
            "val" => hir::DefKind::ValSpec,
            "struct" => hir::DefKind::Struct,
            "enum" => hir::DefKind::Enum,
            "union" => hir::DefKind::Union,
            "type" => hir::DefKind::TypeAlias,
            "mapping" => hir::DefKind::Mapping,
            "register" => hir::DefKind::Register,
            _ => anyhow::bail!("Unknown symbol kind: {kind}"),
        };
        query = query.kind(kind);
    }
    anyhow::ensure!(
        ["all", "workspace", "library"].contains(&scope),
        "Unknown symbol scope: {scope}"
    );
    let allowed: std::collections::HashSet<_> = snap
        .analysis
        .all_salsa_files()
        .into_iter()
        .filter_map(|(uri, _)| {
            let id = snap.analysis.file_id(&uri)?;
            let library = snap.is_library(id);
            if (scope == "workspace" && library) || (scope == "library" && !library) {
                return None;
            }
            if let Some(filter) = project_filter {
                if !snap.projects.values().any(|p| {
                    p.manifest.to_string_lossy().contains(filter)
                        && p.discovered_files.iter().any(|(u, _)| u == &uri)
                }) {
                    return None;
                }
            }
            Some(uri)
        })
        .collect();
    let mut seen = std::collections::HashSet::new();
    let results: Vec<SymbolInformation> = snap
        .analysis
        .workspace_symbols(&query, Some(&allowed))?
        .into_iter()
        .filter_map(|nav| {
            if !seen.insert((nav.url.clone(), nav.focus_range, nav.name.clone())) {
                return None;
            }
            let index = line_index_for(&snap, &nav.url);
            let path = nav.url.to_file_path().ok()?;
            let mut owners = Vec::new();
            for project in snap.projects.values() {
                if let Some(resolved) = &project.resolved {
                    for module in &resolved.modules {
                        if module.files.iter().any(|f| {
                            syntax::preprocess::normalize_path(
                                &project.manifest.parent().unwrap().join(f),
                            ) == path
                        }) {
                            owners.push(format!("{}::{}", project.manifest.display(), module.name));
                        }
                    }
                }
            }
            #[allow(deprecated)]
            Some(SymbolInformation {
                name: nav.name,
                kind: crate::lsp_ext::lsp_symbol_kind(nav.kind),
                tags: None,
                deprecated: None,
                location: Location::new(nav.url, crate::to_proto::range(&index, nav.focus_range)),
                container_name: Some(if owners.is_empty() {
                    path.display().to_string()
                } else {
                    format!("{} · {}", owners.join(", "), path.display())
                }),
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
    if crate::manifest_editing::is_manifest(uri) {
        return Ok(crate::manifest_editing::hover(
            &snap,
            uri,
            position,
            snap.client_caps.hover_markdown_support(),
        ));
    }
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
    let mut changes =
        snap.analysis.will_rename_file_edits(&rename_pairs)?.map_err(anyhow::Error::msg)?;
    let moves: Vec<_> = rename_pairs
        .iter()
        .filter_map(|(a, b)| {
            Some((
                Url::parse(a).ok()?.to_file_path().ok()?,
                Url::parse(b).ok()?.to_file_path().ok()?,
            ))
        })
        .collect();
    let mut documents = Vec::new();
    for project in snap.projects.values() {
        let uri = Url::from_file_path(&project.manifest)
            .map_err(|_| anyhow::anyhow!("Invalid manifest path"))?;
        let text = crate::manifest_editing::text(&snap, &uri)
            .ok_or_else(|| anyhow::anyhow!("Cannot read {uri}"))?;
        let parsed = project_model::ProjectManifest::parse(&text)
            .map_err(|e| anyhow::anyhow!("Cannot safely update {uri}: {}", e.message))?;
        let edits = parsed
            .rename_file_edits(
                &text,
                &project.manifest,
                &paths::renamed_path(&project.manifest, &moves),
                &crate::manifest_editing::project_options(&snap, &uri),
                &moves,
            )
            .map_err(anyhow::Error::msg)?;
        if !edits.is_empty() {
            let li = ide_db::line_index::LineIndex::new(&text);
            documents.push(lsp_types::TextDocumentEdit {
                text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                    uri: uri.clone(),
                    version: snap.document_versions.get(&uri).copied(),
                },
                edits: edits
                    .into_iter()
                    .map(|(r, new_text)| {
                        lsp_types::OneOf::Left(TextEdit {
                            range: crate::to_proto::range(&li, base_db::text_range(r.start, r.end)),
                            new_text,
                        })
                    })
                    .collect(),
            });
        }
    }
    for (uri, ide_edits) in changes.drain() {
        let Some(sf) = snap.analysis.file(&uri) else { continue };
        let li = ide_db::line_index::LineIndex::new(sf.text());
        documents.push(lsp_types::TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                version: snap.document_versions.get(&uri).copied(),
                uri,
            },
            edits: ide_edits
                .iter()
                .map(|e| lsp_types::OneOf::Left(crate::to_proto::text_edit(&li, e)))
                .collect(),
        });
    }
    if documents.is_empty() {
        return Ok(None);
    }
    documents.sort_by(|a, b| a.text_document.uri.cmp(&b.text_document.uri));
    Ok(Some(WorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(documents)),
        ..Default::default()
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
    let Some(result) = snap.analysis.expand_includes(&params.text_document.uri)? else {
        return Ok(None);
    };
    let context = result.context.as_ref().and_then(|key| {
        let root = snap.analysis.url_for_file_id(key.root)?;
        Some(include_context_key(key, root, &snap.module_contexts))
    });
    let project = result
        .context
        .as_ref()
        .and_then(|key| key.module)
        .and_then(|module| snap.module_contexts.get(module))
        .cloned();
    let expanded_index = ide_db::line_index::LineIndex::new(&result.expansion);
    let mappings = result
        .mappings
        .iter()
        .filter_map(|mapping| {
            let uri = snap.analysis.url_for_file_id(mapping.original.file_id)?.clone();
            let file = snap.analysis.file(&uri)?;
            let index = ide_db::line_index::LineIndex::new(file.text());
            Some(crate::lsp_ext::ExpansionMapping {
                range: crate::to_proto::range(&expanded_index, mapping.generated),
                origin: lsp_types::Location {
                    uri,
                    range: crate::to_proto::range(&index, mapping.original.range),
                },
                kind: mapping.kind.to_owned(),
            })
        })
        .collect();
    Ok(Some(crate::lsp_ext::ExpandIncludeResult {
        root: result.root,
        expansion: result.expansion,
        context,
        project,
        mappings,
    }))
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

/// The manifest entry that lists the file, else the entry of the file that
/// `$include`s it.
pub(crate) fn handle_open_project_manifest(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ProjectNavigationParams,
) -> anyhow::Result<Option<Location>> {
    let listed = |uri: &Url| {
        snap.projects.values().find_map(|p| {
            let (_, range) = p
                .declarations
                .iter()
                .flat_map(|m| &m.files)
                .find(|(f, _)| crate::from_proto::same_document(f, uri))?;
            Some(Location::new(Url::from_file_path(&p.manifest).ok()?, *range))
        })
    };
    let uri = params.text_document.uri;
    if let Some(entry) = listed(&uri) {
        return Ok(Some(entry));
    }
    // Not listed: the entry of the listed file whose `$include`s reach it.
    let mut seen = std::collections::HashSet::from([uri.clone()]);
    let mut pending = vec![uri];
    while let Some(file) = pending.pop() {
        for site in snap.analysis.include_sites(&file)? {
            if let Some(entry) = listed(&site.url) {
                return Ok(Some(entry));
            }
            if seen.insert(site.url.clone()) {
                pending.push(site.url);
            }
        }
    }
    Ok(None)
}

/// Module declarations listing the file, then `$include`s naming it.
pub(crate) fn handle_parent_module(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::ProjectNavigationParams,
) -> anyhow::Result<Vec<Location>> {
    let uri = params.text_document.uri;
    let mut out = Vec::new();
    for project in snap.projects.values() {
        let Ok(manifest) = Url::from_file_path(&project.manifest) else { continue };
        for module in &project.declarations {
            if module.files.iter().any(|(f, _)| crate::from_proto::same_document(f, &uri)) {
                out.push(Location::new(manifest.clone(), module.name_range));
            }
        }
    }
    out.extend(file_locations_to_lsp(&snap.analysis.include_sites(&uri)?, &snap.analysis));
    Ok(out)
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
    stdx::format_to!(status, "  version:          {}\n", crate::version::version());
    stdx::format_to!(
        status,
        "  executable:       {}\n",
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|e| format!("unavailable: {e}"))
    );
    let mut workspace = snap.workspace_status;
    workspace.roots.sort();
    workspace.manifests.sort();
    stdx::format_to!(
        status,
        "  workspace loaded: {}\n  reload pending:   {}\n",
        workspace.loaded,
        workspace.loading
    );
    stdx::format_to!(status, "  workspace roots:\n");
    for root in workspace.roots {
        stdx::format_to!(status, "    {root}\n");
    }
    stdx::format_to!(status, "  loaded manifests:\n");
    for manifest in workspace.manifests {
        stdx::format_to!(status, "    {}\n", manifest.display());
    }
    stdx::format_to!(
        status,
        "  standard library: {} ({})\n",
        workspace.sail_dir.map(|p| p.display().to_string()).unwrap_or_else(|| "unavailable".into()),
        workspace.sail_dir_origin
    );
    if let Some(error) = snap.project_error {
        stdx::format_to!(
            status,
            "  project load error (previous model may be retained): {error}\n"
        );
    }
    if let Some(error) = snap.watcher_error {
        stdx::format_to!(status, "  file watching degraded: {error}; background rescans and recovery retries are active\n");
    }
    for error in workspace.config_errors {
        stdx::format_to!(status, "  configuration error: {error}\n");
    }
    stdx::format_to!(
        status,
        "  effective settings (project changes require a successful reload):\n{}\n",
        serde_json::to_string_pretty(&snap.config.diagnostic_json())?
    );

    stdx::format_to!(
        status,
        "  native analysis health:\n{}\n",
        serde_json::to_string_pretty(snap.analysis_health.as_ref())?
    );
    stdx::format_to!(status, "  compiler checks:\n");
    for (target, check) in snap.flycheck_status {
        stdx::format_to!(status, "    {target:?}: {check:?}\n");
        if let Some(history) = snap.flycheck_history.get(&target) {
            stdx::format_to!(status, "      {}\n", history.summary());
        }
    }
    let memory = counts.memory;
    stdx::format_to!(status, "  retained physical file inputs: {}\n", memory.physical_files);
    stdx::format_to!(status, "  retained semantic view inputs: {}\n", memory.semantic_views);
    stdx::format_to!(status, "  unique retained text bytes: {}\n", memory.text_bytes);
    stdx::format_to!(
        status,
        "  additional parse seed text bytes: {}\n",
        memory.seed_only_text_bytes
    );
    stdx::format_to!(
        status,
        "  parse seeds: {} ({} live weak tree references)\n",
        memory.parse_seeds,
        memory.live_seed_trees
    );
    stdx::format_to!(
        status,
        "  live analysis snapshots (including this request): {}\n",
        counts.live_snapshots
    );
    stdx::format_to!(status, "  Cache accounting is passive; text bytes exclude trees, inference, indexes and allocator overhead.\n");
    #[cfg(feature = "z3-solver")]
    {
        let (entries, capacity, live, total) = hir::z3_solver::cache_ownership();
        stdx::format_to!(
            status,
            "  Z3 process-wide estimated allocations: {} bytes (subset of RSS)\n",
            hir::z3_solver::estimated_native_bytes()
        );
        stdx::format_to!(
            status,
            "  solver result cache entries: {:?} / {} (None = uninitialized or busy)\n",
            entries,
            capacity
        );
        stdx::format_to!(
            status,
            "  solver threads: {} live / {} ever initialized (native context owners, not bytes)\n",
            live,
            total
        );
    }
    stdx::format_to!(
        status,
        "  process RSS: {:?} bytes\n  query cache policy: {}\n",
        profile::process_resident_bytes(),
        snap.cache_control.report()
    );
    stdx::format_to!(
        status,
        "  allocator heap:   {} MiB (estimate)\n",
        profile::MemoryUsage::now().allocated.megabytes()
    );
    Ok(status)
}

/// Export metadata without starting inference, compiling Sail or reading buffer contents.
pub(crate) fn handle_diagnostic_report(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::DiagnosticReportParams,
) -> anyhow::Result<serde_json::Value> {
    let active_file = if let Some(document) = params.text_document {
        let contexts: Vec<_> = snap.analysis.include_contexts(&document.uri)?.into_iter().map(|(_, key, root)| {
            serde_json::json!({"key":include_context_key(&key,&root,&snap.module_contexts),"root":root,
                "project":key.module.and_then(|index|snap.module_contexts.get(index))})
        }).collect();
        Some(
            serde_json::json!({"uri":document.uri,"version":snap.document_versions.get(&document.uri),"contexts":contexts}),
        )
    } else {
        None
    };
    let projects: Vec<_> = snap
        .projects
        .values()
        .map(|project| {
            serde_json::json!({
                "manifest":project.manifest,"configuration":project.config,
            })
        })
        .collect();
    let status = handle_sail_lsp_status(snap.clone(), ())?;
    Ok(serde_json::json!({
        "schemaVersion":1,"build":crate::version::metadata(),"executable":std::env::current_exe().ok(),
        "platform":{"os":std::env::consts::OS,"arch":std::env::consts::ARCH},
        "serverStatus":snap.server_status,"nativeAnalysis":snap.analysis_health.as_ref(),
        "compilerChecks":snap.flycheck_history.iter().map(|(target,history)|serde_json::json!({"target":format!("{target:?}"),"history":history})).collect::<Vec<_>>(),
        "queryCache":snap.cache_control.report(),"processRssBytes":profile::process_resident_bytes(),
        "fileWatching":{"error":snap.watcher_error.as_deref()},
        "workspace":{"roots":snap.workspace_status.roots,"loaded":snap.workspace_status.loaded,
            "projects":projects,"loadError":snap.project_error.as_deref()},
        "effectiveConfiguration":snap.config.diagnostic_json(),
        "folderConfigurations":snap.scoped_configs.iter().map(|(root,config)|serde_json::json!({"root":root,"configuration":config.diagnostic_json()})).collect::<Vec<_>>(),
        "activeFile":active_file,"analyzerStatus":status,
        "unknownAccounting":"Reasons in the latest published semantic diagnostics; suppressed diagnostics are excluded",
    }))
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
    Ok(snap.analysis.render_include_graph()?)
}

/// Return semantic structural matches as read-only source locations.
pub(crate) fn handle_ssr_search(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::SsrSearchParams,
) -> anyhow::Result<Vec<lsp_types::Location>> {
    let uri = &params.position.text_document.uri;
    let id = snap
        .analysis
        .file_id(uri)
        .ok_or_else(|| anyhow::anyhow!("SSR context file is unavailable"))?;
    let index = line_index_for(&snap, uri);
    let position = crate::from_proto::line_col(&index, params.position.position);
    let selections: Vec<_> = params
        .selections
        .into_iter()
        .map(|range| crate::from_proto::text_range(&index, range))
        .collect();
    let excluded = snap
        .analysis
        .all_salsa_files()
        .into_iter()
        .filter_map(|(uri, _)| {
            let id = snap.analysis.file_id(&uri)?;
            snap.is_library(id).then_some(id)
        })
        .collect();
    let matches = snap
        .analysis
        .structural_search(id, position, &params.query, &selections, &excluded)?
        .map_err(|message| super::dispatch::RequestError {
            code: lsp_server::ErrorCode::InvalidParams,
            message,
        })?;
    Ok(matches
        .into_iter()
        .filter_map(|found| {
            let uri = snap.analysis.url_for_file_id(found.file_id)?.clone();
            let index = line_index_for(&snap, &uri);
            Some(lsp_types::Location::new(uri, crate::to_proto::range(&index, found.range)))
        })
        .collect())
}

/// SSR returns versioned preview edits. The client decides whether to apply them.
pub(crate) fn handle_ssr(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::SsrParams,
) -> anyhow::Result<lsp_types::WorkspaceEdit> {
    let uri = &params.position.text_document.uri;
    let id = snap
        .analysis
        .file_id(uri)
        .ok_or_else(|| anyhow::anyhow!("SSR context file is unavailable"))?;
    let index = line_index_for(&snap, uri);
    let position = crate::from_proto::line_col(&index, params.position.position);
    let selections: Vec<_> = params
        .selections
        .into_iter()
        .map(|range| crate::from_proto::text_range(&index, range))
        .collect();
    let excluded = snap
        .analysis
        .all_salsa_files()
        .into_iter()
        .filter_map(|(uri, _)| {
            let id = snap.analysis.file_id(&uri)?;
            snap.is_library(id).then_some(id)
        })
        .collect();
    let change = snap
        .analysis
        .structural_search_replace(
            id,
            position,
            &params.query,
            params.parse_only,
            &selections,
            &excluded,
        )?
        .map_err(|e| super::dispatch::RequestError {
            code: lsp_server::ErrorCode::InvalidParams,
            message: e,
        })?;
    let plain = crate::to_proto::workspace_edit(
        &change,
        &|id| snap.analysis.url_for_file_id(id).cloned(),
        &|id| snap.analysis.url_for_file_id(id).map(|uri| line_index_for(&snap, uri)),
        false,
    );
    let changes = plain
        .changes
        .unwrap_or_default()
        .into_iter()
        .map(|(uri, edits)| lsp_types::TextDocumentEdit {
            text_document: lsp_types::OptionalVersionedTextDocumentIdentifier {
                version: snap.document_versions.get(&uri).copied(),
                uri,
            },
            edits: edits.into_iter().map(lsp_types::OneOf::Left).collect(),
        })
        .collect();
    Ok(lsp_types::WorkspaceEdit {
        document_changes: Some(lsp_types::DocumentChanges::Edits(changes)),
        ..Default::default()
    })
}

pub(crate) fn handle_project_graph_sync(
    state: &mut crate::global_state::GlobalState,
    _: (),
) -> anyhow::Result<serde_json::Value> {
    project_graph(&state.projects, &state.config.workspace)
}

fn project_graph(
    loaded: &std::collections::BTreeMap<lsp_types::Url, crate::reload::FolderProject>,
    workspace: &crate::config::WorkspaceConfig,
) -> anyhow::Result<serde_json::Value> {
    let mut projects: Vec<_> = loaded.values().collect();
    projects.sort_by_key(|p| {
        let selected = workspace.active_project.as_ref().is_some_and(|a| {
            syntax::preprocess::normalize_path(&p.workspace_root.join(a)) == p.manifest
        });
        (!selected, p.manifest.clone())
    });
    Ok(serde_json::Value::Array(projects.into_iter().enumerate().map(|(index, project)| {
        let modules: Vec<_> = project.resolved.as_ref().into_iter().flat_map(|r| r.modules.iter().enumerate().map(move |(id, m)| {
            serde_json::json!({"name": m.name, "active": r.module_order.contains(&id),
                "requires": m.requires.iter().map(|&id| &r.modules[id].name).collect::<Vec<_>>(),
                "files": m.files.iter().filter_map(|f| lsp_types::Url::from_file_path(project.manifest.parent().unwrap().join(f)).ok()).collect::<Vec<_>>()})
        })).collect();
        serde_json::json!({"manifest": lsp_types::Url::from_file_path(&project.manifest).ok(), "path": project.manifest,
            "preferred": index == 0, "variables": project.config.project_variables, "roots": project.config.project_modules, "modules": modules})
    }).collect()))
}

pub(crate) fn handle_explain_analysis(
    snap: GlobalStateSnapshot,
    params: lsp_types::TextDocumentPositionParams,
) -> anyhow::Result<serde_json::Value> {
    let uri = params.text_document.uri;
    let Some(file) = snap.analysis.file_id(&uri) else { return Ok(serde_json::Value::Null) };
    let position = crate::from_proto::line_col(&line_index_for(&snap, &uri), params.position);
    let mut result = snap.analysis.explain_position(file, position)?;
    if result.is_null() {
        return Ok(result);
    }
    if let Some(contexts) = result["contexts"].as_array_mut() {
        for context in contexts {
            context["project"] = context["moduleIndex"]
                .as_u64()
                .and_then(|index| snap.module_contexts.get(index as usize))
                .cloned()
                .unwrap_or_default();
        }
    }
    result["projectLoadError"] = serde_json::to_value(snap.project_error.as_deref())?;
    result["reloadPending"] = serde_json::json!(snap.workspace_status.loading);
    result["unsavedManifestPolicy"] = serde_json::json!(
        "Project configuration takes effect after saving and a successful reload"
    );
    Ok(result)
}

/// The module's identity includes its manifest and name, not its transient index.
/// Root/occurrence/include ordinals survive ordinary source edits.
pub(crate) fn include_context_key(
    key: &base_db::preprocess::InstanceKey,
    root: &Url,
    modules: &[serde_json::Value],
) -> String {
    let module = key
        .module
        .and_then(|i| modules.get(i))
        .map(|m| serde_json::json!({"manifest":m["manifest"],"module":m["module"]}));
    serde_json::json!({"root":root,"occurrence":key.occurrence,"includes":key.includes,"module":module}).to_string()
}

pub(crate) fn handle_include_contexts(
    state: &mut crate::global_state::GlobalState,
    params: lsp_types::TextDocumentIdentifier,
) -> anyhow::Result<serde_json::Value> {
    let analysis = state.analysis_snapshot();
    let selected = state.mem_docs.get(&params.uri).and_then(|d| d.selected_context.as_ref());
    let contexts = analysis.include_contexts(&params.uri)?.into_iter().map(|(_,key,root)| {
        let identity = include_context_key(&key,&root,&state.module_contexts);
        serde_json::json!({"key":identity,"root":root,"includes":key.includes,
            "project":key.module.and_then(|i|state.module_contexts.get(i)),"selected":selected == Some(&identity)})
    }).collect::<Vec<_>>();
    Ok(serde_json::json!(contexts))
}

pub(crate) fn handle_select_include_context(
    state: &mut crate::global_state::GlobalState,
    params: crate::lsp_ext::SelectIncludeContextParams,
) -> anyhow::Result<()> {
    if let Some(selected) = &params.context {
        let contexts = state.analysis_snapshot().include_contexts(&params.text_document.uri)?;
        if !contexts.iter().any(|(_, key, root)| {
            include_context_key(key, root, &state.module_contexts) == *selected
        }) {
            return Err(super::dispatch::RequestError {
                code: lsp_server::ErrorCode::InvalidParams,
                message: "Include context is no longer active; select it again".into(),
            }
            .into());
        }
    }
    let document = state
        .mem_docs
        .get_mut(&params.text_document.uri)
        .ok_or_else(|| anyhow::anyhow!("Open the document before selecting a context"))?;
    document.selected_context = params.context;
    state.analysis_host.trigger_cancellation();
    Ok(())
}

pub(crate) fn handle_ssr_preview(
    snap: GlobalStateSnapshot,
    params: crate::lsp_ext::SsrParams,
) -> anyhow::Result<serde_json::Value> {
    let edit = handle_ssr(snap.clone(), params)?;
    let mut documents = Vec::new();
    if let Some(lsp_types::DocumentChanges::Edits(edits)) = &edit.document_changes {
        for doc in edits {
            let file = snap
                .analysis
                .file(&doc.text_document.uri)
                .ok_or_else(|| anyhow::anyhow!("SSR source disappeared"))?;
            documents.push(serde_json::json!({"uri":doc.text_document.uri,"version":doc.text_document.version,"text":file.text()}));
        }
    }
    Ok(serde_json::json!({"edit":edit,"documents":documents}))
}
