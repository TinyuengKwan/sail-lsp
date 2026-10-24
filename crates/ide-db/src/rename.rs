//! Rename infrastructure for definitions.
//! This module provides the backend for semantic rename: given a
//! `Definition` and a new name, compute the multi-file `SourceChange`.
//! The IDE entry point (`ide/src/rename.rs`) delegates to these functions.

use base_db::FileId;

use crate::defs::Definition;
use crate::search::FileReference;
use crate::source_change::SourceChange;
use crate::text_edit::TextEdit;

mod bitfield;

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
        // Through `is_keyword`, which had no caller because this asked
        // the word list directly — and the list is not the question. A
        // rename to `module` passed here and produced a file `sail`
        // rejects before it type-checks anything.
        if crate::keywords::is_keyword(new_name) {
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

impl Definition {
    /// Rename this definition and every mention of it.
    ///
    /// rust-analyzer's rename is this method; ours was here in its shape
    /// and had no caller, and inside it scanned each file's raw text for
    /// the old name, skipping what looked like a comment or a string. It
    /// goes through `FindUsages` now — the same search find-references
    /// uses — so it renames the definition rather than the spelling.
    ///
    /// `scope` is how a local stays in its own file: a binding site is a
    /// position, and a position means nothing in another file.
    pub fn rename(
        &self,
        db: &dyn salsa::Database,
        files: &[(FileId, &dyn crate::FileDb)],
        scope: Option<crate::search::SearchScope>,
        new_name: &str,
    ) -> Result<SourceChange> {
        let (_validated, kind) = IdentifierKind::classify(new_name)?;

        if let Some(loc) = self.def_location() {
            if let Some((tree, item)) = loc.item(db) {
                if tree.generated.contains_key(&item) {
                    return Err(RenameError(
                        "A generated binding cannot be renamed independently of its generating declaration.".into(),
                    ));
                }
            }
        }

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

        let search = crate::search::FindUsages::new(self.clone(), old_name.as_str());
        let mut source_change = SourceChange::new();
        let canonical = self.canonical(db);
        let mut generated_searches = Vec::new();
        let bitfield = bitfield::BitfieldRename::prepare(db, files, &canonical, new_name)?;
        if let Some(bitfield) = &bitfield {
            for (def, name, renamed) in &bitfield.helpers {
                generated_searches
                    .push((crate::search::FindUsages::new(def.clone(), name), renamed.clone()));
            }
        }
        if let Some(loc) = canonical.def_location() {
            if let Some((tree, owner)) = loc.item(db) {
                if let Some(at) =
                    hir::source_order::at_offset(db, loc.file_text, owner.name_span(tree).start)
                {
                    for (&item, origin) in &tree.generated {
                        if origin.owner != owner || !origin.derived_name {
                            continue;
                        }
                        let name = item.name(tree).as_str();
                        let old_owner = owner.name(tree).as_str();
                        let renamed = if name == format!("num_of_{old_owner}") {
                            format!("num_of_{new_name}")
                        } else if origin.kind == hir::generated::Kind::BitfieldFunction {
                            let Some((prefix, suffix)) = ["Mk_", "_get_", "_update_", "_set_"]
                                .into_iter()
                                .find_map(|prefix| {
                                    name.strip_prefix(&format!("{prefix}{old_owner}"))
                                        .map(|suffix| (prefix, suffix))
                                })
                            else {
                                continue;
                            };
                            format!("{prefix}{new_name}{suffix}")
                        } else if let Some(suffix) = name.strip_prefix(old_owner) {
                            format!("{new_name}{suffix}")
                        } else {
                            continue;
                        };
                        if let Some(def) = hir::def_at_declaration_in_namespace(
                            db,
                            at,
                            name,
                            hir::source_order::bindings::Namespace::Value,
                        )
                        .and_then(Definition::from_def)
                        {
                            generated_searches
                                .push((crate::search::FindUsages::new(def, name), renamed));
                        }
                    }
                }
            }
        }

        for &(file_id, file) in files {
            if scope.as_ref().is_some_and(|scope| !scope.contains(file_id)) {
                continue;
            }
            let mut edits: Vec<TextEdit> = search
                .spans_in_file(file)
                .into_iter()
                .map(|(span, _)| TextEdit {
                    range: base_db::text_range(span.start, span.end),
                    new_text: new_name.to_owned(),
                })
                .collect();
            for (search, renamed) in &generated_searches {
                edits.extend(search.spans_in_file(file).into_iter().map(|(span, _)| TextEdit {
                    range: base_db::text_range(span.start, span.end),
                    new_text: renamed.clone(),
                }));
            }
            if let Some(bitfield) = &bitfield {
                edits.extend(bitfield.shared_edits(db, file)?);
            }
            if !edits.is_empty() {
                source_change.insert_source_edit(file_id, edits);
            }
        }

        Ok(source_change)
    }

    // There is no `range_for_rename`. It said in its doc that it
    // returned "the definition's name token" and returned
    // `mod_item.span(item_tree)` — the whole definition — so a
    // `prepareRename` through it would have selected the entire `function
    // foo(x) = x`. It had no caller: `ide::rename::prepare_rename` answers
    // from the token under the cursor, which is what the request asks
    // about and is right for a *use* as well as a definition.
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A name upstream's lexer refuses is not a valid rename target.
    ///
    /// `import` and `module` raise in upstream's keyword table
    /// (`lexer.mll:119-120`) rather than becoming tokens, so they are not
    /// in `SAIL_KEYWORDS` — and this check asked that list directly, so a
    /// rename to `module` was accepted and produced a file `sail` rejects
    /// before it type-checks anything.
    #[test]
    fn a_reserved_word_is_not_a_rename_target() {
        for name in ["module", "import"] {
            assert!(
                IdentifierKind::classify(name).is_err(),
                "{name} is reserved and must be refused"
            );
        }
        // And an ordinary keyword still is.
        assert!(IdentifierKind::classify("function").is_err());
        // While a name that merely contains one is fine.
        assert!(IdentifierKind::classify("module_id").is_ok());
    }

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
    fn source_edit_builds_replacements() {
        use crate::search::ReferenceCategory;
        let refs = vec![
            FileReference {
                range: base_db::text_range(0, 3),
                name: None,
                category: ReferenceCategory::READ,
            },
            FileReference {
                range: base_db::text_range(4, 7),
                name: None,
                category: ReferenceCategory::READ,
            },
        ];
        let edits = source_edit_from_references(&refs, "new");
        assert_eq!(edits.len(), 2);
        assert!(edits.iter().all(|e| e.new_text == "new"));
    }
}
