//! Insert `$include` directives.
//! Provides functions to insert a new `$include` directive into a file,
//! respecting existing includes and formatting conventions.

use crate::text_edit::TextEdit;

/// Configuration for `$include` insertion.
#[derive(Clone, Debug)]
pub struct InsertUseConfig {
    /// Whether to add a blank line after the last include.
    pub blank_line_after: bool,
    /// Whether to sort includes alphabetically.
    pub sort: bool,
}

impl Default for InsertUseConfig {
    fn default() -> Self {
        Self { blank_line_after: true, sort: true }
    }
}

/// Insert a `$include` directive for the given path.
///
/// Returns a `TextEdit` that adds `$include "path"` at the appropriate
/// location in the file (after existing includes, before definitions).
pub fn insert_include(text: &str, include_path: &str) -> TextEdit {
    // Find the last $include line
    let mut last_include_end = 0;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("$include") {
            // byte offset of end of this line
            let line_start: usize = text.lines().take(i).map(|l| l.len() + 1).sum();
            last_include_end = line_start + line.len() + 1; // +1 for \n
        }
    }

    let new_text = if last_include_end > 0 {
        format!("$include \"{include_path}\"\n")
    } else {
        // No existing includes — add at the top
        format!("$include \"{include_path}\"\n\n")
    };

    let offset = last_include_end.min(text.len());
    TextEdit { range: base_db::text_range(offset, offset), new_text }
}

/// Remove a `$include` directive for the given path.
///
/// Returns `Some(TextEdit)` if the include was found, `None` otherwise.
pub fn remove_include(text: &str, include_path: &str) -> Option<TextEdit> {
    let needle = format!("$include \"{include_path}\"");
    let mut offset = 0;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == needle || trimmed == format!("$include <{include_path}>") {
            let line_end = offset + line.len() + 1; // +1 for \n
            return Some(TextEdit {
                range: base_db::text_range(offset, line_end.min(text.len())),
                new_text: String::new(),
            });
        }
        offset += line.len() + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_first_include() {
        let text = "val x : int\n";
        let edit = insert_include(text, "prelude.sail");
        assert!(edit.new_text.contains("$include \"prelude.sail\""));
        assert_eq!(base_db::range_start(edit.range), 0);
    }

    #[test]
    fn insert_after_existing() {
        let text = "$include \"prelude.sail\"\nval x : int\n";
        let edit = insert_include(text, "helpers.sail");
        assert!(edit.new_text.contains("$include \"helpers.sail\""));
    }

    #[test]
    fn remove_existing() {
        let text = "$include \"prelude.sail\"\nval x : int\n";
        let edit = remove_include(text, "prelude.sail");
        assert!(edit.is_some());
        assert!(edit.unwrap().new_text.is_empty());
    }

    #[test]
    fn remove_nonexistent() {
        let text = "val x : int\n";
        assert!(remove_include(text, "nonexistent.sail").is_none());
    }
}
