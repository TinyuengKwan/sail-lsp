#[cfg(test)]
mod corpus_tests;
#[cfg(test)]
mod experience_tests;
pub(crate) mod expr;
pub(crate) mod items;
mod layout;
pub(crate) mod lists;
mod missed_spans;
pub(crate) mod rewrite;
pub(crate) mod shape;
pub(crate) mod snippet;
pub(crate) mod vertical;
pub(crate) mod visitor;

use ide_db::ide_types::{DocumentLink, FormatOptions, IdeTextEdit, LinkedEditingRanges};
use ide_db::line_index::TextRange;
use ide_db::FileDb;
use std::path::Path;
use url::Url;

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
///   3. `formatting_details` (line overflow, trailing whitespace checks)
///   4. Post-process (final newlines, trailing whitespace trim)
///
/// Returns source unchanged on parse error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatStatus {
    Changed,
    AlreadyFormatted,
    ParseError,
    SafetyCheckFailed,
    RangeBoundary,
}

impl FormatStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::AlreadyFormatted => "already_formatted",
            Self::ParseError => "parse_error",
            Self::SafetyCheckFailed => "safety_check_failed",
            Self::RangeBoundary => "range_boundary",
        }
    }
}

#[derive(Debug)]
pub struct FormatResult {
    pub edits: Vec<IdeTextEdit>,
    pub text: String,
    pub status: FormatStatus,
    pub details: Vec<String>,
}

pub fn format_document_cst(text: &str, options: &FormatOptions) -> String {
    format_text(text, options).text
}

/// Syntax-only engine shared by LSP, explanation requests and the CLI.
pub fn format_text(text: &str, options: &FormatOptions) -> FormatResult {
    // Phase 1: Parse.
    let (root, errors) = syntax::parse_text(text);
    if !errors.is_empty() {
        return FormatResult {
            edits: Vec::new(),
            text: text.to_owned(),
            status: FormatStatus::ParseError,
            details: errors.iter().map(|e| format!("{} at byte {}", e.message, e.offset)).collect(),
        };
    }
    // Recovery can retain a partially typed declaration without emitting a
    // parser error. Unclosed delimiters still mean formatting is unavailable.
    let mut delimiters = Vec::new();
    use syntax::SyntaxKind as K;
    for token in root.descendants_with_tokens().filter_map(|e| e.into_token()) {
        match token.kind() {
            K::L_PAREN => delimiters.push(K::R_PAREN),
            K::L_CURLY => delimiters.push(K::R_CURLY),
            K::L_BRACK => delimiters.push(K::R_BRACK),
            K::L_CURLY_BAR => delimiters.push(K::R_CURLY_BAR),
            K::L_BRACKET_BAR => delimiters.push(K::R_BRACKET_BAR),
            K::R_PAREN | K::R_CURLY | K::R_BRACK | K::R_CURLY_BAR | K::R_BRACKET_BAR
                if delimiters.pop() != Some(token.kind()) =>
            {
                return FormatResult {
                    edits: Vec::new(),
                    text: text.into(),
                    status: FormatStatus::ParseError,
                    details: vec![format!(
                        "Unmatched delimiter at byte {}",
                        usize::from(token.text_range().start())
                    )],
                };
            }
            _ => {}
        }
    }
    if !delimiters.is_empty() {
        return FormatResult {
            edits: Vec::new(),
            text: text.into(),
            status: FormatStatus::ParseError,
            details: vec!["Source contains unclosed delimiters".into()],
        };
    }

    // Phase 2: Format.
    let snip = snippet::SnippetProvider::new(text.to_string());
    let mut vis = visitor::FmtVisitor::new(options, &snip);
    vis.walk_source_file(&root);

    // Collect explanation details without storing report state in the visitor.
    let details = formatting_details(&vis.buffer, options);

    // Phase 4: Normalize only whitespace tokens, never string/comment contents.
    let mut result = normalize_blank_lines(&vis.buffer);
    post_process_text(&mut result, text, options);

    // Phase 6: A formatter moves whitespace and nothing else. If any other
    // token — a comment included — differs, the source is returned as it
    // was. A lost comment used to be recorded in the report, which nothing
    // reads, and the text that had lost it was returned.
    let (formatted_root, formatted_errors) = syntax::parse_text(&result);
    if significant_tokens_in(&root) != significant_tokens_in(&formatted_root)
        || !formatted_errors.is_empty()
    {
        return FormatResult { edits: Vec::new(), text: text.to_owned(), status: FormatStatus::SafetyCheckFailed,
            details: vec!["Proposed formatting changed significant tokens or produced invalid syntax; no edits applied".into()] };
    }
    FormatResult {
        edits: diff_edits(text, &result),
        status: if result == text { FormatStatus::AlreadyFormatted } else { FormatStatus::Changed },
        text: result,
        details,
    }
}

