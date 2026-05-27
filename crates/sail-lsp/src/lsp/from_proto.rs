//! LSP wire types → internal types.

use ide_db::ide_types::{CompletionItem, CompletionItemKind, InlayHint, InlayHintKind};
use ide_db::line_index::{LineCol, LineIndex, TextRange};
use ide_diagnostics::{Diagnostic, Severity};
use lsp_types::{Position, Range};

/// Convert an LSP Position to a byte offset.
pub fn offset(line_index: &LineIndex, pos: Position) -> usize {
    line_index.offset(LineCol { line: pos.line, col: pos.character })
}

/// Convert an LSP Range to an internal TextRange.
pub fn text_range(line_index: &LineIndex, range: Range) -> TextRange {
    base_db::text_range(offset(line_index, range.start), offset(line_index, range.end))
}

/// Convert LSP FormattingOptions to internal FormatOptions.
pub fn format_options(opts: &lsp_types::FormattingOptions) -> ide_db::ide_types::FormatOptions {
    ide_db::ide_types::FormatOptions {
        tab_size: opts.tab_size,
        insert_spaces: opts.insert_spaces,
        trim_trailing_whitespace: opts.trim_trailing_whitespace,
        insert_final_newline: opts.insert_final_newline,
        trim_final_newlines: opts.trim_final_newlines,
        max_line_width: Some(100),
    }
}

/// Convert an LSP CompletionItem to an internal CompletionItem.
pub fn completion_item(item: &lsp_types::CompletionItem) -> CompletionItem {
    let kind = match item.kind {
        Some(lsp_types::CompletionItemKind::FUNCTION) => CompletionItemKind::Function,
        Some(lsp_types::CompletionItemKind::VARIABLE) => CompletionItemKind::Variable,
        Some(lsp_types::CompletionItemKind::KEYWORD) => CompletionItemKind::Keyword,
        Some(lsp_types::CompletionItemKind::SNIPPET) => CompletionItemKind::Snippet,
        Some(lsp_types::CompletionItemKind::FIELD) => CompletionItemKind::Field,
        Some(lsp_types::CompletionItemKind::ENUM_MEMBER) => CompletionItemKind::EnumMember,
        Some(lsp_types::CompletionItemKind::TYPE_PARAMETER) => CompletionItemKind::TypeParameter,
        Some(lsp_types::CompletionItemKind::STRUCT) => CompletionItemKind::Struct,
        Some(lsp_types::CompletionItemKind::ENUM) => CompletionItemKind::Enum,
        Some(lsp_types::CompletionItemKind::MODULE) => CompletionItemKind::Module,
        Some(lsp_types::CompletionItemKind::CONSTANT) => CompletionItemKind::Constant,
        _ => CompletionItemKind::Text,
    };
    CompletionItem {
        label: item.label.clone(),
        kind,
        detail: item.detail.clone(),
        documentation: item.documentation.as_ref().map(|d| match d {
            lsp_types::Documentation::String(s) => s.clone(),
            lsp_types::Documentation::MarkupContent(m) => m.value.clone(),
        }),
        insert_text: item.insert_text.clone(),
        text_edit: None,
        sort_text: item.sort_text.clone(),
        filter_text: item.filter_text.clone(),
        deprecated: item.deprecated.unwrap_or(false),
        relevance: Default::default(),
    }
}

/// Convert an LSP InlayHint to an internal InlayHint.
pub fn inlay_hint(line_index: &LineIndex, hint: &lsp_types::InlayHint) -> InlayHint {
    let offset = crate::from_proto::offset(line_index, hint.position);
    let label = match &hint.label {
        lsp_types::InlayHintLabel::String(s) => s.clone(),
        lsp_types::InlayHintLabel::LabelParts(parts) => {
            parts.iter().map(|p| p.value.clone()).collect::<String>()
        }
    };
    let kind = match hint.kind {
        Some(lsp_types::InlayHintKind::TYPE) => InlayHintKind::Type,
        Some(lsp_types::InlayHintKind::PARAMETER) => InlayHintKind::Parameter,
        _ => InlayHintKind::Other,
    };
    let tooltip = hint.tooltip.as_ref().map(|t| match t {
        lsp_types::InlayHintTooltip::String(s) => s.clone(),
        lsp_types::InlayHintTooltip::MarkupContent(m) => m.value.clone(),
    });
    InlayHint {
        offset,
        label,
        kind,
        tooltip,
        padding_left: hint.padding_left,
        padding_right: hint.padding_right,
        data: hint.data.clone(),
    }
}

/// Convert an LSP Diagnostic to an internal Diagnostic.
pub fn diagnostic(line_index: &LineIndex, d: &lsp_types::Diagnostic) -> Diagnostic {
    let code_str = match &d.code {
        Some(lsp_types::NumberOrString::String(s)) => s.as_str(),
        Some(lsp_types::NumberOrString::Number(_n)) => {
            return Diagnostic::new(
                hir_def::diagnostics::DiagnosticCode::SyntaxError,
                d.message.clone(),
                text_range(line_index, d.range),
            )
        }
        None => "",
    };
    let code = hir_def::diagnostics::DiagnosticCode::from_str(code_str);
    let severity = match d.severity {
        Some(lsp_types::DiagnosticSeverity::ERROR) => Severity::Error,
        Some(lsp_types::DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(lsp_types::DiagnosticSeverity::INFORMATION) => Severity::Information,
        Some(lsp_types::DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    };
    Diagnostic::new(code, d.message.clone(), text_range(line_index, d.range))
        .with_severity(severity)
}
