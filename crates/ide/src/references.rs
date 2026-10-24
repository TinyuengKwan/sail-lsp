use std::collections::HashMap;

use base_db::FileId;
use ide_db::defs::Definition;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::search::ReferenceCategory;
use ide_db::{FileDb, LineCol};
use std::collections::BTreeMap;

use parser::Span;
use url::Url;

/// Result of a find-all-references operation.
///
/// rust-analyzer carries `declaration: Option<Declaration>`, because a Rust
/// item is declared once. A Sail name can be declared many times and in
/// several files: a `val` signature, a `scattered function` head, every
/// `function clause`, every `mapping clause`, and the `end` that closes
/// them. Renaming has to reach all of them or the file stops compiling, so
/// this is a list.
#[derive(Debug, Clone)]
pub struct ReferenceSearchResult {
    /// Every site that declares this name, in every file.
    pub declarations: Vec<Declaration>,
    /// References grouped by file.
    pub references: BTreeMap<FileId, Vec<(TextRange, ReferenceCategory)>>,
}

/// A declaration site for a symbol.
///
/// ```text
/// pub struct Declaration {
///     pub nav: NavigationTarget,
///     pub is_mut: bool,
/// }
/// ```
///
/// Simplified: uses (FileId, TextRange, Name) instead of NavigationTarget
/// until NavigationTarget is introduced.
#[derive(Debug, Clone)]
pub struct Declaration {
    pub file_id: FileId,
    pub range: TextRange,
    pub name: String,
    pub is_mut: bool,
}

/// Find all references to the symbol at the given position.
/// Returns one `ReferenceSearchResult` per resolved definition
/// (usually one, but can be multiple for overloaded names).
pub fn find_all_refs(
    files: &[(FileId, &dyn FileDb)],
    target_file_id: FileId,
    target_file: &dyn FileDb,
    position: LineCol,
    search_scope: Option<ide_db::search::SearchScope>,
) -> Option<Vec<ReferenceSearchResult>> {
    // A local never leaves its body, so it is answered here in full and the
    // cross-file walk below is skipped entirely.
    if let Some(spans) = local_spans_through_hir(target_file, position) {
        let mut declarations = Vec::new();
        let mut references: BTreeMap<FileId, Vec<(TextRange, ReferenceCategory)>> =
            BTreeMap::default();
        let name = target_file
            .token_at(position)
            .and_then(|(t, _)| ide_db::token_symbol_key(t))
            .unwrap_or_default();
        for (span, is_decl) in spans {
            let range = base_db::text_range(span.start, span.end);
            if is_decl {
                declarations.push(Declaration {
                    file_id: target_file_id,
                    range,
                    name: name.clone(),
                    is_mut: false,
                });
            } else {
                references
                    .entry(target_file_id)
                    .or_default()
                    .push((range, ReferenceCategory::READ));
            }
        }
        return Some(vec![ReferenceSearchResult { declarations, references }]);
    }

    let defs = find_defs(target_file, position)?;

    let mut results = Vec::new();
    for def in &defs {
        let mut declarations =
            build_declaration(def, target_file, target_file_id).into_iter().collect::<Vec<_>>();

        // Collect references across all files
        let mut references: BTreeMap<FileId, Vec<(TextRange, ReferenceCategory)>> =
            BTreeMap::default();

        // Resolved in the file the cursor is in, then carried.
        let name = search_name(def, target_file);
        for &(file_id, file) in files {
            let is_local = def.is_local();
            if is_local && file_id != target_file_id {
                continue;
            }
            if let Some(ref scope) = search_scope {
                if !scope.contains(file_id) {
                    continue;
                }
            }
            for (span, is_decl) in spans_in_file(file, def, &name, true) {
                if !is_decl {
                    continue;
                }
                let range = base_db::text_range(span.start, span.end);
                if !declarations.iter().any(|d| d.file_id == file_id && d.range == range) {
                    declarations.push(Declaration {
                        file_id,
                        range,
                        name: name.clone(),
                        is_mut: false,
                    });
                }
            }
            let spans = spans_in_file(file, def, &name, false);
            for (span, is_write) in spans {
                let category =
                    if is_write { ReferenceCategory::WRITE } else { ReferenceCategory::READ };
                references
                    .entry(file_id)
                    .or_default()
                    .push((base_db::text_range(span.start, span.end), category));
            }
        }

        // A name can reach here from more than one of the three sources — a
        // call site is also an `Expr::Ident` in the body that makes it — so
        // the same range must not be reported twice.
        declarations.sort_by_key(|d| (d.file_id, base_db::range_start(d.range)));
        declarations.dedup_by_key(|d| (d.file_id, d.range));
        let declared: std::collections::HashSet<(FileId, TextRange)> =
            declarations.iter().map(|d| (d.file_id, d.range)).collect();
        for (&file_id, refs) in references.iter_mut() {
            refs.sort_by_key(|(r, _)| base_db::range_start(*r));
            refs.dedup_by_key(|(r, _)| *r);
            refs.retain(|(r, _)| !declared.contains(&(file_id, *r)));
        }
        results.push(ReferenceSearchResult { declarations, references });
    }

    if results.is_empty() {
        None
    } else {
        Some(results)
    }
}