/// What formatting must keep: every token but whitespace, each comment with
/// its lines' trailing whitespace trimmed (which formatting may remove).
///
/// Breaking a list across lines adds a trailing comma, and upstream accepts
/// one — `forall 'n 'm. (\n int('n),\n int('m),\n) -> bool` type-checks —
/// so a comma directly before a closing delimiter is dropped from both
/// sides. It is the one token the grammar lets a formatter add.
pub(crate) fn significant_tokens(text: &str) -> Vec<(syntax::SyntaxKind, String)> {
    let (root, _) = syntax::parse_text(text);
    significant_tokens_in(&root)
}

fn significant_tokens_in(root: &syntax::SyntaxNode) -> Vec<(syntax::SyntaxKind, String)> {
    use syntax::SyntaxKind as SK;
    let raw: Vec<(SK, String)> = root
        .descendants_with_tokens()
        .filter_map(|el| el.into_token())
        .filter(|t| t.kind() != SK::WHITESPACE)
        .map(|t| {
            let text = if t.kind().is_trivia() {
                t.text().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
            } else {
                t.text().to_string()
            };
            (t.kind(), text)
        })
        .collect();
    raw.iter()
        .enumerate()
        .filter(|(i, (kind, _))| {
            *kind != SK::COMMA
                || !matches!(
                    raw[i + 1..].iter().find(|(k, _)| !k.is_trivia()).map(|(k, _)| *k),
                    Some(SK::R_PAREN | SK::R_CURLY | SK::R_BRACK)
                )
        })
        .map(|(_, t)| t.clone())
        .collect()
}

/// Check each line for overflow and trailing whitespace.
///
/// Called after the visitor produces output, before post-processing.
fn formatting_details(text: &str, options: &FormatOptions) -> Vec<String> {
    let mut details = Vec::new();
    let root_shape = shape::Shape::with_max_width(options);
    let max_width = root_shape.budget(0);
    let mut offset = 0usize;

    for line in text.split('\n') {
        let columns = line.trim_end_matches('\r').chars().fold(0, |column, character| {
            if character == '\t' {
                column + options.tab_spaces() - column % options.tab_spaces()
            } else {
                column + 1
            }
        });
        if columns > max_width {
            details.push(format!(
                "Line width {columns} exceeds soft limit {max_width} at byte {offset}"
            ));
        }

        if line != line.trim_end() {
            details.push(format!("Trailing whitespace at byte {offset}"));
        }

        offset += line.len() + 1;
    }
    details
}

/// Whole-document blank-line policy. Range formatting deliberately leaves
/// whitespace outside its selected definitions untouched.
fn normalize_blank_lines(source: &str) -> String {
    let tokens = parser::LexedStr::new(source);
    let mut edits = Vec::new();
    let mut offset = 0;
    for i in 0..tokens.len() {
        let text = tokens.text(i);
        let start = offset;
        let end = start + text.len();
        offset = end;
        if tokens.kind(i) != syntax::SyntaxKind::WHITESPACE {
            continue;
        }
        let newlines = text.bytes().filter(|&b| b == b'\n').count();
        if newlines == 0 {
            continue;
        }
        let indentation = text.rsplit_once('\n').unwrap().1;
        let keep = if start == 0 { 0 } else { newlines.min(2) };
        if keep != newlines {
            edits.push((start, end, format!("{}{indentation}", "\n".repeat(keep))));
        }
    }
    let mut result = source.to_owned();
    for (start, end, text) in edits.into_iter().rev() {
        result.replace_range(start..end, &text);
    }
    result
}

