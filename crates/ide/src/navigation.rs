use ide_db::ide_types::{CallItem, FileLocation, IdeTextEdit, SymbolKind, TypeHierarchyItem};
use ide_db::line_index::TextRange;
use ide_db::{extract_symbol_decls, find_callable_signature, token_symbol_key, FileDb};

use parser::Span;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use url::Url;

/// Re-export from ide_types.
pub use ide_db::ide_types::CallEdge;

/// Collect call edges where the callee matches `target`, across
/// every file in `files`. walks the per-file `CallGraph`'s
/// site index (built from body arenas via 's
/// `CallableBodies`) instead of `parsed.call_sites`. The
/// resulting edges carry the same `(caller, caller_uri, callee,
/// call_range)` shape, but the data path now flows through the
/// Infrastructure layer rather than the older
/// `ParsedFile` symbol index.
pub fn call_edges_to<'a, F, I>(files: I, target: &str) -> Vec<CallEdge>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut out = Vec::new();
    for (uri, file) in files {
        let Some(graph) = file.callgraph() else {
            continue;
        };
        for site in graph.call_sites_to(target) {
            out.push(CallEdge {
                caller: site.caller.clone(),
                caller_uri: uri.clone(),
                callee: site.callee.clone(),
                call_range: base_db::text_range(site.callee_span.start, site.callee_span.end),
            });
        }
    }
    out
}

/// Collect call edges where the caller matches `source`, across
/// every file in `files`. walks the per-file `CallGraph`'s
/// site index instead of `parsed.call_sites`.
pub fn call_edges_from<'a, F, I>(files: I, source: &str) -> Vec<CallEdge>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut out = Vec::new();
    for (uri, file) in files {
        let Some(graph) = file.callgraph() else {
            continue;
        };
        for site in graph.call_sites_in(source) {
            out.push(CallEdge {
                caller: site.caller.clone(),
                caller_uri: uri.clone(),
                callee: site.callee.clone(),
                call_range: base_db::text_range(site.callee_span.start, site.callee_span.end),
            });
        }
    }
    out
}

/// Aggregate incoming call edges into per-caller items.
///
/// the same caller function are grouped into one `IncomingCallItem`
/// with multiple ranges.
pub fn incoming_calls<'a, F, I>(files: I, target: &str) -> Vec<ide_db::ide_types::IncomingCallItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let edges = call_edges_to(files, target);
    let mut by_caller: std::collections::HashMap<(String, Url), Vec<TextRange>> =
        std::collections::HashMap::new();
    for edge in edges {
        by_caller.entry((edge.caller, edge.caller_uri)).or_default().push(edge.call_range);
    }
    by_caller
        .into_iter()
        .map(|((caller, caller_uri), ranges)| ide_db::ide_types::IncomingCallItem {
            caller,
            caller_uri,
            ranges,
        })
        .collect()
}

/// Aggregate outgoing call edges into per-callee items.
///
/// target from within a function body are grouped.
pub fn outgoing_calls<'a, F, I>(files: I, source: &str) -> Vec<ide_db::ide_types::OutgoingCallItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let edges = call_edges_from(files, source);
    let mut by_callee: std::collections::HashMap<String, Vec<TextRange>> =
        std::collections::HashMap::new();
    for edge in edges {
        by_callee.entry(edge.callee).or_default().push(edge.call_range);
    }
    by_callee
        .into_iter()
        .map(|(callee, ranges)| ide_db::ide_types::OutgoingCallItem { callee, ranges })
        .collect()
}

pub fn call_hierarchy_item<'a, F, I>(files: I, uri_hint: &Url, name: &str) -> Option<CallItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut best: Option<(usize, Url, TextRange, Option<String>)> = None;
    for (uri, file) in files {
        let detail =
            find_callable_signature(std::iter::once((uri, file)), uri, name).map(|s| s.label);
        for span in symbol_definition_spans(file, name) {
            let range = base_db::text_range(span.start, span.end);
            let score = match (uri_hint.path_segments(), uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            };
            match &best {
                Some((best_score, _, _, _)) if *best_score > score => {}
                _ => best = Some((score, uri.clone(), range, detail.clone())),
            }
        }
    }

    let (_, uri, range, detail) = best?;
    Some(CallItem {
        name: name.to_string(),
        kind: SymbolKind::Function,
        detail,
        url: uri,
        range,
        selection_range: range,
        data: Some(serde_json::json!({ "name": name })),
    })
}

/// The name each `type_def` in this file declares, and where the name is.
pub(crate) fn type_decls(file: &dyn FileDb) -> HashMap<String, usize> {
    let Some((db, ft)) = file.file_text() else { return HashMap::new() };
    hir::Semantics::new(db)
        .type_decls(db, ft)
        .into_iter()
        .map(|(name, span, _)| (name, span.start))
        .collect()
}

