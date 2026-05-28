use std::collections::HashMap;

use base_db::FileId;
use ide_db::ide_types::IdeTextEdit;
use ide_db::line_index::TextRange;
use ide_db::search::ReferenceCategory;
use ide_db::{FileDb, LineCol};
use std::collections::BTreeMap;

use parser::Span;
use syntax::parser_lower::{Scope, SymbolOccurrenceKind};
use url::Url;

/// Result of a find-all-references operation.
///
/// ```text
/// pub struct ReferenceSearchResult {
///     pub declaration: Option<Declaration>,
///     pub references: IntMap<FileId, Vec<(TextRange, ReferenceCategory)>>,
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ReferenceSearchResult {
    /// The declaration/definition site, if found.
    pub declaration: Option<Declaration>,
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
    let defs = find_defs(target_file, position)?;

    let mut results = Vec::new();
    for def in &defs {
        // Build declaration info
        let declaration = build_declaration(def, target_file_id);

        // Collect references across all files
        let mut references: BTreeMap<FileId, Vec<(TextRange, ReferenceCategory)>> =
            BTreeMap::default();

        for &(file_id, file) in files {
            let is_local = def.target_span.is_some() || def.kind == SymbolOccurrenceKind::TypeVar;
            if is_local && file_id != target_file_id {
                continue;
            }
            if let Some(ref scope) = search_scope {
                if !scope.contains(file_id) {
                    continue;
                }
            }
            let spans = symbol_spans_for_file(file, def, false);
            for (span, is_write) in spans {
                let category =
                    if is_write { ReferenceCategory::WRITE } else { ReferenceCategory::READ };
                references
                    .entry(file_id)
                    .or_default()
                    .push((base_db::text_range(span.start, span.end), category));
            }
        }

        results.push(ReferenceSearchResult { declaration, references });
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
pub fn find_defs(file: &dyn FileDb, position: LineCol) -> Option<Vec<ResolvedSymbol>> {
    let symbol = resolve_symbol_at(file, position)?;
    Some(vec![symbol])
}

/// Build a Declaration from a ResolvedSymbol.
fn build_declaration(symbol: &ResolvedSymbol, file_id: FileId) -> Option<Declaration> {
    let target = symbol.target_span?;
    Some(Declaration {
        file_id,
        range: base_db::text_range(target.start, target.end),
        name: symbol.name.clone(),
        is_mut: false,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedSymbol {
    pub name: String,
    pub kind: SymbolOccurrenceKind,
    pub scope: Option<Scope>,
    pub target_span: Option<Span>,
}

fn sort_and_dedup_spans(spans: &mut Vec<(Span, bool)>) {
    spans.sort_unstable_by_key(|(span, is_write)| (span.start, span.end, !*is_write));
    spans.dedup();
}

fn matches_symbol(
    occurrence: &syntax::parser_lower::SymbolOccurrence,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> bool {
    if occurrence.kind != symbol.kind {
        return false;
    }
    if !include_declarations && occurrence.role.is_some() {
        return false;
    }

    // Local symbols match by target_span (definition site).
    // Top-level symbols match by name (cross-file resolution).
    if symbol.scope == Some(Scope::TopLevel) {
        // Top-level: match any occurrence of the same name that is
        // either top-level itself or unscoped (references in expressions).
        return occurrence.name == symbol.name && occurrence.scope != Some(Scope::Local);
    }

    // Local symbols: if we have a target_span, match occurrences
    // that point to the same definition site.
    if let Some(target_span) = symbol.target_span {
        return occurrence.target_span == Some(target_span);
    }

    occurrence.name == symbol.name
}

pub fn resolve_symbol_at(file: &dyn FileDb, position: LineCol) -> Option<ResolvedSymbol> {
    let (_, span) = file.token_at(position)?;
    let parsed = file.parsed()?;
    parsed
        .symbol_occurrences
        .iter()
        .filter(|occurrence| occurrence.span == *span)
        .max_by_key(|occurrence| {
            (
                occurrence.role.is_some(),
                occurrence.target_span.is_some(),
                occurrence.scope == Some(Scope::Local),
            )
        })
        .map(|occurrence| ResolvedSymbol {
            name: occurrence.name.clone(),
            kind: occurrence.kind,
            scope: occurrence.scope,
            target_span: occurrence.target_span,
        })
}

pub fn symbol_spans_for_file(
    file: &dyn FileDb,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> Vec<(Span, bool)> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };

    let mut spans = parsed
        .symbol_occurrences
        .iter()
        .filter(|occurrence| matches_symbol(occurrence, symbol, include_declarations))
        .map(|occurrence| (occurrence.span, occurrence.role.is_some()))
        .collect::<Vec<_>>();
    sort_and_dedup_spans(&mut spans);
    spans
}

pub fn reference_locations<'a, F, I>(
    files: I,
    current_uri: &Url,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let local_only = symbol.target_span.is_some() || symbol.kind == SymbolOccurrenceKind::TypeVar;
    let mut locations = Vec::new();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in symbol_spans_for_file(file, symbol, include_declarations) {
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
    symbol: &ResolvedSymbol,
    new_text: &str,
) -> HashMap<Url, Vec<IdeTextEdit>>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let local_only = symbol.target_span.is_some() || symbol.kind == SymbolOccurrenceKind::TypeVar;
    let mut changes: HashMap<Url, Vec<IdeTextEdit>> = HashMap::new();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in symbol_spans_for_file(file, symbol, true) {
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

pub fn reference_locations_dyn(
    files: &[(&Url, &dyn FileDb)],
    current_uri: &Url,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> Vec<ide_db::ide_types::FileLocation> {
    let local_only = symbol.target_span.is_some() || symbol.kind == SymbolOccurrenceKind::TypeVar;
    let mut locations = Vec::new();

    for (uri, file) in files {
        if local_only && *uri != current_uri {
            continue;
        }
        for (span, _) in symbol_spans_for_file(*file, symbol, include_declarations) {
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
