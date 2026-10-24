//! Join lines — joins selected lines into one, fixing up whitespace.

use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::FileDb;

/// Join lines — returns internal IdeTextEdit.
pub(crate) fn join_lines(file: &dyn FileDb, range: TextRange) -> Option<Vec<IdeTextEdit>> {
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

#[cfg(test)]
mod tests {
    use crate::fixture;

    /// Through `Analysis`. This had no caller at all: the module was
    /// re-exported from `ide` and the server never named it.
    #[test]
    fn joining_two_lines_removes_the_break() {
        let source = "function f() = {\n  let x = 1;\n  x\n}\n";
        let (analysis, files) = fixture::analysis(source);
        let url = analysis.url_for_file_id(files[0].0).expect("fixture file has a url");

        // A cursor on the `let` line joins it with the next.
        let offset = source.find("let").expect("fixture");
        let edits = analysis.join_lines(url, base_db::text_range(offset, offset)).unwrap();
        assert!(!edits.is_empty(), "no edit for a joinable pair");

        let mut joined = source.to_owned();
        for edit in edits.iter().rev() {
            joined.replace_range(
                base_db::range_start(edit.range)..base_db::range_end(edit.range),
                &edit.new_text,
            );
        }
        assert!(joined.contains("let x = 1; x"), "lines were not joined: {joined:?}");
    }
}
