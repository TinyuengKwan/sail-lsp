//! Typing — on-enter and on-char-typed actions.
//!
//! `on_char_typed(file, offset, char)` dispatches to per-character handlers
//! that produce local edits. `TRIGGER_CHARS` declares which characters
//! trigger the feature.
//!
//! Language server executes typing assists synchronously — they must
//! be fast.

// Re-export from formatting.rs (where the implementations currently live)
pub use crate::formatting::{indent_level_from_cst, on_enter_edits as on_enter};

/// Characters whose typing edits the text around them. Newline is
/// on-enter's (`on_enter`).
pub const TRIGGER_CHARS: &[char] = &['}', '>'];

/// The edits typing `char_typed` makes, ending at `offset`: local to the
/// line it was typed on, never a reformat of the document.
///
/// - `}` re-indents its line to the line of its `{`
/// - `>` completing `->` gets a space after it
///
/// There is no `=` handler. One inserted `>` after any `=` on a line
/// without `let`, `var`, `:` or `->`, which turned an assignment `x =`
/// into `x =>`.
pub fn on_char_typed(
    file: &dyn ide_db::FileDb,
    offset: usize,
    char_typed: char,
) -> Option<Vec<ide_db::text_edit::TextEdit>> {
    let text = file.text();
    if offset > text.len() {
        return None;
    }
    match char_typed {
        '>' => on_gt_typed(text, offset),
        '}' => on_closing_brace_typed(text, offset),
        _ => None,
    }
}

/// Handle `>` typed — format `->` with trailing space.
fn on_gt_typed(text: &str, offset: usize) -> Option<Vec<ide_db::text_edit::TextEdit>> {
    if offset < 2 {
        return None;
    }

    // Check if we just completed `->`
    let before = &text[..offset];
    if before.ends_with("->") {
        // If there's no space after `->`, add one
        if offset < text.len() {
            let next_char = text.as_bytes().get(offset)?;
            if *next_char != b' ' && *next_char != b'\n' && *next_char != b'\r' {
                return Some(vec![ide_db::text_edit::TextEdit {
                    range: base_db::text_range(offset, offset),
                    new_text: " ".to_string(),
                }]);
            }
        }
    }
    None
}

/// Handle `}` typed — auto-indent to matching `{` level.
fn on_closing_brace_typed(text: &str, offset: usize) -> Option<Vec<ide_db::text_edit::TextEdit>> {
    if offset < 1 {
        return None;
    }

    // Find the matching `{` and compute its indentation — counting from
    // before the `}` just typed, which is not one to match.
    let before = &text[..offset];
    let mut depth = 0i32;
    let mut match_pos = None;
    for (i, ch) in before[..offset - 1].char_indices().rev() {
        match ch {
            '}' => depth += 1,
            '{' => {
                if depth == 0 {
                    match_pos = Some(i);
                    break;
                }
                depth -= 1;
            }
            _ => {}
        }
    }

    let match_pos = match_pos?;

    // Get indentation of the line containing `{`
    let line_start = text[..match_pos].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let indent = &text[line_start..match_pos];
    let indent: String = indent.chars().take_while(|c| c.is_whitespace()).collect();

    // Get indentation of the current line (where `}` was typed)
    let brace_line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let current_indent = &text[brace_line_start..offset - 1]; // -1 for the `}` itself

    // If current indent differs from expected, fix it
    if current_indent != indent && current_indent.chars().all(|c| c.is_whitespace()) {
        let replace_range = base_db::text_range(brace_line_start, offset - 1);
        return Some(vec![ide_db::text_edit::TextEdit { range: replace_range, new_text: indent }]);
    }

    None
}
mod on_enter;

#[cfg(test)]
mod tests {
    /// The edits typing `ch` makes at `$0` (just after the typed character).
    fn typed(fixture: &str, ch: &str) -> Option<Vec<ide_db::text_edit::TextEdit>> {
        let (analysis, file, position) = crate::fixture::position(fixture);
        let url = analysis.url_for_file_id(file).unwrap().clone();
        analysis.on_type_formatting(&url, position, ch).unwrap()
    }

    /// A `}` re-indents its own line and nothing else — the badly
    /// formatted line above it is left as the user wrote it.
    #[test]
    fn a_closing_brace_reindents_only_its_line() {
        let source = "function f() -> int = {\n  let x:int=1;\n      }$0\n";
        let edits = typed(&format!("//- /main.sail\n{source}"), "}").expect("an edit");
        let clean = source.replace("$0", "");
        let line = clean.find("      }").unwrap();
        assert_eq!(edits.len(), 1, "{edits:?}");
        assert_eq!(base_db::range_start(edits[0].range), line);
        assert_eq!(base_db::range_end(edits[0].range), line + 6);
        assert_eq!(edits[0].new_text, "");
    }

    /// `;` reformatted the whole file; `=` in an assignment became `=>`.
    #[test]
    fn a_semicolon_or_an_assignment_changes_nothing() {
        let fixture = "//- /main.sail\nfunction f() -> int = {\n  var x : int = 1;\n  x =$0\n}\n";
        assert_eq!(typed(fixture, "="), None);
        let fixture = "//- /main.sail\nfunction f() -> int = {\n  let x:int=1;$0\n  x\n}\n";
        assert_eq!(typed(fixture, ";"), None);
    }
}