/// How a type definition shows up in an outline, or `None` if it is not one.
/// Whether a definition of this kind *implements* a name, as
/// goto-implementation means it: a body, a clause, or an overload that
/// names the bodies to try.
fn implements(kind: hir::DefKind) -> bool {
    matches!(
        kind,
        hir::DefKind::Function
            | hir::DefKind::Mapping
            | hir::DefKind::Overload
            | hir::DefKind::ScatteredClause
    )
}

fn type_symbol_kind(kind: hir::DefKind) -> Option<SymbolKind> {
    Some(match kind {
        hir::DefKind::TypeAlias
        | hir::DefKind::Struct
        | hir::DefKind::Union
        | hir::DefKind::Bitfield
        | hir::DefKind::Newtype => SymbolKind::Struct,
        hir::DefKind::Enum => SymbolKind::Enum,
        _ => return None,
    })
}

/// Where `symbol_key` is defined in this file, as the span of each name.
fn symbol_definition_spans(file: &dyn FileDb, symbol_key: &str) -> Vec<Span> {
    let Some((db, ft)) = file.file_text() else { return Vec::new() };
    hir::Semantics::new(db).definition_spans(db, ft, symbol_key)
}

pub(crate) fn type_decls_with_kind(file: &dyn FileDb) -> HashMap<String, (usize, SymbolKind)> {
    let Some((db, ft)) = file.file_text() else { return HashMap::new() };
    hir::Semantics::new(db)
        .type_decls(db, ft)
        .into_iter()
        .filter_map(|(name, span, kind)| Some((name, (span.start, type_symbol_kind(kind)?))))
        .collect()
}

/// `(alias, aliased)` for each `type X = Y` in this file — the edges a type
/// hierarchy walks.
pub fn type_alias_edges(file: &dyn FileDb) -> Vec<(String, String)> {
    let Some((db, ft)) = file.file_text() else { return Vec::new() };
    hir::Semantics::new(db).type_alias_edges(db, ft)
}

pub fn type_hierarchy_item<'a, F, I>(
    files: I,
    uri_hint: &Url,
    name: &str,
) -> Option<TypeHierarchyItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut best: Option<(usize, Url, TextRange, SymbolKind)> = None;
    for (uri, file) in files {
        let Some((offset, kind)) = type_decls_with_kind(file).get(name).copied() else {
            continue;
        };
        let range = base_db::text_range(offset, offset + name.len());
        let score = match (uri_hint.path_segments(), uri.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        };
        match &best {
            Some((best_score, _, _, _)) if *best_score > score => {}
            _ => best = Some((score, uri.clone(), range, kind)),
        }
    }

    let (_, uri, range, kind) = best?;
    Some(TypeHierarchyItem {
        name: name.to_string(),
        kind,
        detail: Some("type".to_string()),
        url: uri,
        range,
        selection_range: range,
        data: Some(serde_json::json!({ "name": name })),
    })
}

pub fn type_supertypes<'a, F, I>(files: I, uri_hint: &Url, name: &str) -> Vec<TypeHierarchyItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    let names: HashSet<String> = files
        .iter()
        .flat_map(|(_, file)| type_alias_edges(*file as &dyn FileDb))
        .filter_map(|(sub, sup)| if sub == name { Some(sup) } else { None })
        .collect();

    names
        .into_iter()
        .filter_map(|super_name| type_hierarchy_item(files.iter().copied(), uri_hint, &super_name))
        .collect()
}

pub fn type_subtypes<'a, F, I>(files: I, uri_hint: &Url, name: &str) -> Vec<TypeHierarchyItem>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    let names: HashSet<String> = files
        .iter()
        .flat_map(|(_, file)| type_alias_edges(*file as &dyn FileDb))
        .filter_map(|(sub, sup)| if sup == name { Some(sub) } else { None })
        .collect();

    names
        .into_iter()
        .filter_map(|sub_name| type_hierarchy_item(files.iter().copied(), uri_hint, &sub_name))
        .collect()
}

pub fn type_name_candidates_at_position(
    file: &dyn FileDb,
    position: ide_db::LineCol,
) -> Vec<String> {
    let Some((token, _)) = file.token_at(position) else {
        return Vec::new();
    };
    let Some(name) = token_symbol_key(token) else {
        return Vec::new();
    };
    if name.starts_with('\'') {
        return Vec::new();
    }

    let mut names = vec![name.clone()];
    if let Some(ty) = crate::goto_type_definition::typed_bindings(file).get(&name).cloned() {
        names.push(ty);
    }
    names.sort();
    names.dedup();
    names
}

