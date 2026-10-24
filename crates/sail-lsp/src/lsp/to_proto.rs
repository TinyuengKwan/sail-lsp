//! Internal types → LSP wire types.
//!
//! Single conversion boundary. All IDE crates use internal types;
//! this module translates them for the LSP wire protocol.

use ide::hover::HoverResult;
use ide::inlay_hints::{InlayHint, InlayHintKind};
use ide::syntax_highlighting::{HlRange, HlRanges};
use ide::CompletionItem;
use ide::NavigationTarget;
use ide_db::defs::CompletionItemKind;
use ide_db::ide_types::*;
use ide_db::line_index::{LineIndex, TextRange};
use ide_diagnostics::{Diagnostic, Severity};
#[allow(unused_imports)]
use lsp_types::{self, Position, Range, Url};

// There is no `position(lc)`. It took a `LineCol` and returned a
// `Position` by assignment, with a doc comment saying "prefer
// `position_in`" — and its one caller passed `line_index.line_col(..)`,
// which is a *byte* column, which is the case the comment warned about.
// A conversion that needs a `LineIndex` cannot be called without one.

/// The LSP position of a byte offset, with the column converted.
///
/// LSP's default `positionEncoding` is UTF-16 and this server negotiates
/// nothing else, so every column leaving here has to be converted or every
/// range on a line holding a non-ASCII character lands in the wrong place.
pub fn position_in(line_index: &LineIndex, offset: usize) -> Position {
    let lc = line_index.line_col(offset);
    Position::new(lc.line, line_index.byte_col_to_utf16(lc.line, lc.col))
}

/// An internal `LineCol` — byte column — as an LSP `Position`.
///
/// The counterpart of `from_proto::line_col`. `lsp::ext` had a version
/// that returned `Position::new(lc.line, lc.col)`, sending a byte column
/// where the client reads a UTF-16 one.
pub fn position_from_line_col(line_index: &LineIndex, lc: ide_db::LineCol) -> Position {
    Position::new(lc.line, line_index.byte_col_to_utf16(lc.line, lc.col))
}

pub fn range(line_index: &LineIndex, r: TextRange) -> Range {
    Range::new(
        position_in(line_index, base_db::range_start(r)),
        position_in(line_index, base_db::range_end(r)),
    )
}

pub fn text_edit(line_index: &LineIndex, edit: &IdeTextEdit) -> lsp_types::TextEdit {
    lsp_types::TextEdit { range: range(line_index, edit.range), new_text: edit.new_text.clone() }
}

/// No caller yet. The module is `pub` for `examples/protocol_check.rs`, so
/// the compiler no longer flags this and cannot retire the waiver either;
/// it is here because `text_edit` is, and a caller wanting the plural
/// should not have to add it back.
pub fn text_edits(line_index: &LineIndex, edits: &[IdeTextEdit]) -> Vec<lsp_types::TextEdit> {
    edits.iter().map(|e| text_edit(line_index, e)).collect()
}

/// No caller yet; see `text_edits`.
pub fn location(loc: &FileLocation, line_index: &LineIndex) -> lsp_types::Location {
    lsp_types::Location::new(loc.url.clone(), range(line_index, loc.range))
}

pub fn symbol_kind(kind: SymbolKind) -> lsp_types::SymbolKind {
    match kind {
        SymbolKind::Function => lsp_types::SymbolKind::FUNCTION,
        SymbolKind::Variable => lsp_types::SymbolKind::VARIABLE,
        SymbolKind::Constant => lsp_types::SymbolKind::CONSTANT,
        SymbolKind::TypeParameter => lsp_types::SymbolKind::TYPE_PARAMETER,
        SymbolKind::Struct => lsp_types::SymbolKind::STRUCT,
        SymbolKind::Enum => lsp_types::SymbolKind::ENUM,
        SymbolKind::EnumMember => lsp_types::SymbolKind::ENUM_MEMBER,
        SymbolKind::Module => lsp_types::SymbolKind::MODULE,
        SymbolKind::Field => lsp_types::SymbolKind::FIELD,
        SymbolKind::Property => lsp_types::SymbolKind::PROPERTY,
        SymbolKind::Event => lsp_types::SymbolKind::EVENT,
        SymbolKind::Operator => lsp_types::SymbolKind::OPERATOR,
        SymbolKind::TypeAlias => lsp_types::SymbolKind::TYPE_PARAMETER,
        SymbolKind::Other => lsp_types::SymbolKind::NULL,
    }
}