/// Resolve definitions at a position.
///
/// Returns the resolved symbols (definitions) at the cursor.
pub fn find_defs(file: &dyn FileDb, position: LineCol) -> Option<Vec<Definition>> {
    let symbol = resolve_symbol_at(file, position)?;
    Some(vec![symbol])
}

/// Build a Declaration for a definition that is one binding in one body.
fn build_declaration(def: &Definition, file: &dyn FileDb, file_id: FileId) -> Option<Declaration> {
    let target = def.binding_span()?;
    Some(Declaration {
        file_id,
        range: base_db::text_range(target.start, target.end),
        name: search_name(def, file),
        is_mut: false,
    })
}

fn sort_and_dedup_spans(spans: &mut Vec<(Span, bool)>) {
    // One occurrence per position: the same name at the same offset can be
    // reported by more than one source — a call is both an `Expr::Ident`
    // and a call-site — and the two differ only in whether they are
    // marked a declaration. Deduping the pair kept both, which showed up
    // the moment call sites started carrying a real span instead of 0..0.
    spans.sort_unstable_by_key(|(span, is_write)| (span.start, span.end, !*is_write));
    spans.dedup_by_key(|(span, _)| (span.start, span.end));
}

/// What the name under the cursor is.
///
/// One classifier, in `ide-db`, as rust-analyzer has it: this used to live
/// here and answer in `ResolvedSymbol { name: String, .. }`, so every
/// feature that wanted a definition got a name and a byte span to
/// disambiguate it with.
pub fn resolve_symbol_at(file: &dyn FileDb, position: LineCol) -> Option<Definition> {
    Definition::classify_at(file, position)
}

/// The text to search for, which is the definition's own name.
///
/// Resolved against the file the cursor was in, once, and carried from
/// there — rust-analyzer's `FindUsages` does the same with one
/// `def.name(sema.db)`. Asking each searched file instead means asking a
/// database that does not hold this definition, and a `DefLocation` read
/// through the wrong database names whatever sits at that index there.
fn search_name(def: &Definition, file: &dyn FileDb) -> String {
    file.file_text()
        .and_then(|(db, _)| def.name(db))
        .map(|n| n.as_str().to_owned())
        .unwrap_or_default()
}

/// Every mention of `def` in `file`, searching for `name`.
///
/// Through `FindUsages`, which is rust-analyzer's search and was dead
/// here: it had no caller outside its own tests while this module walked
/// the HIR itself.
fn spans_in_file(
    file: &dyn FileDb,
    def: &Definition,
    name: &str,
    include_declarations: bool,
) -> Vec<(Span, bool)> {
    let search = ide_db::search::FindUsages::new(def.clone(), name);
    let mut spans = search.spans_in_file(file);
    if !include_declarations {
        spans.retain(|(_, is_decl)| !is_decl);
    }
    sort_and_dedup_spans(&mut spans);
    spans
}

/// Every mention of the local binding under the cursor.
///
/// Through `Semantics::local_mentions`. Find-all-references used to
/// resolve locals the way it resolves top-level names — by matching the
/// flat occurrence scrape on a `target_span` it does not fill in — so a
/// local found nothing at all, and references, rename and highlight were
/// dead on every variable, parameter and pattern binding in the language.
pub(crate) fn local_spans_through_hir(
    file: &dyn FileDb,
    position: LineCol,
) -> Option<Vec<(Span, bool)>> {
    let (token, _) = file.token_at(position)?;
    let name = ide_db::token_symbol_key(token)?;
    let (db, ft) = file.file_text()?;
    let offset = file.offset_at(&position);
    let mut spans = hir::Semantics::new(db).local_mentions(db, ft, &name, offset)?;
    sort_and_dedup_spans(&mut spans);
    Some(spans)
}

