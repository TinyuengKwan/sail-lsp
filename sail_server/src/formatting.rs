use crate::state::{File, TextDocument};
use crate::symbols::{token_is_close_bracket, token_is_open_bracket, token_symbol_key};
use std::path::Path;
use tower_lsp::lsp_types::{
    DocumentLink, FormattingOptions, LinkedEditingRanges, Range, SelectionRange, TextEdit, Url,
};

pub(crate) fn range_len(file: &File, range: &Range) -> usize {
    let start = file.source.offset_at(&range.start);
    let end = file.source.offset_at(&range.end);
    end.saturating_sub(start)
}

fn line_range(line: u32) -> Range {
    let start = tower_lsp::lsp_types::Position::new(line, 0);
    let end = tower_lsp::lsp_types::Position::new(line + 1, 0);
    Range::new(start, end)
}

fn full_range(file: &File) -> Range {
    let text = file.source.text();
    let end = file.source.position_at(text.len());
    Range::new(tower_lsp::lsp_types::Position::new(0, 0), end)
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn indentation_delta(line: &str, in_block_comment: &mut bool) -> i32 {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    let mut delta = 0i32;
    let mut in_double = false;
    let mut escape = false;

    while i < bytes.len() {
        let b = bytes[i];

        if *in_block_comment {
            if b == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                *in_block_comment = false;
                i += 2;
            } else {
                i += 1;
            }
            continue;
        }

        if in_double {
            if b == b'"' && !escape {
                in_double = false;
            }
            escape = b == b'\\' && !escape;
            i += 1;
            continue;
        }

        if b == b'/' && i + 1 < bytes.len() {
            if bytes[i + 1] == b'/' {
                break;
            }
            if bytes[i + 1] == b'*' {
                *in_block_comment = true;
                i += 2;
                continue;
            }
        }

        match b {
            b'"' => {
                in_double = true;
                escape = false;
            }
            b'{' | b'[' | b'(' => delta += 1,
            b'}' | b']' | b')' => delta -= 1,
            _ => {}
        }
        i += 1;
    }

    delta
}

pub(crate) fn format_document_text(text: &str, options: &FormattingOptions) -> String {
    let eol = line_ending(text);
    let has_final_newline = text.ends_with('\n') || text.ends_with('\r');
    let trim_trailing = options.trim_trailing_whitespace.unwrap_or(true);
    let indent_unit = if options.insert_spaces {
        " ".repeat(options.tab_size.max(1) as usize)
    } else {
        "\t".to_string()
    };

    let mut indent = 0i32;
    let mut in_block_comment = false;
    let mut formatted = Vec::<String>::new();

    for original in text.lines() {
        let without_trailing = if trim_trailing {
            original.trim_end_matches([' ', '\t'])
        } else {
            original
        };
        let content = without_trailing.trim_start_matches([' ', '\t']);
        let original_indent = &without_trailing[..without_trailing.len() - content.len()];

        if content.is_empty() {
            formatted.push(String::new());
            continue;
        }

        let starts_with_closer =
            content.starts_with('}') || content.starts_with(']') || content.starts_with(')');
        let current_indent = (indent - i32::from(starts_with_closer)).max(0) as usize;

        let computed_indent = indent_unit.repeat(current_indent);
        let mut line = if original_indent.is_empty() {
            computed_indent
        } else {
            original_indent.to_string()
        };
        line.push_str(content);
        formatted.push(line);

        indent += indentation_delta(content, &mut in_block_comment);
        indent = indent.max(0);
    }

    let mut out = formatted.join(eol);
    if has_final_newline {
        out.push_str(eol);
    }

    if options.insert_final_newline == Some(true) && !out.ends_with('\n') {
        out.push_str(eol);
    }

    if options.trim_final_newlines == Some(true) {
        let had_any_final_newline = out.ends_with('\n') || out.ends_with('\r');
        while out.ends_with('\n') || out.ends_with('\r') {
            out.pop();
            if out.ends_with('\r') {
                out.pop();
            }
        }
        if had_any_final_newline {
            out.push_str(eol);
        }
    }

    out
}

pub(crate) fn format_document_edits(
    file: &File,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    let original = file.source.text();
    let formatted = format_document_text(original, options);
    if formatted == original {
        return None;
    }
    Some(vec![TextEdit {
        range: full_range(file),
        new_text: formatted,
    }])
}

pub(crate) fn range_format_document_edits(
    file: &File,
    range: Range,
    options: &FormattingOptions,
) -> Option<Vec<TextEdit>> {
    let formatted_full = format_document_text(file.source.text(), options);
    if formatted_full == file.source.text() {
        return None;
    }

    let start_line = range.start.line;
    let end_line_exclusive = if range.end.line > range.start.line && range.end.character == 0 {
        range.end.line
    } else {
        range.end.line.saturating_add(1)
    };

    let original_start = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(start_line, 0));
    let original_end = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(end_line_exclusive, 0));

    let formatted_doc = TextDocument::new(formatted_full.clone());
    let formatted_start =
        formatted_doc.offset_at(&tower_lsp::lsp_types::Position::new(start_line, 0));
    let formatted_end =
        formatted_doc.offset_at(&tower_lsp::lsp_types::Position::new(end_line_exclusive, 0));

    let original_slice = &file.source.text()[original_start..original_end];
    let formatted_slice = &formatted_full[formatted_start..formatted_end];
    if original_slice == formatted_slice {
        return None;
    }

    Some(vec![TextEdit {
        range: Range::new(
            tower_lsp::lsp_types::Position::new(start_line, 0),
            tower_lsp::lsp_types::Position::new(end_line_exclusive, 0),
        ),
        new_text: formatted_slice.to_string(),
    }])
}