pub fn document_symbol(
    line_index: &LineIndex,
    target: &NavigationTarget,
) -> lsp_types::DocumentSymbol {
    #[allow(deprecated)]
    lsp_types::DocumentSymbol {
        name: target.name.clone(),
        detail: target.detail.clone(),
        kind: symbol_kind(target.kind),
        tags: None,
        deprecated: None,
        range: range(line_index, target.full_range),
        selection_range: range(line_index, target.focus_range),
        children: if target.children.is_empty() {
            None
        } else {
            Some(target.children.iter().map(|c| document_symbol(line_index, c)).collect())
        },
    }
}

/// Convert IDE hover to LSP Hover.
/// HoverActions are available in the `HoverResult.actions` field for
/// clients that support custom commands (e.g., "Go to implementations").
/// Standard LSP Hover only carries content + range; actions require
/// client-side extensions to surface.
///
/// `markdown` is `ClientCapabilities::hover_markdown_support`. A client
/// states the formats it takes in `textDocument.hover.contentFormat`, and
/// the protocol's default when it says nothing is plaintext only — this
/// sent Markdown either way, so a client without it rendered `**bold**`
/// and the backticks as themselves. `doc_to_plain_text` was here for that
/// and had no caller.
pub fn hover(line_index: &LineIndex, h: &HoverResult, markdown: bool) -> lsp_types::Hover {
    let (kind, value) = if markdown {
        (lsp_types::MarkupKind::Markdown, h.markup.clone())
    } else {
        (lsp_types::MarkupKind::PlainText, ide::hover::doc_to_plain_text(&h.markup))
    };
    lsp_types::Hover {
        contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent { kind, value }),
        range: Some(range(line_index, h.range)),
    }
}

/// Convert a `SourceChange` to an LSP `WorkspaceEdit`.
/// via the provided lookup closure, then converts each file's
/// TextEdits to LSP TextEdits using line indices.
/// handlers producing SourceChange can share this conversion.
/// A `SourceChange` as the protocol wants it.
///
/// `snippets` says whether the client understands a `SnippetTextEdit`
/// (`experimental.snippetTextEdit`). A change that carries snippet syntax
/// and a client that does not understand it means the placeholders are
/// stripped — `mk_S($0)` arrives as `mk_S()` rather than literally with a
/// `$0` in it.
pub fn workspace_edit(
    source_change: &ide_db::source_change::SourceChange,
    file_url: &dyn Fn(base_db::FileId) -> Option<Url>,
    file_line_index: &dyn Fn(base_db::FileId) -> Option<LineIndex>,
    snippets: bool,
) -> lsp_types::WorkspaceEdit {
    let mut changes: std::collections::HashMap<Url, Vec<lsp_types::TextEdit>> =
        std::collections::HashMap::new();

    for (&file_id, edits) in &source_change.source_file_edits {
        let Some(url) = file_url(file_id) else {
            continue;
        };
        let Some(li) = file_line_index(file_id) else {
            continue;
        };
        let lsp_edits: Vec<lsp_types::TextEdit> = edits
            .iter()
            .map(|e| {
                let mut edit = text_edit(&li, e);
                if source_change.is_snippet && !snippets {
                    edit.new_text = strip_snippet_placeholders(&edit.new_text);
                }
                edit
            })
            .collect();
        changes.insert(url, lsp_edits);
    }

    lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() }
}