pub fn implementation_locations<'a, F, I>(
    files: I,
    uri_hint: &Url,
    name: &str,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut locations = Vec::new();
    for (uri, file) in files {
        let Some(tree) = file.item_tree() else {
            continue;
        };
        for &id in tree.top_level_items() {
            if id.name(tree).as_str() != name || !implements(id.def_kind(tree)) {
                continue;
            }
            let span = id.span(tree);
            locations.push(ide_db::ide_types::FileLocation {
                url: uri.clone(),
                range: base_db::text_range(span.start, span.end),
            });
        }
    }

    locations.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        })
    });
    locations
}

pub fn symbol_definition_locations<'a, F, I>(
    files: I,
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut definitions = files
        .into_iter()
        .flat_map(|(uri, file)| {
            symbol_definition_spans(file, symbol_key)
                .into_iter()
                .map(move |span| ide_db::span::file_location_from_span(uri, span))
        })
        .collect::<Vec<_>>();

    definitions.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    definitions
}

pub fn symbol_declaration_locations<'a, F, I>(
    files: I,
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    // From the item tree, whose `name_span` is the identifier goto should
    // land on. The scrape searched `callable_heads` for a matching head and,
    // failing that, searched the declaration's own text for the name.
    let mut declarations = files
        .into_iter()
        .flat_map(|(uri, file)| {
            let Some((db, ft)) = file.file_text() else {
                return Vec::new().into_iter();
            };
            hir::Semantics::new(db)
                .declaration_spans(db, ft, symbol_key)
                .into_iter()
                .map(|span| ide_db::span::file_location_from_span(uri, span))
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect::<Vec<_>>();

    declarations.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    declarations
}

/// Resolve a workspace symbol: given a URI + name + kind, find the precise
/// byte-offset location. Returns `Some(WorkspaceSymbol)` with a resolved
/// `FileLocation` if found.
pub fn resolve_workspace_symbol<'a, F, I>(
    name: &str,
    kind: SymbolKind,
    target_uri: &Url,
    files: I,
) -> Option<ide_db::ide_types::WorkspaceSymbol>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    for (uri, file) in files {
        if uri != target_uri {
            continue;
        }
        if let Some(decl) = extract_symbol_decls(file)
            .into_iter()
            .find(|decl| decl.name == name && decl.kind == kind)
        {
            let range = base_db::text_range(decl.offset, decl.offset + decl.name.len());
            return Some(ide_db::ide_types::WorkspaceSymbol {
                name: name.to_string(),
                kind,
                location: FileLocation { url: uri.clone(), range },
            });
        }
        if let Some(span) = symbol_definition_spans(file, name).first().copied() {
            return Some(ide_db::ide_types::WorkspaceSymbol {
                name: name.to_string(),
                kind,
                location: FileLocation {
                    url: uri.clone(),
                    range: base_db::text_range(span.start, span.end),
                },
            });
        }
    }
    None
}

fn basename_from_uri(uri: &str) -> Option<String> {
    Url::parse(uri).ok().and_then(|url| {
        url.path_segments().and_then(|mut segments| segments.next_back().map(str::to_string))
    })
}

/// Rename file pairs: `(old_uri_str, new_uri_str)`.
pub fn will_rename_file_edits<'a, F, I>(
    files: I,
    rename_pairs_raw: &[(String, String)],
) -> Option<HashMap<Url, Vec<IdeTextEdit>>>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let rename_pairs: Vec<(String, String)> = rename_pairs_raw
        .iter()
        .filter_map(|(old_uri, new_uri)| {
            let old_name = basename_from_uri(old_uri)?;
            let new_name = basename_from_uri(new_uri)?;
            if old_name == new_name {
                return None;
            }
            Some((format!("\"{old_name}\""), format!("\"{new_name}\"")))
        })
        .collect();

    if rename_pairs.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<IdeTextEdit>> = HashMap::new();
    for (uri, file) in files {
        let text = file.text();
        let mut edits = Vec::new();
        for (old_text, new_text) in &rename_pairs {
            for (start, _) in text.match_indices(old_text) {
                let end = start + old_text.len();
                edits.push(IdeTextEdit {
                    range: base_db::text_range(start, end),
                    new_text: new_text.clone(),
                });
            }
        }
        if !edits.is_empty() {
            changes.insert(uri.clone(), edits);
        }
    }

    if changes.is_empty() {
        return None;
    }
    Some(changes)
}

// There were four `*_dyn` wrappers here, written "so callers that only
// have trait objects (e.g. `hir::Semantics`) can use them without going
// through turbofish". No such caller ever existed — every handler reaches
// the generic functions above directly. Deleted rather than kept: a
// wrapper written for a hypothetical caller is ballast, and the generic
// ones take the same arguments if one ever turns up.

