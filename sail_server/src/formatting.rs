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

    // AST-aware selection ranges: expressions, definitions, function bodies
    if let Some(core_ast) = file.core_ast.as_deref() {
        collect_ast_ranges(core_ast, offset, file, &mut ranges);
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

/// On-enter edits: continue doc comments (`///`) and maintain indentation
/// after opening brackets.
pub(crate) fn on_enter_edits(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<Vec<TextEdit>> {
    if position.line == 0 {
        return None;
    }
    let prev_line_idx = position.line - 1;
    let prev_start = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(prev_line_idx, 0));
    let prev_end = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(prev_line_idx + 1, 0))
        .min(file.source.text().len());
    let prev_line = &file.source.text()[prev_start..prev_end];
    let prev_trimmed = prev_line.trim_end_matches(['\n', '\r']);

    // Continue `///` doc comments.
    let stripped = prev_trimmed.trim_start();
    if stripped.starts_with("///") {
        let indent: String = prev_trimmed
            .chars()
            .take_while(|ch| *ch == ' ' || *ch == '\t')
            .collect();
        let insert_pos = tower_lsp::lsp_types::Position::new(position.line, 0);
        return Some(vec![TextEdit {
            range: Range::new(insert_pos, position),
            new_text: format!("{indent}/// "),
        }]);
    }

    // Continue `//` line comments only if pressed Enter in the middle of the text
    // or if followed by another comment.
    if stripped.starts_with("//") && !stripped.starts_with("///") {
        let comment_content = stripped.strip_prefix("//").unwrap_or("").trim_start();
        // Continue if: (a) there's content after the //, or (b) next line is also a comment
        let next_line_is_comment = {
            let next_start = file
                .source
                .offset_at(&tower_lsp::lsp_types::Position::new(position.line + 1, 0));
            let next_end = file
                .source
                .offset_at(&tower_lsp::lsp_types::Position::new(position.line + 2, 0))
                .min(file.source.text().len());
            if next_start < file.source.text().len() {
                let next_line = &file.source.text()[next_start..next_end];
                next_line.trim_start().starts_with("//")
            } else {
                false
            }
        };
        if !comment_content.is_empty() || next_line_is_comment {
            let indent: String = prev_trimmed
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let insert_pos = tower_lsp::lsp_types::Position::new(position.line, 0);
            return Some(vec![TextEdit {
                range: Range::new(insert_pos, position),
                new_text: format!("{indent}// "),
            }]);
        }
    }

    // Smart brace indentation: if Enter pressed right after `{`, expand the block
    if stripped.ends_with('{') {
        let cur_start = file
            .source
            .offset_at(&tower_lsp::lsp_types::Position::new(position.line, 0));
        let cur_end = file
            .source
            .offset_at(&tower_lsp::lsp_types::Position::new(position.line + 1, 0))
            .min(file.source.text().len());
        let cur_line = &file.source.text()[cur_start..cur_end];
        let cur_trimmed = cur_line.trim();
        // If the current (just-inserted) line contains only a closing brace, expand
        if cur_trimmed == "}" || cur_trimmed.is_empty() {
            let indent: String = prev_trimmed
                .chars()
                .take_while(|ch| *ch == ' ' || *ch == '\t')
                .collect();
            let inner_indent = format!("{indent}  ");
            let insert_pos = tower_lsp::lsp_types::Position::new(position.line, 0);
            return Some(vec![TextEdit {
                range: Range::new(insert_pos, position),
                new_text: format!("{inner_indent}"),
            }]);
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Join Lines
// ---------------------------------------------------------------------------

/// Join lines in a selection range: merge consecutive lines intelligently.
pub(crate) fn join_lines_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let start_offset = file.source.offset_at(&range.start);
    let end_offset = file.source.offset_at(&range.end);

    // If no selection, join current line with the next one
    let start_line = range.start.line;
    let end_line = if start_offset == end_offset {
        start_line + 1
    } else {
        range.end.line
    };

    if start_line >= end_line {
        return None;
    }

    let mut edits = Vec::new();

    for line in start_line..end_line {
        let line_end_offset = file
            .source
            .offset_at(&tower_lsp::lsp_types::Position::new(line + 1, 0));
        let next_line_end = file
            .source
            .offset_at(&tower_lsp::lsp_types::Position::new(line + 2, 0))
            .min(text.len());

        if line_end_offset >= text.len() {
            break;
        }

        // Find the end of the current line (before newline)
        let mut current_end = line_end_offset;
        while current_end > 0 && matches!(text.as_bytes()[current_end - 1], b'\n' | b'\r') {
            current_end -= 1;
        }

        // Find the start of the next line's content (after leading whitespace)
        let next_line = &text[line_end_offset..next_line_end];
        let next_content_start =
            line_end_offset + next_line.len() - next_line.trim_start().len();

        // Determine separator
        let current_line_text = &text[file
            .source
            .offset_at(&tower_lsp::lsp_types::Position::new(line, 0))..current_end];
        let next_trimmed = next_line.trim();

        let separator = if current_line_text.trim_end().ends_with('{')
            || current_line_text.trim_end().ends_with('(')
            || current_line_text.trim_end().ends_with('[')
            || next_trimmed.starts_with('}')
            || next_trimmed.starts_with(')')
            || next_trimmed.starts_with(']')
            || next_trimmed.starts_with('.')
        {
            "" // No space before/after brackets or dot chains
        } else if current_line_text.trim_end().ends_with(',') {
            " " // Space after comma
        } else if next_trimmed.starts_with("//") {
            // Don't join comment lines
            continue;
        } else {
            " " // Default: single space
        };

        // Remove trailing comma if joining with closing bracket
        let mut actual_current_end = current_end;
        if (next_trimmed.starts_with('}')
            || next_trimmed.starts_with(')')
            || next_trimmed.starts_with(']'))
            && current_line_text.trim_end().ends_with(',')
        {
            actual_current_end -= 1;
        }

        edits.push(TextEdit {
            range: Range::new(
                file.source.position_at(actual_current_end),
                file.source.position_at(next_content_start),
            ),
            new_text: separator.to_string(),
        });
    }

    if edits.is_empty() {
        None
    } else {
        Some(edits)
    }
}

// ---------------------------------------------------------------------------
// Matching Brace
// ---------------------------------------------------------------------------

/// Find the position of the matching brace for the bracket at the given position.
pub(crate) fn matching_brace_position(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<tower_lsp::lsp_types::Position> {
    let tokens = file.tokens.as_deref()?;
    let offset = file.source.offset_at(&position);

    // Find the token at the cursor
    let (idx, _) = tokens
        .iter()
        .enumerate()
        .find(|(_, (_, span))| span.start <= offset && offset < span.end)?;

    let (token, _span) = &tokens[idx];

    // Define bracket pairs
    let pairs: &[(sail_parser::Token, sail_parser::Token)] = &[
        (
            sail_parser::Token::LeftBracket,
            sail_parser::Token::RightBracket,
        ),
        (
            sail_parser::Token::LeftSquareBracket,
            sail_parser::Token::RightSquareBracket,
        ),
        (
            sail_parser::Token::LeftCurlyBracket,
            sail_parser::Token::RightCurlyBracket,
        ),
        (
            sail_parser::Token::LeftCurlyBar,
            sail_parser::Token::RightCurlyBar,
        ),
        (
            sail_parser::Token::LeftSquareBar,
            sail_parser::Token::RightSquareBar,
        ),
    ];

    // Check if token is an opening bracket
    for (open, close) in pairs {
        if token == open {
            // Search forward for matching close
            let mut depth = 1i32;
            for (t, s) in &tokens[idx + 1..] {
                if t == open {
                    depth += 1;
                } else if t == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(file.source.position_at(s.start));
                    }
                }
            }
            return None;
        }
        if token == close {
            // Search backward for matching open
            let mut depth = 1i32;
            for (t, s) in tokens[..idx].iter().rev() {
                if t == close {
                    depth += 1;
                } else if t == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(file.source.position_at(s.start));
                    }
                }
            }
            return None;
        }
    }

    None
}