/// The same change, as the snippet edits a client that opted in can take.
///
/// Kept beside `workspace_edit` rather than folded into it because the
/// protocol has no place for a snippet in a `WorkspaceEdit`: this is
/// rust-analyzer's extension, and it travels in the `data` a code action
/// carries until the client resolves it.
/// A plain workspace edit as the snippet-carrying shape, with every edit
/// marked plain text. The wire type is the same either way; what differs
/// is whether the client was told to expand what it receives.
pub fn snippet_workspace_edit_from_plain(
    edit: &lsp_types::WorkspaceEdit,
) -> super::ext::SnippetWorkspaceEdit {
    super::ext::SnippetWorkspaceEdit {
        changes: edit.changes.as_ref().map(|changes| {
            changes
                .iter()
                .map(|(url, edits)| {
                    let edits = edits
                        .iter()
                        .map(|e| super::ext::SnippetTextEdit {
                            range: e.range,
                            new_text: e.new_text.clone(),
                            insert_text_format: Some(lsp_types::InsertTextFormat::PLAIN_TEXT),
                        })
                        .collect();
                    (url.clone(), edits)
                })
                .collect()
        }),
    }
}

pub fn snippet_workspace_edit(
    source_change: &ide_db::source_change::SourceChange,
    file_url: &dyn Fn(base_db::FileId) -> Option<Url>,
    file_line_index: &dyn Fn(base_db::FileId) -> Option<LineIndex>,
) -> std::collections::HashMap<Url, Vec<super::ext::SnippetTextEdit>> {
    let format = if source_change.is_snippet {
        lsp_types::InsertTextFormat::SNIPPET
    } else {
        lsp_types::InsertTextFormat::PLAIN_TEXT
    };
    let mut changes = std::collections::HashMap::new();
    for (&file_id, edits) in &source_change.source_file_edits {
        let (Some(url), Some(li)) = (file_url(file_id), file_line_index(file_id)) else {
            continue;
        };
        changes.insert(
            url,
            edits
                .iter()
                .map(|e| {
                    let edit = text_edit(&li, e);
                    super::ext::SnippetTextEdit {
                        range: edit.range,
                        new_text: edit.new_text,
                        insert_text_format: Some(format),
                    }
                })
                .collect(),
        );
    }
    changes
}

pub fn diagnostic_severity(sev: Severity) -> lsp_types::DiagnosticSeverity {
    match sev {
        Severity::Error => lsp_types::DiagnosticSeverity::ERROR,
        Severity::Warning | Severity::WeakWarning => lsp_types::DiagnosticSeverity::WARNING,
        Severity::Information => lsp_types::DiagnosticSeverity::INFORMATION,
        Severity::Hint => lsp_types::DiagnosticSeverity::HINT,
    }
}

/// Convert IDE diagnostic to LSP wire format.
/// Maps `Diagnostic.unused` to `DiagnosticTag::UNNECESSARY`.
pub fn diagnostic(
    line_index: &LineIndex,
    url: &lsp_types::Url,
    d: &Diagnostic,
) -> lsp_types::Diagnostic {
    let mut tags = Vec::new();
    if d.unused {
        tags.push(lsp_types::DiagnosticTag::UNNECESSARY);
    }
    // Secondary places, each with what it contributes. Upstream draws a
    // caret at every one; this used to be appended to the message as
    // `constraint from 110..111`, two integers the reader cannot act on.
    // The spans are this file's — inference rebases them against the source
    // map that produced them — so one line index serves.
    let related: Vec<lsp_types::DiagnosticRelatedInformation> = d
        .related
        .iter()
        .filter(|(r, _)| r.file_id == d.range.file_id)
        .map(|(r, message)| lsp_types::DiagnosticRelatedInformation {
            location: lsp_types::Location { uri: url.clone(), range: range(line_index, r.range) },
            message: message.clone(),
        })
        .collect();
    lsp_types::Diagnostic {
        range: range(line_index, d.range.range),
        severity: Some(diagnostic_severity(d.severity)),
        code: Some(lsp_types::NumberOrString::String(d.code.as_str().to_string())),
        message: d.message.clone(),
        tags: if tags.is_empty() { None } else { Some(tags) },
        related_information: (!related.is_empty()).then_some(related),
        ..Default::default()
    }
}

