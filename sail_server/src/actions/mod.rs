use crate::state::File;
use std::collections::HashMap;
use tower_lsp::lsp_types::{
    CodeActionKind, Diagnostic, FormattingOptions, Position, Range, TextEdit, Url,
};

pub(crate) fn code_action_kind_allowed(
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

pub(crate) fn lazy_code_action_data(uri: &Url, edits: &[TextEdit]) -> serde_json::Value {
    serde_json::json!({
        "uri": uri.as_str(),
        "edits": edits,
    })
}

pub(crate) fn resolve_code_action_edit_from_data(
    data: &serde_json::Value,
) -> Option<(Url, Vec<TextEdit>)> {
    let uri = data.get("uri")?.as_str().and_then(|s| Url::parse(s).ok())?;
    let edits_value = data.get("edits")?.clone();
    let edits = serde_json::from_value::<Vec<TextEdit>>(edits_value).ok()?;
    Some((uri, edits))
}

pub(crate) fn sail_source_fix_all_kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.sail")
}

pub(crate) fn default_code_action_format_options() -> FormattingOptions {
    FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    }
}

fn expected_token_from_message(message: &str) -> Option<&'static str> {
    let message = message.to_ascii_lowercase();
    if message.contains("expected ';'") || message.contains("expected ;") {
        return Some(";");
    }
    if message.contains("expected ')'") || message.contains("expected )") {
        return Some(")");
    }
    if message.contains("expected ']'") || message.contains("expected ]") {
        return Some("]");
    }
    if message.contains("expected '}'") || message.contains("expected }") {
        return Some("}");
    }
    if message.contains("expected ','") || message.contains("expected ,") {
        return Some(",");
    }
    if message.contains("expected '='") || message.contains("expected =") {
        return Some("=");
    }
    None
}

fn line_text(file: &File, line: u32) -> Option<&str> {
    let start = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line, 0));
    if start > file.source.text().len() {
        return None;
    }
    let end = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line + 1, 0))
        .min(file.source.text().len());
    Some(&file.source.text()[start..end])
}

pub(crate) fn missing_semicolon_fix(file: &File, diagnostic: &Diagnostic) -> Option<TextEdit> {
    let message = diagnostic.message.to_ascii_lowercase();
    if !message.contains("expected") || !message.contains(';') {
        return None;
    }

    let line = diagnostic.range.start.line;
    let text = line_text(file, line)?;
    let mut logical = text.trim_end_matches(['\n', '\r']);
    if let Some((head, _)) = logical.split_once("//") {
        logical = head;
    }
    let logical = logical.trim_end();
    if logical.is_empty() || logical.ends_with(';') {
        return None;
    }
    if logical.ends_with('{') || logical.ends_with('}') {
        return None;
    }

    let base = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line, 0));
    let insert_offset = base + logical.len();
    let insert_pos = file.source.position_at(insert_offset);

    Some(TextEdit {
        range: Range::new(insert_pos, insert_pos),
        new_text: ";".to_string(),
    })
}

fn missing_token_fix(file: &File, diagnostic: &Diagnostic, token: &str) -> Option<TextEdit> {
    if !matches!(token, ")" | "]" | "}" | "," | "=") {
        return None;
    }
    let pos = diagnostic.range.end;
    let offset = file.source.offset_at(&pos);
    if file.source.text().get(offset..offset + token.len()) == Some(token) {
        return None;
    }
    Some(TextEdit {
        range: Range::new(pos, pos),
        new_text: token.to_string(),
    })
}

pub(crate) fn quick_fix_for_diagnostic(
    file: &File,
    diagnostic: &Diagnostic,
) -> Option<(String, TextEdit, bool)> {
    let token = expected_token_from_message(&diagnostic.message)?;
    if token == ";" {
        let edit = missing_semicolon_fix(file, diagnostic)?;
        return Some(("Insert missing `;`".to_string(), edit, true));
    }
    let edit = missing_token_fix(file, diagnostic, token)?;
    Some((format!("Insert missing `{token}`"), edit, false))
}

pub(crate) fn extract_local_let_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let start = file.source.offset_at(&range.start);
    let end = file.source.offset_at(&range.end);
    if start >= end {
        return None;
    }

    let selected = file.source.text().get(start..end)?.trim();
    if selected.is_empty() || selected.contains('\n') || selected.contains('\r') {
        return None;
    }

    let binding = "extracted_value";
    let line_start = Position::new(range.start.line, 0);
    let line_start_offset = file.source.offset_at(&line_start);
    let line_end_offset = file
        .source
        .offset_at(&Position::new(range.start.line + 1, 0))
        .min(file.source.text().len());
    let current_line = file.source.text().get(line_start_offset..line_end_offset)?;
    let indent = current_line
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect::<String>();

    let insert_text = format!("{indent}let {binding} = {selected};\n");
    Some(vec![
        TextEdit {
            range: Range::new(line_start, line_start),
            new_text: insert_text,
        },
        TextEdit {
            range,
            new_text: binding.to_string(),
        },
    ])
}