// ---------------------------------------------------------------------------
// Move Item Up/Down
// ---------------------------------------------------------------------------

/// Move the definition or item at the given position up or down.
/// Returns the text edits to apply, or None if the item can't be moved.
pub(crate) fn move_item_edits(
    file: &File,
    position: tower_lsp::lsp_types::Position,
    direction: MoveDirection,
) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let offset = file.source.offset_at(&position);
    let core_ast = file.core_ast.as_deref()?;

    // Find which top-level definition contains the cursor
    let mut current_idx = None;
    for (i, (_, def_span)) in core_ast.defs.iter().enumerate() {
        if def_span.start <= offset && offset <= def_span.end {
            current_idx = Some(i);
            break;
        }
    }
    let current_idx = current_idx?;

    let swap_idx = match direction {
        MoveDirection::Up => {
            if current_idx == 0 {
                return None;
            }
            current_idx - 1
        }
        MoveDirection::Down => {
            if current_idx + 1 >= core_ast.defs.len() {
                return None;
            }
            current_idx + 1
        }
    };

    let (_, current_span) = &core_ast.defs[current_idx];
    let (_, swap_span) = &core_ast.defs[swap_idx];

    let current_text = text.get(current_span.start..current_span.end)?;
    let swap_text = text.get(swap_span.start..swap_span.end)?;

    // Build edits: replace both spans. Apply in reverse order to maintain offsets.
    let (first_span, first_text, second_span, second_text) = if current_idx < swap_idx {
        (current_span, swap_text, swap_span, current_text)
    } else {
        (swap_span, current_text, current_span, swap_text)
    };

    Some(vec![
        TextEdit {
            range: Range::new(
                file.source.position_at(second_span.start),
                file.source.position_at(second_span.end),
            ),
            new_text: second_text.to_string(),
        },
        TextEdit {
            range: Range::new(
                file.source.position_at(first_span.start),
                file.source.position_at(first_span.end),
            ),
            new_text: first_text.to_string(),
        },
    ])
}

