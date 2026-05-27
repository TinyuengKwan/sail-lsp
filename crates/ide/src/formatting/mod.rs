pub(crate) mod comment;
pub(crate) mod expr;
pub(crate) mod items;
pub(crate) mod lists;
mod missed_spans;
pub(crate) mod report;
pub(crate) mod rewrite;
pub(crate) mod shape;
pub(crate) mod snippet;
pub(crate) mod vertical;
pub(crate) mod visitor;

use ide_db::ide_types::{
    DocumentLink, FormatOptions, IdeTextEdit, LinkedEditingRanges, SelectionRange,
};
use ide_db::line_index::TextRange;
use ide_db::{
    token_is_close_bracket, token_is_open_bracket, token_symbol_key, FileDb, TextDocument,
};
use std::path::Path;
use url::Url;

fn line_text_range(file: &dyn FileDb, line: u32) -> TextRange {
    let start = file.offset_at(&ide_db::LineCol { line, col: 0 });
    let end = file.offset_at(&ide_db::LineCol { line: line + 1, col: 0 });
    base_db::text_range(start, end)
}

fn full_text_range(file: &dyn FileDb) -> TextRange {
    base_db::text_range(0, file.text().len())
}

fn line_ending(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// CST-based indent level at a byte offset.
/// Count nesting ancestors (BLOCK_EXPR, CALLABLE_DEF, etc.)
/// to determine indentation depth. Falls back to text heuristic if CST
/// is not available.
pub fn indent_level_from_cst(text: &str, offset: usize) -> u32 {
    use parser::SyntaxKind as SK;
    let (root, _) = syntax::parse_text(text);

    // Find the token at offset
    let rowan_offset = rowan::TextSize::from(offset as u32);
    let token = match root.token_at_offset(rowan_offset) {
        rowan::TokenAtOffset::Single(t) => t,
        rowan::TokenAtOffset::Between(_, right) => right,
        rowan::TokenAtOffset::None => return 0,
    };

    // Walk ancestors, count nesting block-like nodes
    let mut level = 0u32;
    for ancestor in token.parent_ancestors() {
        match ancestor.kind() {
            SK::BLOCK_EXPR | SK::BODY => level += 1,
            SK::CALLABLE_DEF | SK::CALLABLE_SPEC => level += 1,
            SK::NAMED_DEF | SK::SCATTERED_DEF | SK::SCATTERED_CLAUSE_DEF => level += 1,
            SK::MATCH_EXPR
            | SK::IF_EXPR
            | SK::TRY_EXPR
            | SK::FOREACH_EXPR
            | SK::WHILE_EXPR
            | SK::REPEAT_EXPR => {
                // These contain their own blocks — don't double-count
                // if parent is already BODY or BLOCK_EXPR
            }
            SK::MATCH_ARM => level += 1,
            SK::SOURCE_FILE => {} // root — no indent
            _ => {}
        }
    }
    // Subtract 1 because CALLABLE_DEF contains BODY which both count
    level.saturating_sub(1)
}


/// CST-based document formatting.
///
/// Pipeline:
///   1. Parse source into CST
///   2. Walk CST with `FmtVisitor` (definition-level rewrites)
///   3. `format_lines` (line overflow, trailing whitespace checks)
///   4. Post-process (final newlines, trailing whitespace trim)
///
/// Returns source unchanged on parse error.
pub fn format_document_cst(text: &str, options: &FormatOptions) -> String {
    // Phase 1: Parse.
    let (root, errors) = syntax::parse_text(text);
    if !errors.is_empty() {
        return text.to_string();
    }

    // Phase 2: Format.
    let snip = snippet::SnippetProvider::new(text.to_string());
    let mut vis = visitor::FmtVisitor::new(options, &snip);
    vis.walk_source_file(&root);

    // Phase 3: format_lines.
    format_lines(&mut vis.buffer, options, &mut vis.report);

    // Phase 4: Post-process.
    let mut result = vis.buffer;
    post_process_text(&mut result, text, options);

    // Phase 5: Report.
    check_lost_comments(text, &result, &mut vis.report);

    // Consume the report (future: surface via LSP diagnostics).
    let _has_issues = vis.report.has_errors();
    for err in &vis.report.errors {
        match &err.kind {
            report::ErrorKind::LineOverflow(actual, max) => {
                let _ = (actual, max, err.range);
            }
            report::ErrorKind::TrailingWhitespace | report::ErrorKind::LostComment => {
                let _ = err.range;
            }
        }
    }

    result
}

/// Check each line for overflow and trailing whitespace.
///
/// Called after the visitor produces output, before post-processing.
fn format_lines(
    text: &mut String,
    options: &FormatOptions,
    report: &mut report::FormatReport,
) {
    let root_shape = shape::Shape::with_max_width(options);
    let max_width = root_shape.budget(0);
    let mut offset = 0usize;

    for line in text.split('\n') {
        let range = base_db::text_range(offset, offset + line.len());

        if line.len() > max_width {
            report.push(report::ErrorKind::LineOverflow(line.len(), max_width), range);
        }

        if line != line.trim_end() {
            report.push(report::ErrorKind::TrailingWhitespace, range);
        }

        offset += line.len() + 1;
    }
}

/// Check if a comment present in `original` was lost in `formatted`.
///
/// Scans for `//` and `/*` tokens in the original that don't appear
/// in the formatted output, and records `ErrorKind::LostComment`.
fn check_lost_comments(
    original: &str,
    formatted: &str,
    report: &mut report::FormatReport,
) {
    // Extract comment snippets from original: first 40 chars of each comment.
    for (i, line) in original.lines().enumerate() {
        let trimmed = line.trim();
        let comment_text = if trimmed.starts_with("//") {
            Some(trimmed)
        } else if trimmed.starts_with("/*") {
            Some(trimmed)
        } else {
            None
        };
        if let Some(snippet) = comment_text {
            // Use a short prefix to match (comments may be reflowed).
            let key = &snippet[..snippet.len().min(30)];
            if !formatted.contains(key) {
                // Approximate offset: line number × average line length.
                let approx_offset = original.lines().take(i).map(|l| l.len() + 1).sum::<usize>();
                report.push(
                    report::ErrorKind::LostComment,
                    base_db::text_range(approx_offset, approx_offset + snippet.len()),
                );
            }
        }
    }
}

/// CST-based formatting with line-range restriction.
///
/// Nodes outside `file_lines` are emitted verbatim by the visitor.
pub fn format_document_cst_range(
    text: &str,
    options: &FormatOptions,
    file_lines: visitor::FileLines,
) -> String {
    let (root, errors) = syntax::parse_text(text);
    if !errors.is_empty() {
        return text.to_string();
    }

    let snip = snippet::SnippetProvider::new(text.to_string());
    let mut vis = visitor::FmtVisitor::with_file_lines(options, &snip, file_lines);
    vis.walk_source_file(&root);

    format_lines(&mut vis.buffer, options, &mut vis.report);

    let mut result = vis.buffer;
    post_process_text(&mut result, text, options);
    check_lost_comments(text, &result, &mut vis.report);
    result
}

/// Post-process formatted output (trailing whitespace, final newlines).
fn post_process_text(result: &mut String, original: &str, options: &FormatOptions) {
    // Trim trailing whitespace per line
    if options.trim_trailing_whitespace.unwrap_or(true) {
        let lines: Vec<&str> = result.lines().collect();
        *result = lines.iter().map(|l| l.trim_end()).collect::<Vec<_>>().join("\n");
    }

    // Preserve original final newline behavior
    let has_final_newline = original.ends_with('\n');
    if has_final_newline && !result.ends_with('\n') {
        result.push('\n');
    }

    if options.insert_final_newline == Some(true) && !result.ends_with('\n') {
        result.push('\n');
    }

    if options.trim_final_newlines == Some(true) {
        while result.ends_with("\n\n") {
            result.pop();
        }
    }
}

/// Format document — returns internal IdeTextEdit (framework-independent).
pub fn format_document_edits(
    file: &dyn FileDb,
    options: &FormatOptions,
) -> Option<Vec<IdeTextEdit>> {
    let original = file.text();
    let formatted = format_document_cst(original, options);
    if formatted == original {
        return None;
    }
    Some(vec![IdeTextEdit { range: base_db::text_range(0, original.len()), new_text: formatted }])
}

/// Range-format — returns internal IdeTextEdit.
pub fn range_format_document_edits(
    file: &dyn FileDb,
    range: TextRange,
    options: &FormatOptions,
) -> Option<Vec<IdeTextEdit>> {
    let formatted_full = format_document_cst(file.text(), options);
    if formatted_full == file.text() {
        return None;
    }
    let start_lc = file.position_at(base_db::range_start(range));
    let end_lc = file.position_at(base_db::range_end(range));
    let start_line = start_lc.line;
    let end_line_exclusive = if end_lc.line > start_lc.line && end_lc.col == 0 {
        end_lc.line
    } else {
        end_lc.line.saturating_add(1)
    };
    let original_start = file.offset_at(&ide_db::LineCol { line: start_line, col: 0 });
    let original_end = file.offset_at(&ide_db::LineCol { line: end_line_exclusive, col: 0 });
    let formatted_doc = TextDocument::new(formatted_full.clone());
    let formatted_start = formatted_doc.offset_at(&ide_db::LineCol { line: start_line, col: 0 });
    let formatted_end =
        formatted_doc.offset_at(&ide_db::LineCol { line: end_line_exclusive, col: 0 });
    let original_slice = &file.text()[original_start..original_end];
    let formatted_slice = &formatted_full[formatted_start..formatted_end];
    if original_slice == formatted_slice {
        return None;
    }
    Some(vec![IdeTextEdit {
        range: base_db::text_range(original_start, original_end),
        new_text: formatted_slice.to_string(),
    }])
}

/// Linked editing ranges — returns internal type.
pub fn linked_editing_ranges_for_position(
    file: &dyn FileDb,
    position: ide_db::LineCol,
) -> Option<LinkedEditingRanges> {
    let Some((token, _)) = file.token_at(position) else {
        return None;
    };
    let Some(symbol_key) = token_symbol_key(token) else {
        return None;
    };
    let Some(tokens) = file.tokens() else {
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
        ranges.push(base_db::text_range(span.start, span.end));
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

/// Document links — returns internal type.
pub fn document_links_for_file(uri: &Url, file: &dyn FileDb) -> Vec<DocumentLink> {
    let mut links = Vec::new();

    if let Some(tokens) = file.tokens() {
        for (token, span) in tokens {
            let parser::Token::String(content) = token else {
                continue;
            };
            let Some(target) = path_like_link_target(uri, content) else {
                continue;
            };
            links.push(DocumentLink {
                range: base_db::text_range(span.start, span.end),
                target: Some(target.to_string()),
                tooltip: Some("Open link".to_string()),
            });
        }
    }

    for prefix in ["https://", "http://"] {
        for (start, _) in file.text().match_indices(prefix) {
            let mut end = start + prefix.len();
            let bytes = file.text().as_bytes();
            while end < bytes.len() {
                let ch = bytes[end] as char;
                if ch.is_ascii_whitespace() || matches!(ch, ')' | ']' | '}' | '"' | '\'') {
                    break;
                }
                end += 1;
            }
            let raw = &file.text()[start..end];
            let Some(target) = Url::parse(raw).ok() else {
                continue;
            };
            links.push(DocumentLink {
                range: base_db::text_range(start, end),
                target: Some(target.to_string()),
                tooltip: Some("Open URL".to_string()),
            });
        }
    }

    // Extract [name] references from doc comments as links.
    // When a doc comment contains [some_function], create a link to
    // that function's definition in the same file.
    if let Some(tree) = file.item_tree() {
        for &id in tree.top_level_items() {
            if let Some(doc) = id.doc(&tree) {
                // Search for [name] patterns in the doc text
                let doc_start = id.span(&tree).start; // approximate: doc is before the def
                let mut search_pos = 0;
                while let Some(bracket_start) = doc[search_pos..].find('[') {
                    let abs_start = search_pos + bracket_start;
                    if let Some(bracket_end) = doc[abs_start + 1..].find(']') {
                        let ref_name = &doc[abs_start + 1..abs_start + 1 + bracket_end];
                        // Check if referenced name exists in ItemTree
                        if !ref_name.is_empty()
                            && ref_name.chars().all(|c| c.is_alphanumeric() || c == '_')
                            && tree.find_by_name(ref_name).is_some()
                        {
                            // Create a link (approximate range in doc)
                            links.push(DocumentLink {
                                range: base_db::text_range(
                                    doc_start, // approximate
                                    doc_start + ref_name.len(),
                                ),
                                target: Some(format!("{}#symbol={}", uri, ref_name)),
                                tooltip: Some(format!("Go to {ref_name}")),
                            });
                        }
                        search_pos = abs_start + 1 + bracket_end + 1;
                    } else {
                        break;
                    }
                }
            }
        }
    }

    links
}

/// Selection range — returns internal type.
pub fn make_selection_range(file: &dyn FileDb, position: ide_db::LineCol) -> SelectionRange {
    let mut ranges = Vec::<TextRange>::new();
    let offset = file.offset_at(&position);

    if let Some((_, span)) = file.token_at(position) {
        let tr = base_db::text_range(span.start, span.end);
        if !tr.is_empty() {
            ranges.push(tr);
        }
    }

    if let Some(tokens) = file.tokens() {
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
                let tr = base_db::text_range(open_span.start, span.end);
                if !tr.is_empty() {
                    ranges.push(tr);
                }
            }
        }
    }

    // Definition-level selection ranges from ItemTree
    if let Some(item_tree) = file.item_tree() {
        for &id in item_tree.top_level_items() {
            let span = id.span(&item_tree);
            if span.start <= offset && offset <= span.end {
                let r = base_db::text_range(span.start, span.end);
                if !r.is_empty() {
                    ranges.push(r);
                }
            }
        }
    }

    let lr = line_text_range(file, position.line);
    if !lr.is_empty() {
        ranges.push(lr);
    }
    let fr = full_text_range(file);
    if !fr.is_empty() {
        ranges.push(fr);
    }

    ranges.sort_by_key(|r| (r.len(), r.start()));
    ranges.dedup_by(|a, b| a == b);

    let mut parent: Option<Box<SelectionRange>> = None;
    for r in ranges.into_iter().rev() {
        parent = Some(Box::new(SelectionRange { range: r, parent }));
    }
    parent
        .map(|node| *node)
        .unwrap_or(SelectionRange { range: line_text_range(file, position.line), parent: None })
}

/// On-enter edits: continue doc comments (`///`) and maintain indentation
/// after opening brackets.
/// Returns internal IdeTextEdit.
pub fn on_enter_edits(file: &dyn FileDb, position: ide_db::LineCol) -> Option<Vec<IdeTextEdit>> {
    if position.line == 0 {
        return None;
    }
    let line_start = file.offset_at(&ide_db::LineCol { line: position.line, col: 0 });
    let current_offset = file.offset_at(&position);
    let prev_line_idx = position.line - 1;
    let prev_start = file.offset_at(&ide_db::LineCol { line: prev_line_idx, col: 0 });
    let prev_end =
        file.offset_at(&ide_db::LineCol { line: prev_line_idx + 1, col: 0 }).min(file.text().len());
    let prev_line = &file.text()[prev_start..prev_end];
    let prev_trimmed = prev_line.trim_end_matches(|c| c == '\n' || c == '\r');
    let stripped = prev_trimmed.trim_start();

    let make_edit = |new_text: String| -> IdeTextEdit {
        IdeTextEdit { range: base_db::text_range(line_start, current_offset), new_text }
    };

    if stripped.starts_with("///") {
        let indent: String =
            prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        return Some(vec![make_edit(format!("{indent}/// "))]);
    }
    if stripped.starts_with("//") && !stripped.starts_with("///") {
        let comment_content = stripped.strip_prefix("//").unwrap_or("").trim_start();
        let next_start = file.offset_at(&ide_db::LineCol { line: position.line + 1, col: 0 });
        let next_end = file
            .offset_at(&ide_db::LineCol { line: position.line + 2, col: 0 })
            .min(file.text().len());
        let next_line_is_comment = if next_start < next_end {
            file.text()
                .get(next_start..next_end)
                .map_or(false, |l| l.trim_start().starts_with("//"))
        } else {
            false
        };
        if !comment_content.is_empty() || next_line_is_comment {
            let indent: String =
                prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
            return Some(vec![make_edit(format!("{indent}// "))]);
        }
    }
    if stripped.ends_with('{') {
        let cur_start = file.offset_at(&ide_db::LineCol { line: position.line, col: 0 });
        let cur_end = file
            .offset_at(&ide_db::LineCol { line: position.line + 1, col: 0 })
            .min(file.text().len());
        let cur_line = &file.text()[cur_start..cur_end];
        let cur_trimmed = cur_line.trim();
        if cur_trimmed == "}" || cur_trimmed.is_empty() {
            let indent: String =
                prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
            return Some(vec![make_edit(format!("{indent}  "))]);
        }
    }
    // Sail keyword-blocks: `then`, `else`, `do` without braces
    if stripped.ends_with("then") || stripped.ends_with("else") || stripped.ends_with("do") {
        let indent: String =
            prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        return Some(vec![make_edit(format!("{indent}  "))]);
    }

    // CST-based fallback — if no specific trigger matched,
    // use CST ancestor walk to determine correct indent level.
    let cst_level = indent_level_from_cst(file.text(), current_offset);
    if cst_level > 0 {
        let indent = "  ".repeat(cst_level as usize);
        return Some(vec![make_edit(indent)]);
    }
    None
}


/// Join lines — returns internal IdeTextEdit.
pub fn join_lines_edits(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
    let text = file.text();
    let start_lc = file.position_at(base_db::range_start(range));
    let end_lc = file.position_at(base_db::range_end(range));

    // If no selection, join current line with the next one
    let start_line = start_lc.line;
    let end_line = if base_db::range_start(range) == base_db::range_end(range) {
        start_line + 1
    } else {
        end_lc.line
    };

    if start_line >= end_line {
        return None;
    }

    let mut edits = Vec::new();

    for line in start_line..end_line {
        let line_end_offset = file.offset_at(&ide_db::LineCol { line: line + 1, col: 0 });
        let next_line_end =
            file.offset_at(&ide_db::LineCol { line: line + 2, col: 0 }).min(text.len());

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
        let next_content_start = line_end_offset + next_line.len() - next_line.trim_start().len();

        // Determine separator
        let current_line_text =
            &text[file.offset_at(&ide_db::LineCol { line, col: 0 })..current_end];
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

        edits.push(IdeTextEdit {
            range: base_db::text_range(actual_current_end, next_content_start),
            new_text: separator.to_string(),
        });
    }

    if edits.is_empty() {
        None
    } else {
        Some(edits)
    }
}


/// Matching brace — returns byte offset of the matching bracket.
pub fn matching_brace_offset(file: &dyn FileDb, position: ide_db::LineCol) -> Option<usize> {
    let tokens = file.tokens()?;
    let offset = file.offset_at(&position);

    // Find the token at the cursor
    let (idx, _) = tokens
        .iter()
        .enumerate()
        .find(|(_, (_, span))| span.start <= offset && offset < span.end)?;

    let (token, _span) = &tokens[idx];

    // Define bracket pairs
    let pairs: &[(parser::Token, parser::Token)] = &[
        (parser::Token::LeftBracket, parser::Token::RightBracket),
        (parser::Token::LeftSquareBracket, parser::Token::RightSquareBracket),
        (parser::Token::LeftCurlyBracket, parser::Token::RightCurlyBracket),
        (parser::Token::LeftCurlyBar, parser::Token::RightCurlyBar),
        (parser::Token::LeftSquareBar, parser::Token::RightSquareBar),
    ];

    for (open, close) in pairs {
        if token == open {
            let mut depth = 1i32;
            for (t, s) in &tokens[idx + 1..] {
                if t == open {
                    depth += 1;
                } else if t == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s.start);
                    }
                }
            }
            return None;
        }
        if token == close {
            let mut depth = 1i32;
            for (t, s) in tokens[..idx].iter().rev() {
                if t == close {
                    depth += 1;
                } else if t == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some(s.start);
                    }
                }
            }
            return None;
        }
    }

    None
}