fn is_import_directive(line: &str) -> bool {
    line.starts_with("$include ") || line.starts_with("include ")
}

pub(crate) fn organize_imports_edits(file: &File) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    if text.is_empty() {
        return None;
    }

    let mut lines: Vec<(usize, usize, &str)> = Vec::new();
    let mut cursor = 0usize;
    for chunk in text.split_inclusive('\n') {
        let end = cursor + chunk.len();
        lines.push((cursor, end, chunk));
        cursor = end;
    }
    if cursor < text.len() {
        lines.push((cursor, text.len(), &text[cursor..]));
    }
    if lines.is_empty() {
        return None;
    }

    let mut first_code = 0usize;
    while first_code < lines.len() {
        let trimmed = lines[first_code].2.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            first_code += 1;
            continue;
        }
        break;
    }
    if first_code >= lines.len() {
        return None;
    }

    let first_trimmed = lines[first_code].2.trim();
    if !is_import_directive(first_trimmed) || first_trimmed.contains("//") {
        return None;
    }

    let mut block_end = first_code;
    let mut directives: Vec<String> = Vec::new();
    while block_end < lines.len() {
        let trimmed = lines[block_end].2.trim();
        if trimmed.is_empty() {
            block_end += 1;
            continue;
        }
        if trimmed.starts_with("//") {
            return None;
        }
        if !is_import_directive(trimmed) || trimmed.contains("//") {
            break;
        }
        directives.push(trimmed.to_string());
        block_end += 1;
    }

    if directives.len() < 2 {
        return None;
    }

    directives.sort_unstable();
    directives.dedup();

    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let old_range_start = lines[first_code].0;
    let old_range_end = lines[block_end - 1].1;
    let mut replacement = directives.join(eol);
    if text.get(old_range_end.saturating_sub(1)..old_range_end) == Some("\n") {
        replacement.push_str(eol);
    }

    let old_block = text.get(old_range_start..old_range_end)?;
    if old_block == replacement {
        return None;
    }

    let edit = TextEdit {
        range: Range::new(
            file.source.position_at(old_range_start),
            file.source.position_at(old_range_end),
        ),
        new_text: replacement,
    };
    Some(vec![edit])
}

#[cfg(test)]
mod tests {
    use super::{extract_local_let_edits, organize_imports_edits};
    use crate::state::File;
    use tower_lsp::lsp_types::{Position, Range};

    #[test]
    fn extract_local_let_builds_insert_and_replace_edits() {
        let src = "function f(x) = x + 1\n";
        let file = File::new(src.to_string());
        let start = src.find("x + 1").expect("selection start");
        let end = start + "x + 1".len();
        let edits = extract_local_let_edits(
            &file,
            Range::new(file.source.position_at(start), file.source.position_at(end)),
        )
        .expect("extract edits");
        assert_eq!(edits.len(), 2);
        assert!(edits[0].new_text.contains("let extracted_value = x + 1;"));
        assert_eq!(edits[1].new_text, "extracted_value");
    }

    #[test]
    fn extract_local_let_rejects_empty_or_multiline_selection() {
        let src = "let x = (\n  1 + 2\n)\n";
        let file = File::new(src.to_string());
        let empty = Range::new(Position::new(0, 0), Position::new(0, 0));
        assert!(extract_local_let_edits(&file, empty).is_none());

        let start = src.find("(\n").expect("multiline start");
        let end = src.find(")\n").expect("multiline end") + 1;
        let range = Range::new(file.source.position_at(start), file.source.position_at(end));
        assert!(extract_local_let_edits(&file, range).is_none());
    }

    #[test]
    fn organize_imports_sorts_and_deduplicates_top_block() {
        let src = "$include \"b.sail\"\n$include \"a.sail\"\n$include \"a.sail\"\nlet x = 1\n";
        let file = File::new(src.to_string());
        let edits = organize_imports_edits(&file).expect("organize edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "$include \"a.sail\"\n$include \"b.sail\"\n"
        );
    }

    #[test]
    fn organize_imports_skips_when_comment_in_block() {
        let src = "$include \"b.sail\"\n// keep note\n$include \"a.sail\"\n";
        let file = File::new(src.to_string());
        assert!(organize_imports_edits(&file).is_none());
    }
}
