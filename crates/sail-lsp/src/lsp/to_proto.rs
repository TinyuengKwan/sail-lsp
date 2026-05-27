//! Internal types → LSP wire types.
//!
//! Single conversion boundary. All IDE crates use internal types;
//! this module translates them for the LSP wire protocol.

use ide_db::ide_types::*;
use ide_db::line_index::{LineCol, LineIndex, TextRange};
use ide_diagnostics::{Diagnostic, Severity};
#[allow(unused_imports)]
use lsp_types::{self, Position, Range, Url};

pub fn position(lc: LineCol) -> Position {
    Position::new(lc.line, lc.col)
}

pub fn range(line_index: &LineIndex, r: TextRange) -> Range {
    Range::new(
        position(line_index.line_col(base_db::range_start(r))),
        position(line_index.line_col(base_db::range_end(r))),
    )
}

pub fn text_edit(line_index: &LineIndex, edit: &IdeTextEdit) -> lsp_types::TextEdit {
    lsp_types::TextEdit { range: range(line_index, edit.range), new_text: edit.new_text.clone() }
}

#[allow(dead_code)]
pub fn text_edits(line_index: &LineIndex, edits: &[IdeTextEdit]) -> Vec<lsp_types::TextEdit> {
    edits.iter().map(|e| text_edit(line_index, e)).collect()
}

#[allow(dead_code)]
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
pub fn hover(line_index: &LineIndex, h: &HoverResult) -> lsp_types::Hover {
    lsp_types::Hover {
        contents: lsp_types::HoverContents::Markup(lsp_types::MarkupContent {
            kind: lsp_types::MarkupKind::Markdown,
            value: h.markup.clone(),
        }),
        range: Some(range(line_index, h.range)),
    }
}

/// Convert a `SourceChange` to an LSP `WorkspaceEdit`.
/// via the provided lookup closure, then converts each file's
/// TextEdits to LSP TextEdits using line indices.
/// handlers producing SourceChange can share this conversion.
pub fn workspace_edit(
    source_change: &ide_db::source_change::SourceChange,
    file_url: &dyn Fn(base_db::FileId) -> Option<Url>,
    file_line_index: &dyn Fn(base_db::FileId) -> Option<LineIndex>,
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
        let lsp_edits: Vec<lsp_types::TextEdit> = edits.iter().map(|e| text_edit(&li, e)).collect();
        changes.insert(url, lsp_edits);
    }

    lsp_types::WorkspaceEdit { changes: Some(changes), ..Default::default() }
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
pub fn diagnostic(line_index: &LineIndex, d: &Diagnostic) -> lsp_types::Diagnostic {
    let mut tags = Vec::new();
    if d.unused {
        tags.push(lsp_types::DiagnosticTag::UNNECESSARY);
    }
    lsp_types::Diagnostic {
        range: range(line_index, d.range.range),
        severity: Some(diagnostic_severity(d.severity)),
        code: Some(lsp_types::NumberOrString::String(d.code.as_str().to_string())),
        message: d.message.clone(),
        tags: if tags.is_empty() { None } else { Some(tags) },
        ..Default::default()
    }
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

pub fn completion_item(item: &CompletionItem) -> lsp_types::CompletionItem {
    let is_snippet = item.kind == CompletionItemKind::Snippet
        || item.insert_text.as_ref().is_some_and(|t| t.contains("$"));
    let insert_text_format = if is_snippet {
        Some(lsp_types::InsertTextFormat::SNIPPET)
    } else {
        Some(lsp_types::InsertTextFormat::PLAIN_TEXT)
    };
    lsp_types::CompletionItem {
        label: item.label.clone(),
        kind: Some(completion_item_kind(item.kind)),
        detail: item.detail.clone(),
        documentation: item.documentation.as_ref().map(|d| {
            lsp_types::Documentation::MarkupContent(lsp_types::MarkupContent {
                kind: lsp_types::MarkupKind::Markdown,
                value: d.clone(),
            })
        }),
        filter_text: item.filter_text.clone(),
        insert_text: item.insert_text.clone(),
        insert_text_format,
        sort_text: item.sort_text.clone(),
        deprecated: if item.deprecated { Some(true) } else { None },
        data: Some(serde_json::json!({
            "source": "sail-lsp",
            "kind": format!("{:?}", item.kind),
            "detail": item.detail.as_deref().unwrap_or("symbol"),
        })),
        ..lsp_types::CompletionItem::default()
    }
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
        position: position(line_index.line_col(hint.offset)),
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

pub fn semantic_tokens(t: &HlRanges) -> lsp_types::SemanticTokens {
    lsp_types::SemanticTokens {
        result_id: t.result_id.clone(),
        data: t.data.iter().map(semantic_token).collect(),
    }
}

#[allow(dead_code)]
pub fn semantic_tokens_edit(e: &HlRangesEdit) -> lsp_types::SemanticTokensEdit {
    lsp_types::SemanticTokensEdit {
        start: e.start,
        delete_count: e.delete_count,
        data: e.data.as_ref().map(|d| d.iter().map(semantic_token).collect()),
    }
}

#[allow(dead_code)]
pub fn semantic_tokens_delta(d: &HlRangesDelta) -> lsp_types::SemanticTokensDelta {
    lsp_types::SemanticTokensDelta {
        result_id: d.result_id.clone(),
        edits: d.edits.iter().map(semantic_tokens_edit).collect(),
    }
}

#[allow(dead_code)]
pub fn folding_range(line_index: &LineIndex, fr: &FoldingRange) -> lsp_types::FoldingRange {
    let start = line_index.line_col(base_db::range_start(fr.range));
    let end = line_index.line_col(base_db::range_end(fr.range));
    lsp_types::FoldingRange {
        start_line: start.line,
        start_character: None,
        end_line: end.line,
        end_character: None,
        kind: Some(match fr.kind {
            FoldingRangeKind::Region => lsp_types::FoldingRangeKind::Region,
            FoldingRangeKind::Comment => lsp_types::FoldingRangeKind::Comment,
            FoldingRangeKind::Import => lsp_types::FoldingRangeKind::Imports,
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