pub(crate) fn linked_editing_ranges_for_position(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<LinkedEditingRanges> {
    let Some((token, _)) = file.token_at(position) else {
        return None;
    };
    let Some(symbol_key) = token_symbol_key(token) else {
        return None;
    };
    let Some(tokens) = file.tokens.as_deref() else {
        return None;
    };

    let mut ranges = Vec::new();
    for (candidate, span) in tokens {
        let Some(candidate_key) = token_symbol_key(candidate) else {
            continue;
        };
        if candidate_key != symbol_key {
            continue;
        }
        ranges.push(Range::new(
            file.source.position_at(span.start),
            file.source.position_at(span.end),
        ));
    }

    if ranges.len() < 2 {
        return None;
    }

    Some(LinkedEditingRanges {
        ranges,
        word_pattern: Some("[_A-Za-z][_A-Za-z0-9'~?]*".to_string()),
    })
}

fn path_like_link_target(base_uri: &Url, text: &str) -> Option<Url> {
    let cleaned = text.trim().trim_matches('"').trim_matches('\'');
    if cleaned.contains("://") {
        return Url::parse(cleaned).ok();
    }
    if !cleaned.ends_with(".sail") {
        return None;
    }
    let Ok(base_path) = base_uri.to_file_path() else {
        return None;
    };
    let target_path = if Path::new(cleaned).is_absolute() {
        Path::new(cleaned).to_path_buf()
    } else {
        let parent = base_path.parent()?;
        parent.join(cleaned)
    };
    Url::from_file_path(target_path).ok()
}

pub(crate) fn document_links_for_file(uri: &Url, file: &File) -> Vec<DocumentLink> {
    let mut links = Vec::new();

    if let Some(tokens) = file.tokens.as_deref() {
        for (token, span) in tokens {
            let sail_parser::Token::String(content) = token else {
                continue;
            };
            let Some(target) = path_like_link_target(uri, content) else {
                continue;
            };
            links.push(DocumentLink {
                range: Range::new(
                    file.source.position_at(span.start),
                    file.source.position_at(span.end),
                ),
                target: Some(target.clone()),
                tooltip: Some("Open link".to_string()),
                data: Some(serde_json::json!({ "target": target.as_str() })),
            });
        }
    }

    for prefix in ["https://", "http://"] {
        for (start, _) in file.source.text().match_indices(prefix) {
            let mut end = start + prefix.len();
            let bytes = file.source.text().as_bytes();
            while end < bytes.len() {
                let ch = bytes[end] as char;
                if ch.is_ascii_whitespace() || matches!(ch, ')' | ']' | '}' | '"' | '\'') {
                    break;
                }
                end += 1;
            }
            let raw = &file.source.text()[start..end];
            let Some(target) = Url::parse(raw).ok() else {
                continue;
            };
            links.push(DocumentLink {
                range: Range::new(file.source.position_at(start), file.source.position_at(end)),
                target: Some(target.clone()),
                tooltip: Some("Open URL".to_string()),
                data: Some(serde_json::json!({ "target": target.as_str() })),
            });
        }
    }

    links
}

pub(crate) fn make_selection_range(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> SelectionRange {
    let mut ranges = Vec::<Range>::new();
    let offset = file.source.offset_at(&position);

    if let Some((_, span)) = file.token_at(position) {
        let token_range = Range::new(
            file.source.position_at(span.start),
            file.source.position_at(span.end),
        );
        if range_len(file, &token_range) > 0 {
            ranges.push(token_range);
        }
    }

    if let Some(tokens) = file.tokens.as_deref() {
        let mut stack: Vec<usize> = Vec::new();
        for (idx, (token, span)) in tokens.iter().enumerate() {
            if token_is_open_bracket(token) {
                stack.push(idx);
                continue;
            }
            if !token_is_close_bracket(token) {
                continue;
            }
            let Some(open_idx) = stack.pop() else {
                continue;
            };
            let open_span = &tokens[open_idx].1;
            if open_span.start <= offset && offset <= span.end {
                let r = Range::new(
                    file.source.position_at(open_span.start),
                    file.source.position_at(span.end),
                );
                if range_len(file, &r) > 0 {
                    ranges.push(r);
                }
            }
        }
    }

    let lr = line_range(position.line);
    if range_len(file, &lr) > 0 {
        ranges.push(lr);
    }
    let fr = full_range(file);
    if range_len(file, &fr) > 0 {
        ranges.push(fr);
    }

    ranges.sort_by_key(|r| (range_len(file, r), r.start.line, r.start.character));
    ranges.dedup_by(|a, b| a == b);

    let mut parent: Option<Box<SelectionRange>> = None;
    for r in ranges.into_iter().rev() {
        parent = Some(Box::new(SelectionRange { range: r, parent }));
    }
    parent.map(|node| *node).unwrap_or(SelectionRange {
        range: line_range(position.line),
        parent: None,
    })
}