/// Resolve secondary locations against their own physical source and line
/// index. Scattered declarations and ends may be in different include files.
pub fn diagnostic_with_analysis(
    analysis: &ide::Analysis,
    line_index: &LineIndex,
    url: &lsp_types::Url,
    d: &Diagnostic,
) -> lsp_types::Diagnostic {
    let mut converted = diagnostic(line_index, url, d);
    let related: Vec<_> = d
        .related
        .iter()
        .filter_map(|(r, message)| {
            let location = if r.file_id == d.range.file_id {
                lsp_types::Location { uri: url.clone(), range: range(line_index, r.range) }
            } else {
                let uri = analysis.url_for_file_id(r.file_id)?.clone();
                let file = analysis.file_by_id(r.file_id)?;
                let index = LineIndex::new(hir::callgraph::SourceFileInfo::text(&file));
                lsp_types::Location { uri, range: range(&index, r.range) }
            };
            Some(lsp_types::DiagnosticRelatedInformation { location, message: message.clone() })
        })
        .collect();
    converted.related_information = (!related.is_empty()).then_some(related);
    converted
}

pub fn completion_item_kind(kind: CompletionItemKind) -> lsp_types::CompletionItemKind {
    match kind {
        CompletionItemKind::Function => lsp_types::CompletionItemKind::FUNCTION,
        CompletionItemKind::Variable => lsp_types::CompletionItemKind::VARIABLE,
        CompletionItemKind::Keyword => lsp_types::CompletionItemKind::KEYWORD,
        CompletionItemKind::Snippet => lsp_types::CompletionItemKind::SNIPPET,
        CompletionItemKind::Field => lsp_types::CompletionItemKind::FIELD,
        CompletionItemKind::EnumMember => lsp_types::CompletionItemKind::ENUM_MEMBER,
        CompletionItemKind::TypeParameter => lsp_types::CompletionItemKind::TYPE_PARAMETER,
        CompletionItemKind::Struct => lsp_types::CompletionItemKind::STRUCT,
        CompletionItemKind::Enum => lsp_types::CompletionItemKind::ENUM,
        CompletionItemKind::Module => lsp_types::CompletionItemKind::MODULE,
        CompletionItemKind::Constant => lsp_types::CompletionItemKind::CONSTANT,
        CompletionItemKind::Operator => lsp_types::CompletionItemKind::OPERATOR,
        CompletionItemKind::Property => lsp_types::CompletionItemKind::PROPERTY,
        CompletionItemKind::Text => lsp_types::CompletionItemKind::TEXT,
    }
}