// The 'symbol_locations_use_identifier_spans' test that used to live
// here was tightly coupled to sail_server::state::File (it used
// File::new directly). Equivalent end-to-end coverage exists in
// sail_server::tests; the inline test was dropped during the
// move rather than reproducing the full File pipeline behind
// a TestFile stub.

//
// These use SymbolIndex for O(1) cross-file lookups
// instead of iterating all files.

use hir::DefKind;
use ide_db::symbol_index::SymbolIndex;

/// Lookup definition locations from index. O(1) by name.
pub fn definition_locations_indexed(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Vec<FileLocation> {
    definition_locations_indexed_in(index, symbol_key, uri_hint, url_of, false)
}

/// The same, told which namespace the use site is in.
///
/// Sail has two — `PerNs` is Types and Values — and this matched on the
/// spelling alone, so `x : int` in a workspace that declares `val int : ..`
/// navigated to the function. A wrong answer is worse than none.
pub fn definition_locations_indexed_in(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
    in_type_position: bool,
) -> Vec<FileLocation> {
    let value_only = |kind: DefKind| {
        matches!(
            kind,
            DefKind::Function
                | DefKind::ValSpec
                | DefKind::Mapping
                | DefKind::MappingSpec
                | DefKind::Register
                | DefKind::Let
                | DefKind::Var
                | DefKind::Overload
        )
    };
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| !in_type_position || !value_only(e.kind))
        .filter(|e| {
            matches!(
                e.kind,
                DefKind::Function
                    | DefKind::Mapping
                    | DefKind::TypeAlias
                    | DefKind::Struct
                    | DefKind::Union
                    | DefKind::Enum
                    | DefKind::Bitfield
                    | DefKind::Newtype
                    | DefKind::Register
                    | DefKind::Let
                    | DefKind::Var
                    | DefKind::Overload
                    | DefKind::ScatteredHead
            )
        })
        .filter(|e| !e.is_clause) // Prefer non-clause definitions
        .filter_map(|e| {
            Some(FileLocation {
                url: url_of(e.file_id)?,
                // Navigation lands on the name, not on the keyword that opens
                // the definition (`NavigationTarget::focus_range`'s contract).
                range: base_db::text_range(e.name_span.start, e.name_span.end),
            })
        })
        .collect();

    // If no non-clause definitions found, include clauses. Still only in
    // the namespace asked for: this fallback is why a type position kept
    // reaching a `val`, filtered out one line above.
    if locs.is_empty() {
        locs = entries
            .iter()
            .filter(|e| !in_type_position || !value_only(e.kind))
            .filter_map(|e| {
                Some(FileLocation {
                    url: url_of(e.file_id)?,
                    // Navigation lands on the name, not on the keyword that opens
                    // the definition (`NavigationTarget::focus_range`'s contract).
                    range: base_db::text_range(e.name_span.start, e.name_span.end),
                })
            })
            .collect();
    }

    // Sort: prefer same file as hint
    locs.sort_by_key(|loc| {
        Reverse(match (uri_hint.path_segments(), loc.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    locs
}

/// Lookup declaration locations (ValSpec, MappingSpec) from index.
pub fn declaration_locations_indexed(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Vec<FileLocation> {
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| matches!(e.kind, DefKind::ValSpec | DefKind::MappingSpec))
        .filter_map(|e| {
            Some(FileLocation {
                url: url_of(e.file_id)?,
                // Navigation lands on the name, not on the keyword that opens
                // the definition (`NavigationTarget::focus_range`'s contract).
                range: base_db::text_range(e.name_span.start, e.name_span.end),
            })
        })
        .collect();

    if locs.is_empty() {
        // Fallback: use definitions
        return definition_locations_indexed(index, symbol_key, uri_hint, url_of);
    }
    locs.sort_by_key(|loc| {
        Reverse(match (uri_hint.path_segments(), loc.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    locs
}

/// Lookup implementation locations (Function, Mapping definitions) from index.
pub fn implementation_locations_indexed(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
    url_of: &dyn Fn(base_db::FileId) -> Option<Url>,
) -> Vec<FileLocation> {
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| matches!(e.kind, DefKind::Function | DefKind::Mapping | DefKind::Overload))
        .filter_map(|e| {
            Some(FileLocation {
                url: url_of(e.file_id)?,
                // Navigation lands on the name, not on the keyword that opens
                // the definition (`NavigationTarget::focus_range`'s contract).
                range: base_db::text_range(e.name_span.start, e.name_span.end),
            })
        })
        .collect();

    locs.sort_by_key(|loc| {
        Reverse(match (uri_hint.path_segments(), loc.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    locs
}