// Move Item Up/Down — REMOVED.
// Legacy move_item_edits/MoveDirection removed in E-Plan.
// Use ide::move_item::{move_item, Direction} instead.

/// Align bitfield fields for sail-riscv style formatting.
///
/// Input:
/// ```sail
/// bitfield Minterrupts : xlenbits = {
///   LCOFI: 13, // comment
///   MEI: 11,
///   MTI: 7,
/// }
/// ```
///
/// Output (aligned):
/// ```sail
/// bitfield Minterrupts : xlenbits = {
///   LCOFI : 13,  // comment
///   MEI   : 11,
///   MTI   :  7,
/// }
/// ```
pub fn align_bitfield_fields(text: &str) -> String {
    let eol = line_ending(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut result = Vec::new();
    let mut in_bitfield = false;
    let mut field_lines: Vec<(usize, &str)> = Vec::new(); // (line_idx, line_text)

    for (idx, &line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("bitfield") && trimmed.contains('{') {
            in_bitfield = true;
            result.push(line.to_string());
            continue;
        }
        if in_bitfield {
            if trimmed == "}" || trimmed.starts_with('}') {
                // Flush collected fields with alignment
                if !field_lines.is_empty() {
                    let aligned = align_field_group(&field_lines, &lines);
                    result.extend(aligned);
                    field_lines.clear();
                }
                in_bitfield = false;
                result.push(line.to_string());
                continue;
            }
            if trimmed.contains(':') && !trimmed.starts_with("//") {
                field_lines.push((idx, line));
            } else {
                // Non-field line inside bitfield (comment-only line)
                field_lines.push((idx, line));
            }
        } else {
            result.push(line.to_string());
        }
    }
    // Flush if file ends inside bitfield (shouldn't happen but be safe)
    if !field_lines.is_empty() {
        let aligned = align_field_group(&field_lines, &lines);
        result.extend(aligned);
    }

    let mut out = result.join(eol);
    if text.ends_with('\n') || text.ends_with('\r') {
        out.push_str(eol);
    }
    out
}

/// Align a group of bitfield field lines.
/// Pads field names to the same width and aligns bit positions.
fn align_field_group(field_lines: &[(usize, &str)], _all_lines: &[&str]) -> Vec<String> {
    // Parse each line into (indent, name, bit_text, comment)
    struct FieldParts {
        indent: String,
        name: String,
        bits: String,
        comment: String,
    }

    let mut parts: Vec<Option<FieldParts>> = Vec::new();
    let mut max_name_len = 0usize;
    let mut max_bits_len = 0usize;

    for &(_idx, line) in field_lines {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || trimmed.is_empty() {
            parts.push(None);
            continue;
        }
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        // Split at first ':'
        if let Some(colon_pos) = trimmed.find(':') {
            let name = trimmed[..colon_pos].trim().to_string();
            let rest = trimmed[colon_pos + 1..].trim();
            // Split rest at '//' for comment
            let (bits, comment) = if let Some(comment_pos) = rest.find("//") {
                (
                    rest[..comment_pos].trim().trim_end_matches(',').to_string(),
                    rest[comment_pos..].to_string(),
                )
            } else {
                (rest.trim_end_matches(',').to_string(), String::new())
            };
            max_name_len = max_name_len.max(name.len());
            max_bits_len = max_bits_len.max(bits.len());
            parts.push(Some(FieldParts { indent, name, bits, comment }));
        } else {
            parts.push(None);
        }
    }

    let mut result = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        match part {
            Some(fp) => {
                let padded_name = format!("{:<width$}", fp.name, width = max_name_len);
                let padded_bits = format!("{:>width$}", fp.bits, width = max_bits_len);
                let mut line = format!("{}{} : {}", fp.indent, padded_name, padded_bits);
                // Add comma (except maybe last)
                line.push(',');
                if !fp.comment.is_empty() {
                    // Pad before comment
                    let pad = max_bits_len.saturating_sub(fp.bits.len());
                    line.push_str(&" ".repeat(pad.min(2) + 2));
                    line.push_str(&fp.comment);
                }
                result.push(line);
            }
            None => {
                // Preserve original line (comment or blank)
                result.push(field_lines[i].1.to_string());
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn align_bitfield_simple() {
        let input = "\
bitfield Flags : bits(8) = {
  carry: 0,
  zero: 1,
  negative: 7,
}
";
        let output = align_bitfield_fields(input);
        // All field names should be padded to same width
        assert!(output.contains("carry    :"), "carry should be padded: {output}");
        assert!(output.contains("zero     :"), "zero should be padded: {output}");
        assert!(output.contains("negative :"), "negative should be at natural width: {output}");
    }

    #[test]
    fn align_bitfield_with_comments() {
        let input = "\
bitfield CSR : bits(32) = {
  MEI: 11, // Machine external
  MTI: 7, // Machine timer
  MSI: 3, // Machine software
}
";
        let output = align_bitfield_fields(input);
        // Bit positions should be right-aligned
        assert!(output.contains("MEI : 11,"), "MEI should align: {output}");
        assert!(output.contains("MTI :  7,"), "MTI should right-align: {output}");
        assert!(output.contains("MSI :  3,"), "MSI should right-align: {output}");
    }

    #[test]
    fn align_bitfield_preserves_non_bitfield() {
        let input = "val foo : int -> int\nfunction foo(x) = x\n";
        let output = align_bitfield_fields(input);
        assert_eq!(output, input, "non-bitfield code should be unchanged");
    }

    #[test]
    fn align_bitfield_range_fields() {
        let input = "\
bitfield Inst : bits(32) = {
  opcode: 6 .. 0,
  funct3: 14 .. 12,
}
";
        let output = align_bitfield_fields(input);
        assert!(output.contains("opcode :"), "opcode field present: {output}");
        assert!(output.contains("funct3 :"), "funct3 field present: {output}");
    }

    #[test]
    fn format_wraps_long_top_level_line() {
        let input = "val very_long_function_name : (bits(32), bits(32), bits(32), bits(32), bits(32), bits(32)) -> bits(64)\n";
        let opts = FormatOptions {
            tab_size: 2,
            insert_spaces: true,
            max_line_width: Some(60),
            ..Default::default()
        };
        // Use CST pipeline (rewrite_val_spec handles this).
        let output = format_document_cst(input, &opts);
        // Should be wrapped into multiple lines by CST rewrite_val_spec.
        let line_count = output.lines().count();
        assert!(line_count > 1, "long val should be wrapped (got {line_count} lines):\n{output}");
    }

}

