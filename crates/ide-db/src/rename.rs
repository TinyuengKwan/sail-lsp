//! Rename infrastructure for definitions.
//! This module provides the backend for semantic rename: given a
//! `Definition` and a new name, compute the multi-file `SourceChange`.
//! The IDE entry point (`ide/src/rename.rs`) delegates to these functions.

use base_db::FileId;

use crate::defs::Definition;
use crate::line_index::TextRange;
use crate::search::FileReference;
use crate::source_change::SourceChange;
use crate::text_edit::TextEdit;

/// Rename error — wraps a human-readable message.
///
/// `pub struct RenameError(pub String);`
#[derive(Debug)]
pub struct RenameError(pub String);

impl std::fmt::Display for RenameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RenameError {}

/// Convenience Result alias.
pub type Result<T, E = RenameError> = std::result::Result<T, E>;

/// Classification of an identifier for rename validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierKind {
    /// A normal identifier (function, type, variable name).
    Ident,
    /// An underscore `_` (wildcard).
    Underscore,
}

impl IdentifierKind {
    /// Validate and classify a proposed new name.
    ///
    /// Returns the validated name + its kind, or an error if invalid.
    pub fn classify(new_name: &str) -> Result<(String, IdentifierKind)> {
        if new_name.is_empty() {
            return Err(RenameError("New name cannot be empty".into()));
        }
        if new_name == "_" {
            return Ok((new_name.to_string(), IdentifierKind::Underscore));
        }
        let first = new_name.chars().next().unwrap();
        if !first.is_alphabetic() && first != '_' && first != '\'' {
            return Err(RenameError(format!(
                "Invalid identifier: must start with a letter, '_', or '\\'': `{new_name}`"
            )));
        }
        if new_name.chars().any(|c| c.is_whitespace()) {
            return Err(RenameError(format!(
                "Invalid identifier: contains whitespace: `{new_name}`"
            )));
        }
        if crate::keywords::SAIL_KEYWORDS.contains(&new_name) {
            return Err(RenameError(format!("Invalid name: `{new_name}` is a keyword")));
        }
        Ok((new_name.to_string(), IdentifierKind::Ident))
    }
}

/// Build a TextEdit list from a set of FileReferences.
///
/// For each reference, replaces its range with `new_name`.
pub fn source_edit_from_references(references: &[FileReference], new_name: &str) -> Vec<TextEdit> {
    references
        .iter()
        .map(|reference| TextEdit { range: reference.range, new_text: new_name.to_string() })
        .collect()
}

/// Find all whole-word occurrences of `name` in `text`, returning
/// FileReferences with byte-offset TextRanges.
///
/// This is the backend for `Definition::rename` when operating
/// on raw text without a full FileDb.
pub fn find_name_in_text(name: &str, text: &str) -> Vec<FileReference> {
    use crate::search::ReferenceCategory;

    let name_len = name.len();
    let bytes = text.as_bytes();
    let mut refs = Vec::new();
    let mut pos = 0;
    while pos + name_len <= text.len() {
        if let Some(idx) = text[pos..].find(name) {
            let start = pos + idx;
            let end = start + name_len;

            let before_ok = start == 0 || !is_ident_char(bytes[start - 1]);
            let after_ok = end >= text.len() || !is_ident_char(bytes[end]);

            if before_ok && after_ok && !is_in_comment_or_string(text, start) {
                let range = base_db::text_range(start, end);
                refs.push(FileReference { range, name: None, category: ReferenceCategory::READ });
            }
            pos = start + 1;
        } else {
            break;
        }
    }
    refs
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'\''
}

/// Simple heuristic: check if a byte offset is inside a line comment or string.
fn is_in_comment_or_string(text: &str, offset: usize) -> bool {
    let prefix = &text[..offset];
    // Line comment check
    let line_start = prefix.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line = &prefix[line_start..];
    if let Some(comment_pos) = line.find("//") {
        if line_start + comment_pos < offset {
            return true;
        }
    }
    // Block comment check (/* ... */)
    if let Some(block_start) = prefix.rfind("/*") {
        if prefix[block_start..].find("*/").is_none() {
            return true;
        }
    }
    // String literal check (count unescaped quotes)
    let quote_count = prefix.chars().filter(|&c| c == '"').count();
    quote_count % 2 != 0
}