/// Convert a completion for a client that may or may not take snippets.
///
/// `snippets` is `ClientCapabilities::completion_snippet_support`, which had
/// no caller: a client that cannot expand `${1:x}` was sent one anyway and
/// inserted the placeholder text literally.
pub fn completion_item(
    item: &CompletionItem,
    snippets: bool,
    markdown: bool,
    line_index: Option<&LineIndex>,
) -> lsp_types::CompletionItem {
    let wants_snippet = item.kind == CompletionItemKind::Snippet
        || item.insert_text.as_ref().is_some_and(|t| t.contains("$"));
    let is_snippet = wants_snippet && snippets;
    let insert_text = if wants_snippet && !snippets {
        item.insert_text.as_deref().map(strip_snippet_placeholders)
    } else {
        item.insert_text.clone()
    };
    let insert_text_format = if is_snippet {
        Some(lsp_types::InsertTextFormat::SNIPPET)
    } else {
        Some(lsp_types::InsertTextFormat::PLAIN_TEXT)
    };
    lsp_types::CompletionItem {
        label: item.label.clone(),
        kind: Some(completion_item_kind(item.kind)),
        detail: item.detail.clone(),
        // The client answers this separately from the hover one, and does
        // answer it differently.
        documentation: item.documentation.as_ref().map(|d| {
            let (kind, value) = if markdown {
                (lsp_types::MarkupKind::Markdown, d.clone())
            } else {
                (lsp_types::MarkupKind::PlainText, ide::hover::doc_to_plain_text(d))
            };
            lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent { kind, value })
        }),
        filter_text: item.filter_text.clone(),
        insert_text,
        insert_text_format,
        sort_text: item.sort_text.clone(),
        additional_text_edits: line_index
            .map(|index| text_edits(index, &item.additional_text_edits))
            .filter(|edits| !edits.is_empty()),
        deprecated: if item.deprecated { Some(true) } else { None },
        data: Some(serde_json::json!({
            "source": "sail-lsp",
            "autoInclude": item.import.as_ref().map(import_request),
            "scopeFile": item.scope_file.map(|id| id.index()),
            "origin": item.origin.map(|origin| serde_json::json!({
                "file": origin.file.index(), "declaration": origin.declaration,
                "namespace": match origin.namespace {
                    hir::source_order::bindings::Namespace::Type => "type",
                    hir::source_order::bindings::Namespace::Value => "value",
                    hir::source_order::bindings::Namespace::Callable => "callable",
                    hir::source_order::bindings::Namespace::Outcome => "outcome",
                },
            })),
            "kind": format!("{:?}", item.kind),
            "detail": item.detail.as_deref().unwrap_or("symbol"),
        })),
        ..lsp_types::CompletionItem::default()
    }
}

pub fn import_request(
    request: &ide_db::imports::import_assets::ImportRequest,
) -> serde_json::Value {
    serde_json::json!({
        "source": request.source.index(), "context": request.context.index(), "offset": request.offset,
        "target": request.target.index(), "name": request.name,
        "sourceHash": format!("{:x}", request.source_hash), "targetHash": format!("{:x}", request.target_hash),
    })
}

/// `f(${1:x}, ${2:y})$0` as `f(x, y)`, for a client that would otherwise
/// insert those braces as text.
fn strip_snippet_placeholders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('$') {
        out.push_str(&rest[..at]);
        rest = &rest[at + 1..];
        if let Some(inner) = rest.strip_prefix('{') {
            let Some(close) = inner.find('}') else { break };
            // `${1:name}` keeps `name`; `${1}` keeps nothing.
            let body = &inner[..close];
            if let Some((_, name)) = body.split_once(':') {
                out.push_str(name);
            }
            rest = &inner[close + 1..];
        } else {
            // `$0`, `$1`: a tab stop with nothing to insert.
            rest = rest.trim_start_matches(|c: char| c.is_ascii_digit());
        }
    }
    out.push_str(rest);
    out
}

pub fn inlay_hint_kind(kind: InlayHintKind) -> Option<lsp_types::InlayHintKind> {
    match kind {
        InlayHintKind::Type => Some(lsp_types::InlayHintKind::TYPE),
        InlayHintKind::Parameter => Some(lsp_types::InlayHintKind::PARAMETER),
        InlayHintKind::Other => None,
    }
}

pub fn inlay_hint(line_index: &LineIndex, hint: &InlayHint) -> lsp_types::InlayHint {
    lsp_types::InlayHint {
        position: position_in(line_index, hint.offset),
        label: lsp_types::InlayHintLabel::String(hint.label.clone()),
        kind: inlay_hint_kind(hint.kind),
        text_edits: None,
        tooltip: hint.tooltip.as_ref().map(|t| lsp_types::InlayHintTooltip::String(t.clone())),
        padding_left: hint.padding_left,
        padding_right: hint.padding_right,
        data: hint.data.clone(),
    }
}

