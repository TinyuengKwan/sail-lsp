//! Typing — on-enter and on-char-typed actions.
//!
//! `on_char_typed(db, position, char)` dispatches to per-character handlers
//! that produce `SourceChange`. `TRIGGER_CHARS` declares which characters
//! trigger the feature.
//!
//! Language server executes typing assists synchronously — they must
//! be fast.

// Re-export from formatting.rs (where the implementations currently live)
pub use crate::formatting::{indent_level_from_cst, on_enter_edits as on_enter};

/// Trigger characters for on-type formatting.
/// Sail uses: `['}', ';', '\n', '=', '>']`.
pub const TRIGGER_CHARS: &[char] = &['}', ';', '\n', '=', '>'];

/// On-char-typed handler.
/// Checks that `char_typed` is a trigger character and dispatches
/// to the appropriate handler. Returns `None` if no edit is needed.
///
/// Currently delegates to existing formatting infrastructure.
/// Individual per-char handlers will be added incrementally:
/// - `}` → auto-indent to matching `{` level
/// - `=` → complete `=>` in match arms
/// - `>` → format `->` spacing
/// - `;` → auto-indent after let/var
/// - `\n` → continue line comments
pub fn on_char_typed(
    file: &dyn ide_db::FileDb,
    offset: usize,
    char_typed: char,
) -> Option<Vec<ide_db::text_edit::TextEdit>> {
    if !TRIGGER_CHARS.contains(&char_typed) {
        return None;
    }

    let text = file.text();
    if offset > text.len() {
        return None;
    }

    // Dispatch to per-character handlers
    match char_typed {
        '=' => on_eq_typed(text, offset),
        '>' => on_gt_typed(text, offset),
        '}' => on_closing_brace_typed(text, offset),
        _ => None,
    }
}

/// Handle `=` typed — complete `=>` in match context.
fn on_eq_typed(text: &str, offset: usize) -> Option<Vec<ide_db::text_edit::TextEdit>> {
    // If we just typed `=` and the previous context looks like a match arm
    // pattern, complete to `=>`
    if offset < 1 {
        return None;
    }

    // Check if this `=` is preceded by a pattern in match context
    let before = &text[..offset];
    let trimmed = before.trim_end();

    // Look for match arm context: line should not already have `=>`
    // and we should be inside a match block
    if trimmed.ends_with('=') && !trimmed.ends_with("==") && !trimmed.ends_with("!=") {
        // Check if previous non-whitespace suggests match arm
        let pre = trimmed[..trimmed.len() - 1].trim_end();
        // Simple heuristic: if the line doesn't contain `:` or `->`, might be match arm
        let last_line = pre.rsplit('\n').next().unwrap_or(pre);
        if !last_line.contains(':')
            && !last_line.contains("->")
            && !last_line.contains("let")
            && !last_line.contains("var")
        {
            // Insert `>` after `=` to make `=>`
            let insert_pos = offset;
            return Some(vec![ide_db::text_edit::TextEdit {
                range: base_db::text_range(insert_pos, insert_pos),
                new_text: ">".to_string(),
            }]);
        }
    }
    None
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

    // Find the matching `{` and compute its indentation
    let before = &text[..offset];
    let mut depth = 0i32;
    let mut match_pos = None;
    for (i, ch) in before.char_indices().rev() {
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