#[cfg(test)]
mod cst_tests {
    use super::*;


    #[test]
    fn cst_passthrough_well_formed() {
        let input = "val foo : int -> int\nfunction foo(x) = x + 1\n";
        let result = format_document_cst(input, &FormatOptions::default());
        // Must not panic, must not lose content
        assert!(result.contains("val foo"), "val preserved: {result}");
        assert!(result.contains("function foo"), "function preserved: {result}");
    }


    #[test]
    fn cst_struct_alignment() {
        let input = "struct Point = {\n  x : int,\n  y_offset : bits(32),\n}\n";
        let result = format_document_cst(input, &FormatOptions::default());
        assert!(result.contains("x        : int"), "got: {result}");
        assert!(result.contains("y_offset : bits(32)"), "got: {result}");
    }


    #[test]
    fn cst_bitfield_alignment() {
        let input = "bitfield Foo : bits(8) = {\n  L : 7,\n  A : 4 .. 3,\n  X : 2,\n}\n";
        let result = format_document_cst(input, &FormatOptions::default());
        // All colons should be vertically aligned
        let l_line = result.lines().find(|l| l.contains("L ")).expect("L line");
        let a_line = result.lines().find(|l| l.contains("A ")).expect("A line");
        let x_line = result.lines().find(|l| l.contains("X ")).expect("X line");
        let l_colon = l_line.find(" : ").expect("L colon");
        let a_colon = a_line.find(" : ").expect("A colon");
        let x_colon = x_line.find(" : ").expect("X colon");
        assert_eq!(l_colon, a_colon, "L and A colons should align:\n{l_line}\n{a_line}");
        assert_eq!(a_colon, x_colon, "A and X colons should align:\n{a_line}\n{x_line}");
    }