impl Definition {
    /// Rename this definition and all its references across files.
    /// Steps:
    /// 1. Validate new_name via IdentifierKind::classify
    /// 2. Find all occurrences of old_name in provided files
    /// 3. Build SourceChange with per-file edits
    pub fn rename(
        &self,
        db: &dyn hir_def::db::DefDatabase,
        files: &[(FileId, &str)],
        new_name: &str,
    ) -> Result<SourceChange> {
        let (_validated, kind) = IdentifierKind::classify(new_name)?;

        if kind == IdentifierKind::Underscore {
            match self {
                Definition::Local(_) => {}
                _ => {
                    return Err(RenameError(
                        "Cannot rename to `_`: only local bindings can be discarded".into(),
                    ))
                }
            }
        }

        let old_name = self
            .name(db)
            .ok_or_else(|| RenameError("Cannot rename: definition has no name".into()))?;

        let mut source_change = SourceChange::new();

        for &(file_id, text) in files {
            let refs = find_name_in_text(old_name.as_str(), text);
            if !refs.is_empty() {
                let edits = source_edit_from_references(&refs, new_name);
                source_change.insert_source_edit(file_id, edits);
            }
        }

        Ok(source_change)
    }

    /// Get the range of the identifier to rename.
    ///
    /// Returns the file + text range of the definition's name token.
    pub fn range_for_rename(
        &self,
        db: &dyn hir_def::db::DefDatabase,
    ) -> Option<(FileId, TextRange)> {
        let file_id = self.file_id(db)?;
        let loc = self.def_location()?;
        let item_tree = hir_def::def_query::file_item_tree(db, loc.file_text).as_ref()?;
        let def_map = hir_def::def_query::crate_def_map(db, loc.file_text).as_ref()?;
        let raw_id = loc.raw_def_id();
        let def_data = def_map.0.get(raw_id)?;
        let mod_item = item_tree.top_level_items().get(def_data.item_tree_index)?;
        let span = mod_item.span(item_tree);

        let range = base_db::text_range(span.start, span.end);
        Some((file_id, range))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_valid_ident() {
        let (name, kind) = IdentifierKind::classify("foo").unwrap();
        assert_eq!(name, "foo");
        assert_eq!(kind, IdentifierKind::Ident);
    }

    #[test]
    fn classify_underscore() {
        let (_, kind) = IdentifierKind::classify("_").unwrap();
        assert_eq!(kind, IdentifierKind::Underscore);
    }

    #[test]
    fn classify_empty_fails() {
        assert!(IdentifierKind::classify("").is_err());
    }

    #[test]
    fn classify_keyword_fails() {
        assert!(IdentifierKind::classify("function").is_err());
    }

    #[test]
    fn classify_whitespace_fails() {
        assert!(IdentifierKind::classify("foo bar").is_err());
    }

    #[test]
    fn find_name_whole_word() {
        let refs = find_name_in_text("foo", "foo(bar, foo)");
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn find_name_skips_substrings() {
        let refs = find_name_in_text("foo", "foobar foo_x xfoo");
        assert_eq!(refs.len(), 0);
    }

    #[test]
    fn find_name_skips_comments() {
        let refs = find_name_in_text("foo", "foo // foo\nfoo");
        assert_eq!(refs.len(), 2); // first + last, not the one in comment
    }

    #[test]
    fn find_name_skips_strings() {
        let refs = find_name_in_text("foo", "foo \"foo\" foo");
        assert_eq!(refs.len(), 2); // first + last, not the one in string
    }

    #[test]
    fn source_edit_builds_replacements() {
        let refs = find_name_in_text("old", "old(old)");
        let edits = source_edit_from_references(&refs, "new");
        assert_eq!(edits.len(), 2);
        assert_eq!(edits[0].new_text, "new");
        assert_eq!(edits[1].new_text, "new");
    }
}
