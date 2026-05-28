use ide_db::ide_types::{CallItem, FileLocation, IdeTextEdit, SymbolKind, TypeHierarchyItem};
use ide_db::line_index::TextRange;
use ide_db::{extract_symbol_decls, find_callable_signature, token_symbol_key, FileDb};

use parser::Span;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use syntax::parser_lower::{DeclRole, Scope};
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

fn type_decls(file: &dyn FileDb) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let Some(parsed) = file.parsed() else {
        return out;
    };
    for decl in &parsed.decls {
        if decl.scope != syntax::parser_lower::Scope::TopLevel {
            continue;
        }
        if matches!(
            decl.kind,
            syntax::parser_lower::DeclKind::Type
                | syntax::parser_lower::DeclKind::Struct
                | syntax::parser_lower::DeclKind::Union
                | syntax::parser_lower::DeclKind::Enum
                | syntax::parser_lower::DeclKind::Bitfield
                | syntax::parser_lower::DeclKind::Newtype
        ) {
            out.insert(decl.name.clone(), decl.span.start);
        }
    }
    out
}

fn symbol_definition_spans(file: &dyn FileDb, symbol_key: &str) -> Vec<Span> {
    // Try ParsedFile first (has local scope bindings + decl roles)
    if let Some(parsed) = file.parsed() {
        let mut spans: Vec<Span> = parsed
            .decls
            .iter()
            .filter(|decl| {
                decl.name == symbol_key
                    && decl.role == DeclRole::Definition
                    && match decl.kind {
                        syntax::parser_lower::DeclKind::Let
                        | syntax::parser_lower::DeclKind::Var
                        | syntax::parser_lower::DeclKind::Parameter => {
                            decl.scope == Scope::TopLevel
                        }
                        _ => true,
                    }
            })
            .map(|decl| {
                // Definition location points to the identifier, not the full decl.
                parsed
                    .callable_heads
                    .iter()
                    .find(|h| {
                        h.name == decl.name
                            && h.name_span.start >= decl.span.start
                            && h.name_span.end <= decl.span.end
                    })
                    .map(|h| h.name_span)
                    .unwrap_or_else(|| {
                        let text = file.text();
                        let name_offset = text[decl.span.start..decl.span.end]
                            .find(&decl.name)
                            .map(|o| decl.span.start + o)
                            .unwrap_or(decl.span.start);
                        Span::new(name_offset, name_offset + decl.name.len())
                    })
            })
            .collect();
        if !spans.is_empty() {
            spans.sort_unstable_by_key(|span| (span.start, span.end));
            spans.dedup();
            return spans;
        }
    }

    // Fallback: use ItemTree (works when parsed() is None, e.g. SalsaFile path)
    if let Some(item_tree) = file.item_tree() {
        let spans: Vec<Span> = item_tree
            .top_level_items()
            .iter()
            .filter(|id| id.name(item_tree).as_str() == symbol_key && !id.is_clause(item_tree))
            .map(|id| id.span(item_tree))
            .collect();
        if !spans.is_empty() {
            return spans;
        }
        // Also check clauses as fallback
        return item_tree
            .top_level_items()
            .iter()
            .filter(|id| id.name(item_tree).as_str() == symbol_key)
            .map(|id| id.span(item_tree))
            .collect();
    }

    Vec::new()
}

fn type_decls_with_kind(file: &dyn FileDb) -> HashMap<String, (usize, SymbolKind)> {
    let mut out = HashMap::new();
    let Some(parsed) = file.parsed() else {
        return out;
    };
    for decl in &parsed.decls {
        if decl.scope != syntax::parser_lower::Scope::TopLevel {
            continue;
        }
        let Some(kind) = (match decl.kind {
            syntax::parser_lower::DeclKind::Type
            | syntax::parser_lower::DeclKind::Struct
            | syntax::parser_lower::DeclKind::Union
            | syntax::parser_lower::DeclKind::Bitfield
            | syntax::parser_lower::DeclKind::Newtype => Some(SymbolKind::Struct),
            syntax::parser_lower::DeclKind::Enum => Some(SymbolKind::Enum),
            _ => None,
        }) else {
            continue;
        };
        out.insert(decl.name.clone(), (decl.span.start, kind));
    }
    out
}

pub fn type_alias_edges(file: &dyn FileDb) -> Vec<(String, String)> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    parsed.type_aliases.iter().map(|a| (a.sub.clone(), a.sup.clone())).collect()
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
    if let Some(ty) = typed_bindings(file).get(&name).cloned() {
        names.push(ty);
    }
    names.sort();
    names.dedup();
    names
}