    #[test]
    fn cst_mapping_alignment() {
        let input = "mapping foo : A <-> string = {\n  AMOSWAP <-> \"amoswap\",\n  AMOAND <-> \"amoand\",\n  AMOMAXU <-> \"amomaxu\",\n}\n";
        let result = format_document_cst(input, &FormatOptions::default());
        let swap_line = result.lines().find(|l| l.contains("AMOSWAP")).unwrap();
        let maxu_line = result.lines().find(|l| l.contains("AMOMAXU")).unwrap();
        let swap_arrow = swap_line.find("<->").unwrap();
        let maxu_arrow = maxu_line.find("<->").unwrap();
        assert_eq!(swap_arrow, maxu_arrow, "mapping <-> should align:\n{swap_line}\n{maxu_line}");
    }


    #[test]
    fn cst_match_arm_passthrough() {
        let input = "\
function bar(x : int) -> int = {
  match x {
    Some(y) => y,
    None => 0,
  }
}
";
        let result = format_document_cst(input, &FormatOptions::default());
        // Match arms are preserved as-is by the CST visitor.
        assert!(result.contains("Some(y) => y"), "Some arm preserved: {result}");
        assert!(result.contains("None => 0"), "None arm preserved: {result}");
    }


    #[test]
    fn cst_register_passthrough() {
        let input = "register PC : xlenbits\nregister nextPC : xlenbits\n";
        let result = format_document_cst(input, &FormatOptions::default());
        // Register declarations are preserved as-is by the CST visitor.
        assert!(result.contains("register PC : xlenbits"), "PC preserved: {result}");
        assert!(result.contains("register nextPC : xlenbits"), "nextPC preserved: {result}");
    }


    #[test]
    fn cst_fallback_on_parse_error() {
        // Deliberately broken Sail: unclosed brace
        let input = "function foo() = {\n  let x = 1\n";
        let cst_result = format_document_cst(input, &FormatOptions::default());
        assert_eq!(
            cst_result, input,
            "CST path should return source unchanged on parse error"
        );
    }


    #[test]
    fn cst_idempotent_simple() {
        let input = "val foo : int -> int\nfunction foo(x) = x + 1\n";
        let opts = FormatOptions::default();
        let first = format_document_cst(input, &opts);
        let second = format_document_cst(&first, &opts);
        assert_eq!(first, second, "formatting is not idempotent");
    }


    #[test]
    fn cst_idempotent_on_sample() {
        let sample = "\
val foo : int -> int
function foo(x) = x + 1

val bar : bits(32) -> bool
function bar(b) = b == 0x00000000
";
        let opts = FormatOptions::default();
        let first = format_document_cst(sample, &opts);
        let second = format_document_cst(&first, &opts);
        assert_eq!(first, second, "formatting is not idempotent");
    }

    #[test]
    fn cst_idempotent_struct() {
        let input = "struct Point = {\n  x : int,\n  y_offset : bits(32),\n}\n";
        let opts = FormatOptions::default();
        let first = format_document_cst(input, &opts);
        let second = format_document_cst(&first, &opts);
        assert_eq!(first, second, "struct formatting is not idempotent");
    }

    #[test]
    fn cst_preserves_final_newline() {
        let with_nl = "val foo : int\n";
        let result = format_document_cst(with_nl, &FormatOptions::default());
        assert!(result.ends_with('\n'), "final newline should be preserved");

        let without_nl = "val foo : int";
        let result2 = format_document_cst(without_nl, &FormatOptions::default());
        assert!(!result2.ends_with('\n'), "no final newline should be added when absent");
    }

    #[test]
    fn cst_insert_final_newline_option() {
        let input = "val foo : int";
        let opts = FormatOptions { insert_final_newline: Some(true), ..FormatOptions::default() };
        let result = format_document_cst(input, &opts);
        assert!(result.ends_with('\n'), "insert_final_newline should add newline");
    }

    #[test]
    fn cst_trim_final_newlines_option() {
        let input = "val foo : int\n\n\n";
        let opts = FormatOptions { trim_final_newlines: Some(true), ..FormatOptions::default() };
        let result = format_document_cst(input, &opts);
        assert!(!result.ends_with("\n\n"), "excess trailing newlines should be trimmed");
    }
}