pub fn semantic_token(t: &HlRange) -> lsp_types::SemanticToken {
    lsp_types::SemanticToken {
        delta_line: t.delta_line,
        delta_start: t.delta_start,
        length: t.length,
        token_type: t.token_type,
        token_modifiers_bitset: t.token_modifiers_bitset,
    }
}

/// The tokens as the protocol wants them, with no result id.
///
/// A result id is what the *client* quotes back to ask for a delta, so it
/// has to be one this server issued and remembers — `SemanticTokensCache`
/// hands those out. `ide` used to mint one from the file's length and the
/// token count, which the cache then ignored.
pub fn semantic_tokens(t: &HlRanges) -> lsp_types::SemanticTokens {
    lsp_types::SemanticTokens { result_id: None, data: t.data.iter().map(semantic_token).collect() }
}

/// A fold, as the protocol wants it.
///
/// This used to take `ide_db::ide_types::FoldingRange` — a type with no
/// producer anywhere — while `handle_folding_range` built the LSP value
/// inline from the type the feature actually returns. `folding_ranges.rs`
/// even carried a comment saying the conversion happened here.
pub fn folding_range(fold: &ide::folding_ranges::Fold) -> lsp_types::FoldingRange {
    use ide::folding_ranges::FoldKind;
    lsp_types::FoldingRange {
        start_line: fold.start_line,
        start_character: None,
        end_line: fold.end_line,
        end_character: None,
        kind: Some(match fold.kind {
            FoldKind::Comment => lsp_types::FoldingRangeKind::Comment,
            FoldKind::Imports => lsp_types::FoldingRangeKind::Imports,
            _ => lsp_types::FoldingRangeKind::Region,
        }),
        collapsed_text: None,
    }
}

pub fn selection_range(
    line_index: &LineIndex,
    sr: &ide_db::ide_types::SelectionRange,
) -> lsp_types::SelectionRange {
    lsp_types::SelectionRange {
        range: range(line_index, sr.range),
        parent: sr.parent.as_ref().map(|p| Box::new(selection_range(line_index, p))),
    }
}

pub fn linked_editing_ranges(
    line_index: &LineIndex,
    lr: &ide_db::ide_types::LinkedEditingRanges,
) -> lsp_types::LinkedEditingRanges {
    lsp_types::LinkedEditingRanges {
        ranges: lr.ranges.iter().map(|r| range(line_index, *r)).collect(),
        word_pattern: lr.word_pattern.clone(),
    }
}

pub fn document_link(
    line_index: &LineIndex,
    dl: &ide_db::ide_types::DocumentLink,
) -> lsp_types::DocumentLink {
    lsp_types::DocumentLink {
        range: range(line_index, dl.range),
        target: dl.target.as_ref().and_then(|t| lsp_types::Url::parse(t).ok()),
        tooltip: dl.tooltip.clone(),
        data: None,
    }
}

pub fn call_hierarchy_item(
    line_index: &LineIndex,
    item: &CallItem,
) -> lsp_types::CallHierarchyItem {
    lsp_types::CallHierarchyItem {
        name: item.name.clone(),
        kind: symbol_kind(item.kind),
        tags: None,
        detail: item.detail.clone(),
        uri: item.url.clone(),
        range: range(line_index, item.range),
        selection_range: range(line_index, item.selection_range),
        data: item.data.clone(),
    }
}

pub fn type_hierarchy_item(
    line_index: &LineIndex,
    item: &TypeHierarchyItem,
) -> lsp_types::TypeHierarchyItem {
    lsp_types::TypeHierarchyItem {
        name: item.name.clone(),
        kind: symbol_kind(item.kind),
        tags: None,
        detail: item.detail.clone(),
        uri: item.url.clone(),
        range: range(line_index, item.range),
        selection_range: range(line_index, item.selection_range),
        data: item.data.clone(),
    }
}
