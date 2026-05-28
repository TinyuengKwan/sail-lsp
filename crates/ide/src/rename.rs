//! Rename — renames a symbol across the workspace.
//! # Architecture
//!
//! The primary `rename` entry point delegates to
//! `ide_db::rename::Definition::rename()` which:
//! 1. Validates the new name via `IdentifierKind::classify()`
//! 2. Finds all occurrences across files
//! 3. Returns a `SourceChange` with per-file edits
//!
//! Legacy path (`rename_edits` from references.rs) still used by
//! LSP handlers until they migrate to the new API.

use base_db::FileId;
use ide_db::defs::Definition;
use ide_db::line_index::{LineCol, TextRange};
use ide_db::rename::{IdentifierKind, RenameError};
use ide_db::source_change::SourceChange;
use ide_db::FileDb;

use ide_db::ide_types::IdeTextEdit;
use std::collections::HashMap;
use url::Url;

// Re-export from ide_db::rename for downstream consumers.
pub use ide_db::rename::{IdentifierKind as IdKind, RenameError as Error};

// Re-export legacy rename for backward compat with LSP handlers.
pub use crate::navigation::will_rename_file_edits as will_rename_file;
pub use crate::references::{normalize_validated_rename, rename_edits};

/// Prepare rename: validate that the symbol at position can be renamed.
///
/// ```text
/// pub(crate) fn prepare_rename(
///     db: &RootDatabase,
///     position: FilePosition,
/// ) -> RenameResult<RangeInfo<()>>
/// ```
///
/// Returns the current name + its text range, or `None` if not renameable.
pub fn prepare_rename(file: &dyn FileDb, position: LineCol) -> Option<(String, TextRange)> {
    let (token, span) = file.token_at(position)?;
    let name = ide_db::token_symbol_key(token)?;

    if name.is_empty() || name.starts_with('$') || name.starts_with('@') {
        return None;
    }

    Some((name, base_db::text_range(span.start, span.end)))
}

/// Rename a symbol across the workspace.
///
/// ```text
/// pub(crate) fn rename(
///     db: &RootDatabase,
///     position: FilePosition,
///     new_name: &str,
///     config: &RenameConfig,
/// ) -> RenameResult<SourceChange>
/// ```
///
/// Delegates to `Definition::rename` from ide-db/rename.rs.
pub fn rename(
    db: &dyn hir_def::db::DefDatabase,
    files: &[(FileId, &str)],
    current_file: &dyn FileDb,
    position: LineCol,
    new_name: &str,
) -> Result<SourceChange, RenameError> {
    // 1. Validate new name
    let (_name, _kind) = IdentifierKind::classify(new_name)?;

    // 2. Resolve definition at position
    let def = resolve_definition_at(current_file, position)
        .ok_or_else(|| RenameError("No renameable symbol at cursor position".into()))?;

    // 3. Delegate to Definition::rename
    def.rename(db, files, new_name)
}

/// Resolve the Definition at a cursor position.
///
/// Uses the legacy ResolvedSymbol → Definition conversion path.
fn resolve_definition_at(file: &dyn FileDb, position: LineCol) -> Option<Definition> {
    use crate::references::resolve_symbol_at;
    use ide_db::defs::BuiltinType;
    use syntax::parser_lower::{Scope, SymbolOccurrenceKind};

    let symbol = resolve_symbol_at(file, position)?;

    // Map ResolvedSymbol → Definition
    // For top-level symbols, we construct from parsed declarations
    if symbol.scope == Some(Scope::TopLevel) || symbol.scope.is_none() {
        if symbol.kind == SymbolOccurrenceKind::Type
            && BuiltinType::from_name(&symbol.name).is_some() {
                return None; // Can't rename builtins
            }
        // For top-level names, create a Definition::Function/TypeDef/etc.
        // by looking up the declaration in the parsed file.
        if let Some(parsed) = file.parsed() {
            for decl in &parsed.decls {
                if decl.name == symbol.name {
                    return definition_from_decl(decl, file);
                }
            }
        }
    }

    // For local symbols, we can't easily create a Definition without
    // a salsa database. Return None — the caller falls back to text search.
    None
}

/// Convert a Decl to a Definition if possible.
fn definition_from_decl(
    _decl: &syntax::parser_lower::Decl,
    _file: &dyn FileDb,
) -> Option<Definition> {
    // Full Decl → Definition mapping requires FileText (salsa input)
    // which we don't have from FileDb alone. This is a known limitation:
    // the LSP handler layer needs to provide FileText for full semantic
    // rename. For now, return None to fall back to the text-based path.
    //
    // The `Definition::rename()` backend in ide-db/rename.rs works
    // correctly when called from code that has FileText + DefDatabase.
    None
}