// ---------------------------------------------------------------------------
// AST-aware selection ranges
// ---------------------------------------------------------------------------

fn collect_ast_ranges(
    ast: &sail_parser::core_ast::SourceFile,
    offset: usize,
    file: &File,
    ranges: &mut Vec<Range>,
) {
    use sail_parser::core_ast::DefinitionKind;

    for (def, def_span) in &ast.defs {
        if def_span.start <= offset && offset <= def_span.end {
            // Definition range
            let r = Range::new(
                file.source.position_at(def_span.start),
                file.source.position_at(def_span.end),
            );
            if range_len(file, &r) > 0 {
                ranges.push(r);
            }

            // Recurse into callable body
            if let DefinitionKind::Callable(c) = &def.kind {
                if let Some(body) = &c.body {
                    collect_expr_ranges(body, offset, file, ranges);
                }
                for clause in &c.clauses {
                    if let Some(body) = &clause.0.body {
                        collect_expr_ranges(body, offset, file, ranges);
                    }
                    // Clause span
                    let cs = clause.1;
                    if cs.start <= offset && offset <= cs.end {
                        let cr = Range::new(
                            file.source.position_at(cs.start),
                            file.source.position_at(cs.end),
                        );
                        if range_len(file, &cr) > 0 {
                            ranges.push(cr);
                        }
                    }
                }
            }
        }
    }
}

fn collect_expr_ranges(
    expr: &(sail_parser::core_ast::Expr, sail_parser::Span),
    offset: usize,
    file: &File,
    ranges: &mut Vec<Range>,
) {
    use sail_parser::core_ast::{BlockItem, Expr};

    if offset < expr.1.start || offset > expr.1.end {
        return;
    }

    // Add this expression's range
    let r = Range::new(
        file.source.position_at(expr.1.start),
        file.source.position_at(expr.1.end),
    );
    if range_len(file, &r) > 0 {
        ranges.push(r);
    }

    // Recurse into sub-expressions
    match &expr.0 {
        Expr::If { cond, then_branch, else_branch, .. } => {
            collect_expr_ranges(cond, offset, file, ranges);
            collect_expr_ranges(then_branch, offset, file, ranges);
            if let Some(e) = else_branch {
                collect_expr_ranges(e, offset, file, ranges);
            }
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                        collect_expr_ranges(e, offset, file, ranges);
                    }
                    BlockItem::Let(lb) => collect_expr_ranges(&*lb.value, offset, file, ranges),
                }
            }
        }
        Expr::Match { scrutinee, cases, .. } | Expr::Try { scrutinee, cases, .. } => {
            collect_expr_ranges(scrutinee, offset, file, ranges);
            for (case, case_span) in cases {
                if case_span.start <= offset && offset <= case_span.end {
                    let cr = Range::new(
                        file.source.position_at(case_span.start),
                        file.source.position_at(case_span.end),
                    );
                    if range_len(file, &cr) > 0 {
                        ranges.push(cr);
                    }
                    collect_expr_ranges(&case.body, offset, file, ranges);
                }
            }
        }
        Expr::Let { body, .. } | Expr::Var { body, .. } => {
            collect_expr_ranges(body, offset, file, ranges);
        }
        Expr::Foreach(f) => collect_expr_ranges(&f.body, offset, file, ranges),
        Expr::While { cond, body, .. } => {
            collect_expr_ranges(cond, offset, file, ranges);
            collect_expr_ranges(body, offset, file, ranges);
        }
        Expr::Repeat { body, until, .. } => {
            collect_expr_ranges(body, offset, file, ranges);
            collect_expr_ranges(until, offset, file, ranges);
        }
        Expr::Infix { lhs, rhs, .. } => {
            collect_expr_ranges(lhs, offset, file, ranges);
            collect_expr_ranges(rhs, offset, file, ranges);
        }
        Expr::Call(call) => {
            collect_expr_ranges(&call.callee, offset, file, ranges);
            for arg in &call.args {
                collect_expr_ranges(arg, offset, file, ranges);
            }
        }
        Expr::Return(e) | Expr::Throw(e) | Expr::Prefix { expr: e, .. } | Expr::Cast { expr: e, .. } => {
            collect_expr_ranges(e, offset, file, ranges);
        }
        Expr::Assign { rhs, .. } => collect_expr_ranges(rhs, offset, file, ranges),
        Expr::Field { expr: e, .. } => collect_expr_ranges(e, offset, file, ranges),
        _ => {}
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MoveDirection {
    Up,
    Down,
}