pub fn typed_bindings(file: &dyn FileDb) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let Some(parsed) = file.parsed() {
        for decl in &parsed.decls {
            if !matches!(
                decl.kind,
                syntax::parser_lower::DeclKind::Parameter
                    | syntax::parser_lower::DeclKind::Let
                    | syntax::parser_lower::DeclKind::Var
            ) {
                continue;
            }

            // Use name_span (binding identifier) for PatId matching,
            // not the full declaration span. name_span aligns with Body's pattern spans.
            let lookup_span = decl.name_span.unwrap_or(decl.span);
            if let Some(ty) = file.binding_type_text(lookup_span) {
                out.insert(decl.name.clone(), ty);
            }
        }
    }

    let Some(parsed) = file.parsed() else {
        return out;
    };
    let text = file.text();
    for binding in &parsed.typed_bindings {
        out.entry(binding.name.clone())
            .or_insert_with(|| text[binding.ty_span.start..binding.ty_span.end].trim().to_string());
    }
    out
}

pub fn parse_named_type(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let builtins = [
        "int", "nat", "bool", "string", "unit", "bits", "bit", "real", "list", "vector", "atom",
        "implicit", "order", "type",
    ];
    let chars = text.chars().collect::<Vec<_>>();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch.is_ascii_alphabetic() || ch == '_' {
            let mut j = i + 1;
            while j < chars.len()
                && (chars[j].is_ascii_alphanumeric() || chars[j] == '_' || chars[j] == '\'')
            {
                j += 1;
            }
            let name = chars[i..j].iter().collect::<String>();
            if !builtins.contains(&name.to_ascii_lowercase().as_str())
                && !builtins.contains(&lower.as_str())
            {
                return Some(name);
            }
            i = j;
        } else {
            i += 1;
        }
    }
    None
}