/// Post-process formatted output (trailing whitespace, final newlines).
fn post_process_text(result: &mut String, original: &str, options: &FormatOptions) {
    let ending = line_ending(original);
    let tokens = parser::LexedStr::new(result);
    let mut processed = String::new();
    for i in 0..tokens.len() {
        let text = tokens.text(i);
        if tokens.kind(i) != syntax::SyntaxKind::WHITESPACE {
            processed.push_str(text);
            continue;
        }
        let normalized = text.replace("\r\n", "\n");
        for line in normalized.split_inclusive('\n') {
            if let Some(body) = line.strip_suffix('\n') {
                processed.push_str(if options.trim_trailing_whitespace.unwrap_or(true) {
                    body.trim_end_matches([' ', '\t'])
                } else {
                    body
                });
                // Line-comment tokens can own the CR while whitespace owns
                // the LF. Do not turn that boundary into CR-CR-LF.
                if ending == "\r\n" && processed.ends_with('\r') {
                    processed.push('\n');
                } else {
                    processed.push_str(ending);
                }
            } else {
                processed.push_str(
                    if i + 1 == tokens.len() && options.trim_trailing_whitespace.unwrap_or(true) {
                        line.trim_end_matches([' ', '\t'])
                    } else {
                        line
                    },
                );
            }
        }
    }
    *result = processed;
    if (original.ends_with('\n') || options.insert_final_newline.unwrap_or(true))
        && !result.ends_with('\n')
    {
        result.push_str(ending);
    }
    if options.trim_final_newlines.unwrap_or(true) {
        let double = ending.repeat(2);
        while result.ends_with(&double) {
            result.truncate(result.len() - ending.len());
        }
    }
}

/// Format document — returns internal IdeTextEdit (framework-independent).
pub fn format_document_edits(
    file: &dyn FileDb,
    options: &FormatOptions,
) -> Option<Vec<IdeTextEdit>> {
    let result = format_text(file.text(), options);
    (result.status == FormatStatus::Changed).then_some(result.edits)
}

/// Preserve unchanged source as anchors for cursors, selections and decorations.
/// Chunk lengths are UTF-8 bytes; the diff library only splits at character
/// boundaries. Adjacent insertions/deletions form one non-overlapping edit.
pub fn diff_edits(original: &str, formatted: &str) -> Vec<IdeTextEdit> {
    let mut edits = Vec::new();
    let mut offset = 0;
    let mut pending: Option<(usize, String)> = None;
    for chunk in dissimilar::diff(original, formatted) {
        match chunk {
            dissimilar::Chunk::Equal(text) => {
                if let Some((start, new_text)) = pending.take() {
                    edits.push(IdeTextEdit { range: base_db::text_range(start, offset), new_text });
                }
                offset += text.len();
            }
            dissimilar::Chunk::Delete(text) => {
                pending.get_or_insert_with(|| (offset, String::new()));
                offset += text.len();
            }
            dissimilar::Chunk::Insert(text) => {
                pending.get_or_insert_with(|| (offset, String::new())).1.push_str(text);
            }
        }
    }
    if let Some((start, new_text)) = pending {
        edits.push(IdeTextEdit { range: base_db::text_range(start, offset), new_text });
    }
    edits
}

/// Range-format — returns internal IdeTextEdit.
pub fn range_format_document_edits(
    file: &dyn FileDb,
    range: TextRange,
    options: &FormatOptions,
) -> Option<Vec<IdeTextEdit>> {
    let result = range_format_document_result(file, range, options);
    (result.status == FormatStatus::Changed).then_some(result.edits)
}

pub(crate) fn range_format_document_result(
    file: &dyn FileDb,
    range: TextRange,
    options: &FormatOptions,
) -> FormatResult {
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

    format_range(file.text(), options, base_db::text_range(original_start, original_end))
}

/// Keep complete edits inside the selected lines, using the enclosing syntax
/// for context. A fragment is never parsed as a separate compilation unit.
pub fn format_range(text: &str, options: &FormatOptions, range: TextRange) -> FormatResult {
    let result = format_text(text, options);
    if result.status != FormatStatus::Changed {
        return result;
    }
    let selected: Vec<_> = result
        .edits
        .into_iter()
        .filter(|edit| {
            range.contains_range(edit.range)
                && (edit.range.start() < range.end() || usize::from(range.end()) == text.len())
        })
        .collect();
    if selected.is_empty() {
        return FormatResult {
            edits: Vec::new(),
            text: text.into(),
            status: FormatStatus::RangeBoundary,
            details: vec!["No complete formatting edits fall inside the selected lines".into()],
        };
    }
    let mut formatted = text.to_owned();
    for edit in selected.iter().rev() {
        formatted.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.new_text,
        );
    }
    let (formatted_root, formatted_errors) = syntax::parse_text(&formatted);
    if significant_tokens(text) != significant_tokens_in(&formatted_root)
        || !formatted_errors.is_empty()
    {
        return FormatResult { edits: Vec::new(), text: text.into(), status: FormatStatus::RangeBoundary,
            details: vec!["Selected edits need neighbouring syntax; expand the selection to include the expression".into()] };
    }
    FormatResult {
        edits: selected,
        text: formatted,
        status: FormatStatus::Changed,
        details: result.details,
    }
}

