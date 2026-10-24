//! LSP wire types → internal types.

use ide::inlay_hints::{InlayHint, InlayHintKind};
use ide::CompletionItem;
use ide_db::defs::CompletionItemKind;
use ide_db::line_index::{LineCol, LineIndex, TextRange};
use ide_diagnostics::{Diagnostic, Severity};
use lsp_types::{Position, Range};

/// Convert an LSP Position to a byte offset.
///
/// `Position::character` is a **UTF-16** column: that is LSP's default
/// `positionEncoding`, and this server negotiates nothing else. It used to
/// be passed straight through as a byte column, so on any line holding a
/// non-ASCII character every position from the client was wrong — the
/// bundled prelude's own licence header (`Union's Horizon 2020`) is such a
/// line — and slicing at one panicked.
///
/// rust-analyzer keeps the same conversion in its `LineIndex`
/// (`lib/line-index`, `to_utf8`/`to_wide` over a per-line wide-character
/// table).
pub fn offset(line_index: &LineIndex, pos: Position) -> usize {
    let col = line_index.utf16_col_to_byte(pos.line, pos.character);
    line_index.offset(LineCol { line: pos.line, col })
}

/// Convert an LSP Position to an internal `LineCol`, whose column is a
/// **byte** column.
///
/// The same conversion `offset` does, for the `Analysis` methods that take
/// a position rather than an offset. `lsp::ext` had a pair that did this
/// by assignment — `LineCol { line: pos.line, col: pos.character }` — and
/// twenty-seven handlers called it, so the UTF-16 fix that landed here
/// reached almost nothing: every hover, goto, rename and signature help
/// on a line with a non-ASCII character was still asking about the wrong
/// column. `protocol_check` reported zero over 1.2M ranges because it
/// exercises this module, and those handlers went round it.
pub fn line_col(line_index: &LineIndex, pos: Position) -> LineCol {
    LineCol { line: pos.line, col: line_index.utf16_col_to_byte(pos.line, pos.character) }
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
        additional_text_edits: Vec::new(), // Resolve recomputes edits from current inputs.
        import: item
            .data
            .as_ref()
            .and_then(|data| data.get("autoInclude"))
            .and_then(import_request),
        origin: item.data.as_ref().and_then(|data| {
            let origin = data.get("origin")?;
            let raw = u32::try_from(origin.get("file")?.as_u64()?).ok()?;
            Some(ide::CompletionOrigin {
                file: if raw & 0x8000_0000 != 0 {
                    base_db::FileId::for_instance(raw & 0x7fff_ffff)
                } else {
                    base_db::FileId::from_raw(raw)
                },
                declaration: usize::try_from(origin.get("declaration")?.as_u64()?).ok()?,
                namespace: match origin.get("namespace")?.as_str()? {
                    "type" => hir::source_order::bindings::Namespace::Type,
                    "value" => hir::source_order::bindings::Namespace::Value,
                    "callable" => hir::source_order::bindings::Namespace::Callable,
                    "outcome" => hir::source_order::bindings::Namespace::Outcome,
                    _ => return None,
                },
            })
        }),
        scope_file: item
            .data
            .as_ref()
            .and_then(|d| d.get("scopeFile"))
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok())
            .map(|raw| {
                if raw & 0x8000_0000 != 0 {
                    base_db::FileId::for_instance(raw & 0x7fff_ffff)
                } else {
                    base_db::FileId::from_raw(raw)
                }
            }),
        label: item.label.clone(),
        kind,
        detail: item.detail.clone(),
        documentation: item.documentation.as_ref().map(|d| match d {
            lsp_types::Documentation::String(s) => s.clone(),
            lsp_types::Documentation::MarkupContent(m) => m.value.clone(),
        }),
        insert_text: item.insert_text.clone(),
        sort_text: item.sort_text.clone(),
        filter_text: item.filter_text.clone(),
        deprecated: item.deprecated.unwrap_or(false),
        relevance: Default::default(),
    }
}

#[test]
fn completion_resolve_round_trip_keeps_request_scope() {
    use hir::source_order::bindings::Namespace;
    for (namespace, expected) in [
        ("type", Namespace::Type),
        ("value", Namespace::Value),
        ("callable", Namespace::Callable),
        ("outcome", Namespace::Outcome),
    ] {
        let wire = lsp_types::CompletionItem {
            label: "exported".into(),
            kind: Some(lsp_types::CompletionItemKind::FUNCTION),
            data: Some(
                serde_json::json!({"scopeFile": 42, "origin": {"file": 2147483649u32, "declaration": 7, "namespace": namespace}}),
            ),
            ..Default::default()
        };
        let item = completion_item(&wire);
        let wire = crate::to_proto::completion_item(&item, true, true, None);
        let item = completion_item(&wire);
        assert_eq!(item.scope_file, Some(base_db::FileId::from_raw(42)));
        assert_eq!(
            item.origin,
            Some(ide::CompletionOrigin {
                file: base_db::FileId::for_instance(1),
                declaration: 7,
                namespace: expected
            })
        );
        assert_eq!(item.label, "exported");
    }
}

pub fn import_request(
    data: &serde_json::Value,
) -> Option<ide_db::imports::import_assets::ImportRequest> {
    let physical = |field: &str| {
        let raw = u32::try_from(data.get(field)?.as_u64()?).ok()?;
        (raw < 0x8000_0000).then(|| base_db::FileId::from_raw(raw))
    };
    let context = u32::try_from(data.get("context")?.as_u64()?).ok()?;
    Some(ide_db::imports::import_assets::ImportRequest {
        source: physical("source")?,
        context: if context & 0x8000_0000 != 0 {
            base_db::FileId::for_instance(context & 0x7fff_ffff)
        } else {
            base_db::FileId::from_raw(context)
        },
        target: physical("target")?,
        offset: usize::try_from(data.get("offset")?.as_u64()?).ok()?,
        declaration: usize::try_from(data.get("declaration")?.as_u64()?).ok()?,
        name: data.get("name")?.as_str()?.to_owned(),
        in_type: data.get("inType")?.as_bool()?,
        source_hash: u64::from_str_radix(data.get("sourceHash")?.as_str()?, 16).ok()?,
        target_hash: u64::from_str_radix(data.get("targetHash")?.as_str()?, 16).ok()?,
    })
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
                hir::diagnostics::DiagnosticCode::SyntaxError,
                d.message.clone(),
                text_range(line_index, d.range),
            )
        }
        None => "",
    };
    let code = hir::diagnostics::DiagnosticCode::from_str(code_str);
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