pub fn type_definition_locations<'a, F, I>(
    files: I,
    uri_hint: &Url,
    ty_name: &str,
) -> Vec<ide_db::ide_types::FileLocation>
where
    F: FileDb + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut locations = files
        .into_iter()
        .filter_map(|(uri, file)| {
            type_decls(file).get(ty_name).copied().map(|offset| {
                ide_db::span::file_location_from_span(
                    uri,
                    Span::new(offset, offset + ty_name.len()),
                )
            })
        })
        .collect::<Vec<_>>();
    locations.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        })
    });
    locations
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
        let Some(parsed) = file.parsed() else {
            continue;
        };
        for decl in &parsed.decls {
            if decl.name != name
                || decl.role != DeclRole::Definition
                || !matches!(
                    decl.kind,
                    syntax::parser_lower::DeclKind::Function
                        | syntax::parser_lower::DeclKind::Mapping
                        | syntax::parser_lower::DeclKind::Overload
                )
            {
                continue;
            }
            locations.push(ide_db::ide_types::FileLocation {
                url: uri.clone(),
                range: base_db::text_range(decl.span.start, decl.span.end),
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
    let mut declarations = files
        .into_iter()
        .flat_map(|(uri, file)| {
            let Some(parsed) = file.parsed() else {
                return Vec::new().into_iter();
            };
            parsed
                .decls
                .iter()
                .filter(move |decl| {
                    decl.name == symbol_key
                        && decl.scope == Scope::TopLevel
                        && decl.role == DeclRole::Declaration
                })
                .map(move |decl| {
                    // Use identifier span (name_span from callable_heads) if available,
                    // otherwise compute from decl name position within the text.
                    // Goto-declaration targets the identifier, not the full decl.
                    let name_span = parsed
                        .callable_heads
                        .iter()
                        .find(|h| {
                            h.name == decl.name
                                && h.name_span.start >= decl.span.start
                                && h.name_span.end <= decl.span.end
                        })
                        .map(|h| h.name_span)
                        .unwrap_or_else(|| {
                            // Fallback: search for name in decl text
                            let text = file.text();
                            let name_offset = text[decl.span.start..decl.span.end]
                                .find(&decl.name)
                                .map(|o| decl.span.start + o)
                                .unwrap_or(decl.span.start);
                            parser::Span::new(name_offset, name_offset + decl.name.len())
                        });
                    ide_db::span::file_location_from_span(uri, name_span)
                })
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

// ---- dyn-dispatch wrappers ----
//
// The generic functions above require `F: Sized` because the inner
// helpers (`location_from_span`, `range_from_span`) accept
// `&dyn FileDb`, and the `&F -> &dyn FileDb` coercion needs
// `F: Sized`.  These thin wrappers accept `&[(&Url, &dyn FileDb)]`
// directly so callers that only have trait objects (e.g.
// `hir::Semantics`) can use them without going through turbofish.

pub fn symbol_definition_locations_dyn(
    files: &[(&Url, &dyn FileDb)],
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<ide_db::ide_types::FileLocation> {
    let mut definitions: Vec<ide_db::ide_types::FileLocation> = files
        .iter()
        .flat_map(|(uri, file)| {
            symbol_definition_spans(*file, symbol_key)
                .into_iter()
                .map(move |span| ide_db::span::file_location_from_span(uri, span))
        })
        .collect();
    definitions.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    definitions
}

pub fn symbol_declaration_locations_dyn(
    files: &[(&Url, &dyn FileDb)],
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<ide_db::ide_types::FileLocation> {
    let mut declarations: Vec<ide_db::ide_types::FileLocation> = files
        .iter()
        .flat_map(|(uri, file)| {
            let Some(parsed) = file.parsed() else {
                return Vec::new().into_iter();
            };
            parsed
                .decls
                .iter()
                .filter(move |decl| decl.name == symbol_key && decl.role == DeclRole::Declaration)
                .map(move |decl| ide_db::ide_types::FileLocation {
                    url: (*uri).clone(),
                    range: base_db::text_range(decl.span.start, decl.span.end),
                })
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect();
    declarations.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
            _ => 0,
        })
    });
    declarations
}

pub fn implementation_locations_dyn(
    files: &[(&Url, &dyn FileDb)],
    uri_hint: &Url,
    name: &str,
) -> Vec<ide_db::ide_types::FileLocation> {
    let mut locations = Vec::new();
    for (uri, file) in files {
        let Some(parsed) = file.parsed() else {
            continue;
        };
        for decl in &parsed.decls {
            if decl.name != name
                || decl.role != DeclRole::Definition
                || !matches!(
                    decl.kind,
                    syntax::parser_lower::DeclKind::Function
                        | syntax::parser_lower::DeclKind::Mapping
                        | syntax::parser_lower::DeclKind::Overload
                )
            {
                continue;
            }
            locations.push(ide_db::ide_types::FileLocation {
                url: (*uri).clone(),
                range: base_db::text_range(decl.span.start, decl.span.end),
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

pub fn type_definition_locations_dyn(
    files: &[(&Url, &dyn FileDb)],
    uri_hint: &Url,
    ty_name: &str,
) -> Vec<ide_db::ide_types::FileLocation> {
    let mut locations: Vec<ide_db::ide_types::FileLocation> = files
        .iter()
        .filter_map(|(uri, file)| {
            type_decls(*file).get(ty_name).copied().map(|offset| {
                ide_db::span::file_location_from_span(
                    uri,
                    Span::new(offset, offset + ty_name.len()),
                )
            })
        })
        .collect();
    locations.sort_by_key(|location| {
        Reverse(match (uri_hint.path_segments(), location.url.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        })
    });
    locations
}

// The 'symbol_locations_use_identifier_spans' test that used to live
// here was tightly coupled to sail_server::state::File (it used
// File::new directly). Equivalent end-to-end coverage exists in
// sail_server::tests; the inline test was dropped during the
// move rather than reproducing the full File pipeline behind
// a TestFile stub.

//
// These use SymbolIndex for O(1) cross-file lookups
// instead of iterating all files.

use hir_def::item_tree::ItemKind;
use ide_db::workspace_index::SymbolIndex;

/// Lookup definition locations from index. O(1) by name.
pub fn definition_locations_indexed(
    index: &SymbolIndex,
    symbol_key: &str,
    uri_hint: &Url,
) -> Vec<FileLocation> {
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| {
            matches!(
                e.kind,
                ItemKind::Function
                    | ItemKind::Mapping
                    | ItemKind::TypeAlias
                    | ItemKind::Struct
                    | ItemKind::Union
                    | ItemKind::Enum
                    | ItemKind::Bitfield
                    | ItemKind::Newtype
                    | ItemKind::Register
                    | ItemKind::Let
                    | ItemKind::Var
                    | ItemKind::Overload
                    | ItemKind::ScatteredHead
            )
        })
        .filter(|e| !e.is_clause) // Prefer non-clause definitions
        .map(|e| FileLocation {
            url: e.url.clone(),
            range: base_db::text_range(e.span.start, e.span.end),
        })
        .collect();

    // If no non-clause definitions found, include clauses
    if locs.is_empty() {
        locs = entries
            .iter()
            .map(|e| FileLocation {
                url: e.url.clone(),
                range: base_db::text_range(e.span.start, e.span.end),
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
) -> Vec<FileLocation> {
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| matches!(e.kind, ItemKind::ValSpec | ItemKind::MappingSpec))
        .map(|e| FileLocation {
            url: e.url.clone(),
            range: base_db::text_range(e.span.start, e.span.end),
        })
        .collect();

    if locs.is_empty() {
        // Fallback: use definitions
        return definition_locations_indexed(index, symbol_key, uri_hint);
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
) -> Vec<FileLocation> {
    let entries = index.find(symbol_key);
    let mut locs: Vec<FileLocation> = entries
        .iter()
        .filter(|e| matches!(e.kind, ItemKind::Function | ItemKind::Mapping | ItemKind::Overload))
        .map(|e| FileLocation {
            url: e.url.clone(),
            range: base_db::text_range(e.span.start, e.span.end),
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