/// Ranges to edit together as the name at `position` is typed: the binding
/// of a local and every mention of that binding.
///
/// This linked every token in the file that spelled the same name, so with
/// `editor.linkedEditing` on, typing in one function's `x` rewrote every
/// other `x` in the file. A top-level name is not offered at all: its other
/// mentions may be in other files, which linked editing cannot reach —
/// rename is the operation for that, as in rust-analyzer, which offers no
/// linked editing for Rust.
pub fn linked_editing_ranges_for_position(
    file: &dyn FileDb,
    position: ide_db::LineCol,
) -> Option<LinkedEditingRanges> {
    let spans = crate::references::local_spans_through_hir(file, position)?;
    if spans.len() < 2 {
        return None;
    }
    Some(LinkedEditingRanges {
        ranges: spans
            .into_iter()
            .map(|(span, _)| base_db::text_range(span.start, span.end))
            .collect(),
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
            if let Some(doc) = id.doc(tree) {
                // Search for [name] patterns in the doc text
                let doc_start = id.span(tree).start; // approximate: doc is before the def
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

/// On-enter edits: continue doc comments (`///`) and maintain indentation
/// after opening brackets.
/// Returns internal IdeTextEdit.
pub fn on_enter_edits(file: &dyn FileDb, position: ide_db::LineCol) -> Option<Vec<IdeTextEdit>> {
    on_enter_edits_with_options(
        file,
        position,
        &FormatOptions { tab_size: 2, ..Default::default() },
    )
}

pub fn on_enter_edits_with_options(
    file: &dyn FileDb,
    position: ide_db::LineCol,
    options: &FormatOptions,
) -> Option<Vec<IdeTextEdit>> {
    if position.line == 0 {
        return None;
    }
    let line_start = file.offset_at(&ide_db::LineCol { line: position.line, col: 0 });
    let current_offset = file.offset_at(&position);
    if !file.text().get(line_start..current_offset)?.chars().all(|c| matches!(c, ' ' | '\t')) {
        return None;
    }
    let prev_line_idx = position.line - 1;
    let prev_start = file.offset_at(&ide_db::LineCol { line: prev_line_idx, col: 0 });
    let prev_end =
        file.offset_at(&ide_db::LineCol { line: prev_line_idx + 1, col: 0 }).min(file.text().len());
    let prev_line = &file.text()[prev_start..prev_end];
    let prev_trimmed = prev_line.trim_end_matches(['\n', '\r']);
    let stripped = prev_trimmed.trim_start();
    let (syntax, _) = syntax::parse_text(file.text());
    let tokens: Vec<_> = syntax.descendants_with_tokens().filter_map(|e| e.into_token()).collect();
    // A newline inside a multiline literal/comment must not acquire code or
    // a comment prefix merely because its source line resembles Sail syntax.
    if tokens.iter().any(|t| {
        !t.kind().is_trivia()
            && t.kind() != syntax::SyntaxKind::WHITESPACE
            && usize::from(t.text_range().start()) < current_offset
            && usize::from(t.text_range().end()) > current_offset
            && matches!(
                t.kind(),
                syntax::SyntaxKind::STRING_LIT
                    | syntax::SyntaxKind::MULTILINE_STRING_LIT
                    | syntax::SyntaxKind::ERROR
            )
    }) {
        return None;
    }
    // A `///` comment is `DOC_COMMENT`, which the parser attaches to its
    // item and so is not trivia; a comment is a comment here regardless.
    let is_comment = |kind: syntax::SyntaxKind| {
        matches!(
            kind,
            syntax::SyntaxKind::LINE_COMMENT
                | syntax::SyntaxKind::BLOCK_COMMENT
                | syntax::SyntaxKind::DOC_COMMENT
        )
    };
    let comment = tokens.iter().any(|t| {
        is_comment(t.kind())
            && usize::from(t.text_range().start())
                == prev_start + prev_trimmed.len() - stripped.len()
            && t.text().starts_with("//")
    });
    if tokens.iter().any(|t| {
        is_comment(t.kind())
            && usize::from(t.text_range().start()) < current_offset
            && usize::from(t.text_range().end()) > current_offset
    }) {
        return None;
    }
    let last = tokens
        .iter()
        .rfind(|t| {
            !t.kind().is_trivia()
                && usize::from(t.text_range().end()) <= prev_end
                && usize::from(t.text_range().end()) > prev_start
        })
        .map(|t| t.kind());
    let unit = if options.hard_tabs() { "\t".to_owned() } else { " ".repeat(options.tab_spaces()) };

    let make_edit = |new_text: String| -> IdeTextEdit {
        IdeTextEdit { range: base_db::text_range(line_start, current_offset), new_text }
    };

    if comment && stripped.starts_with("///") {
        let indent: String =
            prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        return Some(vec![make_edit(format!("{indent}/// "))]);
    }
    if comment && stripped.starts_with("//") && !stripped.starts_with("///") {
        let comment_content = stripped.strip_prefix("//").unwrap_or("").trim_start();
        let next_start = file.offset_at(&ide_db::LineCol { line: position.line + 1, col: 0 });
        let next_end = file
            .offset_at(&ide_db::LineCol { line: position.line + 2, col: 0 })
            .min(file.text().len());
        let next_line_is_comment = if next_start < next_end {
            file.text().get(next_start..next_end).is_some_and(|l| l.trim_start().starts_with("//"))
        } else {
            false
        };
        if !comment_content.is_empty() || next_line_is_comment {
            let indent: String =
                prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
            return Some(vec![make_edit(format!("{indent}// "))]);
        }
    }
    if matches!(
        last,
        Some(
            syntax::SyntaxKind::L_CURLY | syntax::SyntaxKind::L_PAREN | syntax::SyntaxKind::L_BRACK
        )
    ) {
        let cur_start = file.offset_at(&ide_db::LineCol { line: position.line, col: 0 });
        let cur_end = file
            .offset_at(&ide_db::LineCol { line: position.line + 1, col: 0 })
            .min(file.text().len());
        let cur_line = &file.text()[cur_start..cur_end];
        let cur_trimmed = cur_line.trim();
        if cur_trimmed == "}" || cur_trimmed.is_empty() {
            let indent: String =
                prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
            return Some(vec![make_edit(format!("{indent}{unit}"))]);
        }
    }
    // Sail keyword-blocks: `then`, `else`, `do` without braces
    if matches!(
        last,
        Some(syntax::SyntaxKind::KW_THEN | syntax::SyntaxKind::KW_ELSE | syntax::SyntaxKind::KW_DO)
    ) {
        let indent: String =
            prev_trimmed.chars().take_while(|ch| *ch == ' ' || *ch == '\t').collect();
        return Some(vec![make_edit(format!("{indent}{unit}"))]);
    }

    // Continue the actual surrounding layout rather than guessing a level
    // from recovery nodes, whose ancestors can change while typing.
    let indent: String = prev_trimmed.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();
    if !indent.is_empty() {
        return Some(vec![make_edit(indent)]);
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

    /// Range formatting replaces only the lines asked for, and computes
    /// them by formatting only those lines.
    ///
    /// This used to format the whole document and cut the requested lines
    /// out of the result *by line number on both texts*. That holds only
    /// while formatting preserves the line count everywhere before the
    /// range — and a formatter that joins or splits a line is what a
    /// formatter is for.
    #[test]
    fn a_range_format_edits_only_the_range() {
        use ide_db::test_utils::TestFile;

        // Two definitions, the first written across two lines and the
        // second badly spaced. Ask about the second only.
        let source = "val f :
  int -> int
val g:int->int
";
        let file = TestFile::new(source);
        let opts = ide_db::ide_types::FormatOptions::default();
        let start = source.find("val g").expect("second definition");
        let range = base_db::text_range(start, source.len());
        let Some(edits) = super::range_format_document_edits(&file, range, &opts) else {
            return; // nothing to change is a legitimate answer
        };
        assert_eq!(edits.len(), 1);
        let edit = &edits[0];
        assert!(
            base_db::range_start(edit.range) >= start,
            "the edit starts inside the requested range, not before it"
        );
        assert!(
            !edit.new_text.contains("val f"),
            "the replacement does not carry the other definition: {:?}",
            edit.new_text
        );
    }

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
        assert_eq!(cst_result, input, "CST path should return source unchanged on parse error");
    }

    /// Formatting keeps every token, and formatting again changes nothing.
    ///
    /// `every_offered_assist_has_something_to_apply` re-parses what an assist
    /// produces; the formatter had no equivalent, and it is the feature that
    /// rewrites the user's whole file. Three idempotence tests on
    /// hand-written snippets is not the same check: a formatter that drops a
    /// token, or that reorders one, is silent corruption.
    ///
    /// The comparison is the non-trivia token stream, which is what "the same
    /// program" means for a formatter — whitespace is exactly what it is
    /// allowed to change.
    /// A formatter that would change more than whitespace returns the
    /// source as it was. This one joined a parameter list onto one line,
    /// and the comment between two parameters then commented out the rest
    /// of the header: `function f(a : int, // the flag b : bool) -> int = a`.
    #[test]
    fn a_format_that_would_change_a_token_changes_nothing() {
        let src = "function f(\n  a : int,\n  // the flag\n  b : bool\n) -> int = a\n";
        assert_eq!(format_document_cst(src, &FormatOptions::default()), src);
    }

    /// A `forwards`/`backwards` clause is part of the mapping. Aligning the
    /// `<->` arms rebuilt the body from those arms alone and dropped it.
    #[test]
    fn a_mapping_keeps_its_one_way_clauses() {
        let src = "mapping m : bits(2) <-> int = {\n  0b01 <-> 1,\n  backwards 0b00 => 0\n}\n\
                   struct S = {\n  a     : int,\n  bb : bool,\n}\n";
        let out = format_document_cst(src, &FormatOptions::default());
        assert!(out.contains("backwards 0b00 => 0"), "{out}");
        // And the rest of the file is still formatted: the guard did not
        // have to discard the whole result.
        assert!(out.contains("    a  : int,\n    bb : bool,"), "{out}");
    }

    #[test]
    fn formatting_keeps_every_token_and_settles() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../sail-lsp/data/sail-lib");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return; // the bundle moved; the other tests still run
        };
        let opts = FormatOptions::default();
        // Every token but whitespace, comments included: what the
        // formatter's own guard compares.
        let tokens = significant_tokens;

        let mut checked = 0usize;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "sail") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            // A file the parser already rejects is returned verbatim, so it
            // says nothing about the formatter.
            let (_, errors) = syntax::parse_text(&text);
            if !errors.is_empty() {
                continue;
            }
            checked += 1;
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();

            let once = format_document_cst(&text, &opts);
            let (_, errors) = syntax::parse_text(&once);
            assert!(errors.is_empty(), "formatting {name} produced text that does not parse");
            let (before, after) = (tokens(&text), tokens(&once));
            if before != after {
                let at = before.iter().zip(after.iter()).position(|(a, b)| a != b);
                let ctx = |v: &[(parser::SyntaxKind, String)], i: usize| {
                    v[i.saturating_sub(6)..(i + 3).min(v.len())]
                        .iter()
                        .map(|(_, t)| t.clone())
                        .collect::<Vec<_>>()
                        .join(" ")
                };
                match at {
                    Some(i) => panic!(
                        "formatting {name} changed its tokens at #{i}\n  before: {}\n   after: {}",
                        ctx(&before, i),
                        ctx(&after, i)
                    ),
                    None => panic!(
                        "formatting {name} changed its token count: {} -> {}",
                        before.len(),
                        after.len()
                    ),
                }
            }

            let twice = format_document_cst(&once, &opts);
            if once != twice {
                let at = once.lines().zip(twice.lines()).position(|(a, b)| a != b).unwrap_or(0);
                panic!(
                    "formatting {name} does not settle, first at line {}\n  once:  {:?}\n  twice: {:?}",
                    at + 1,
                    once.lines().nth(at).unwrap_or(""),
                    twice.lines().nth(at).unwrap_or("")
                );
            }
        }
        assert!(checked > 20, "expected the bundled library, found {checked} files");
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
        assert!(result2.ends_with('\n'), "Sail files end in a newline by default");
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