/// Every mention of `def` in `file`, where `file` is the one the cursor
/// was in — otherwise the name has to come from there, through
/// `spans_in_file`.
pub fn symbol_spans_for_file(
    file: &dyn FileDb,
    def: &Definition,
    include_declarations: bool,
) -> Vec<(Span, bool)> {
    spans_in_file(file, def, &search_name(def, file), include_declarations)
}

// There is no `declaration_spans_for_file`. Its doc claimed it was "what
// rename needs to edit and what find-references needs to label", and
// neither called it: rename edits every occurrence, and the label rides
// on the `is_decl` flag `spans_in_file` already returns beside each span.
// A filter over a live function is not a second function.

pub fn reference_locations<'a, F, I>(
    files: I,
    current_uri: &Url,
    def: &Definition,
    include_declarations: bool,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let local_only = def.is_local();
    let mut locations = Vec::new();

    let files: Vec<_> = files.into_iter().collect();
    let name = files
        .iter()
        .find(|(uri, _)| *uri == current_uri)
        .map(|(_, file)| search_name(def, *file))
        .unwrap_or_default();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in spans_in_file(file, def, &name, include_declarations) {
            locations.push(ide_db::span::file_location_from_span(uri, span));
        }
    }

    locations.sort_by_key(|location| {
        std::cmp::Reverse(match (current_uri.path_segments(), location.url.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(lhs, rhs)| lhs == rhs).count(),
            _ => 0,
        })
    });
    locations
}

pub fn rename_edits<'a, F, I>(
    files: I,
    current_uri: &Url,
    def: &Definition,
    new_text: &str,
) -> HashMap<Url, Vec<IdeTextEdit>>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let local_only = def.is_local();
    let mut changes: HashMap<Url, Vec<IdeTextEdit>> = HashMap::new();

    let files: Vec<_> = files.into_iter().collect();
    let name = files
        .iter()
        .find(|(uri, _)| *uri == current_uri)
        .map(|(_, file)| search_name(def, *file))
        .unwrap_or_default();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in spans_in_file(file, def, &name, true) {
            changes.entry(uri.clone()).or_default().push(IdeTextEdit {
                range: base_db::text_range(span.start, span.end),
                new_text: new_text.to_string(),
            });
        }
    }

    changes.retain(|_, edits| !edits.is_empty());
    changes
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'?' | b'\'' | b'~')
}

fn is_valid_identifier_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|ch| ch.is_ascii() && is_identifier_byte(ch as u8))
}

pub fn normalize_validated_rename(
    token: &parser::Token,
    requested_name: &str,
    keywords: &[&str],
) -> std::result::Result<Option<String>, String> {
    let (base_name, quoted) = if let Some(stripped) = requested_name.strip_prefix('\'') {
        (stripped, true)
    } else {
        (requested_name, false)
    };
    if !is_valid_identifier_name(base_name) {
        return Err("new_name must be a valid identifier".to_string());
    }
    if keywords.contains(&base_name) {
        return Err("new_name cannot be a Sail keyword".to_string());
    }

    match token {
        parser::Token::TyVal(_) => {
            if quoted {
                Ok(Some(requested_name.to_string()))
            } else {
                Ok(Some(format!("'{base_name}")))
            }
        }
        _ if quoted => {
            Err("type variable marker (') is only valid when renaming type variables".to_string())
        }
        _ => Ok(Some(base_name.to_string())),
    }
}

// ---- dyn-dispatch wrapper ----

// `reference_locations_dyn` was here, with the same story as the four
// in `navigation.rs`: a trait-object wrapper for a caller that never
// existed.

#[cfg(test)]
mod through_analysis {
    use crate::fixture::reference_texts;

    /// A local binding that shadows a top-level name is a different symbol.
    #[test]
    fn a_shadowing_local_is_not_the_top_level_name() {
        let found = reference_texts(
            r#"
//- /main.sail
default Order dec
let x : int = 1
function f() -> int = { let x : int = 2; x$0 }
function g() -> int = x
"#,
        );
        assert_eq!(found.len(), 2, "expected the binding and its one use: {found:?}");
    }

    /// A name declared in a file this one does not `$include` is a different
    /// symbol, even spelled the same.
    #[test]
    fn an_unincluded_file_is_a_different_symbol() {
        let found = reference_texts(
            r#"
//- /a.sail
default Order dec
function shared() -> int = 1
//- /b.sail
default Order dec
function shared$0() -> int = 2
"#,
        );
        assert!(
            found.iter().all(|f| f.starts_with("1:")),
            "reached into a file b.sail does not include: {found:?}"
        );
    }
}