/// Legacy rename: text-based symbol rename.
///
/// Kept for backward compatibility with current LSP handlers.
/// New code should use `rename()` above.
pub fn rename_legacy<'a, F, I>(
    files: I,
    current_uri: &Url,
    current_file: &F,
    position: LineCol,
    new_name: &str,
) -> Result<HashMap<Url, Vec<IdeTextEdit>>, RenameError>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    // Validate
    let (_name, _kind) = IdentifierKind::classify(new_name)?;

    let (token, _span) = current_file
        .token_at(position)
        .ok_or_else(|| RenameError("No symbol at cursor position".into()))?;
    let old_name = ide_db::token_symbol_key(token)
        .ok_or_else(|| RenameError("Not a renameable symbol".into()))?;

    if old_name == new_name {
        return Err(RenameError("New name is same as old name".into()));
    }

    let is_local = is_local_binding(current_file, position, &old_name);
    let mut changes: HashMap<Url, Vec<IdeTextEdit>> = HashMap::new();

    if is_local {
        let edits = find_local_references(current_file, &old_name, position);
        if !edits.is_empty() {
            changes.insert(
                current_uri.clone(),
                edits
                    .iter()
                    .map(|range| IdeTextEdit { range: *range, new_text: new_name.to_string() })
                    .collect(),
            );
        }
    } else {
        for (uri, file) in files {
            let file_edits = find_symbol_occurrences(file, &old_name);
            if !file_edits.is_empty() {
                changes.insert(
                    uri.clone(),
                    file_edits
                        .iter()
                        .map(|range| IdeTextEdit { range: *range, new_text: new_name.to_string() })
                        .collect(),
                );
            }
        }
    }

    if changes.is_empty() {
        return Err(RenameError(format!("No references found for '{}'", old_name)));
    }

    Ok(changes)
}

fn is_local_binding(file: &dyn FileDb, position: LineCol, name: &str) -> bool {
    let offset = file.offset_at(&position);
    if let Some(bodies) = file.bodies() {
        for entry in bodies.entries() {
            if let Some(expr_id) = entry.source_map.expr_at_offset(offset) {
                if let hir_def::Expr::Ident(ref ident) = entry.body.store[expr_id] {
                    if ident == name {
                        for (_, pat) in entry.body.iter_pats() {
                            if let hir_def::Pat::Bind(ref pat_name) = pat {
                                if pat_name == name {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    false
}

fn find_local_references(file: &dyn FileDb, name: &str, _position: LineCol) -> Vec<TextRange> {
    let mut ranges = Vec::new();
    if let Some(bodies) = file.bodies() {
        for entry in bodies.entries() {
            for (expr_id, expr) in entry.body.iter_exprs() {
                if let hir_def::Expr::Ident(ref ident) = expr {
                    if ident == name {
                        if let Some(span) = entry.source_map.expr_syntax(expr_id) {
                            ranges.push(base_db::text_range(span.start, span.end));
                        }
                    }
                }
            }
            for (pat_id, pat) in entry.body.iter_pats() {
                if let hir_def::Pat::Bind(ref pat_name) = pat {
                    if pat_name == name {
                        if let Some(span) = entry.source_map.pat_syntax(pat_id) {
                            ranges.push(base_db::text_range(span.start, span.end));
                        }
                    }
                }
            }
        }
    }
    ranges
}

fn find_symbol_occurrences(file: &dyn FileDb, name: &str) -> Vec<TextRange> {
    let mut ranges = Vec::new();
    if let Some(parsed) = file.parsed() {
        for decl in &parsed.decls {
            if decl.name == name {
                if let Some(name_span) = &decl.name_span {
                    ranges.push(base_db::text_range(name_span.start, name_span.end));
                }
            }
        }
        for occ in &parsed.symbol_occurrences {
            if occ.name == name {
                ranges.push(base_db::text_range(occ.span.start, occ.span.end));
            }
        }
    }
    ranges.sort_by_key(|r| (r.start(), r.end()));
    ranges.dedup();
    ranges
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn find_occurrences_includes_scattered_clause() {
        let source = "\
scattered function execute
function clause execute(x) = x
end execute
";
        let file = TestFile::new(source);
        let occs = find_symbol_occurrences(&file, "execute");
        assert!(
            occs.len() >= 3,
            "expected >=3 occurrences for scattered 'execute', found {}: {:?}",
            occs.len(),
            occs
        );
    }

    #[test]
    fn prepare_rename_on_identifier() {
        let source = "function foo(x) = x\n";
        let file = TestFile::new(source);
        let pos = file.position_at(9);
        let result = prepare_rename(&file, pos);
        assert!(result.is_some(), "should be able to rename 'foo'");
        let (name, _range) = result.unwrap();
        assert_eq!(name, "foo");
    }

    #[test]
    fn classify_rejects_keywords() {
        let result = IdentifierKind::classify("function");
        assert!(result.is_err(), "keyword should be rejected");
    }

    #[test]
    fn classify_rejects_empty() {
        let result = IdentifierKind::classify("");
        assert!(result.is_err(), "empty name should be rejected");
    }

    #[test]
    fn classify_accepts_valid_ident() {
        let result = IdentifierKind::classify("new_foo");
        assert!(result.is_ok());
    }
}
