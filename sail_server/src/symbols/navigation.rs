use super::analysis::{
    extract_symbol_decls, find_callable_signature, location_from_span, range_from_span,
    token_symbol_key,
};
use crate::state::File;
use sail_parser::{DeclRole, Scope, Span};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use tower_lsp::lsp_types::{
    CallHierarchyItem, Location, OneOf, Range, RenameFilesParams, SymbolKind, TextEdit,
    TypeHierarchyItem, Url, WorkspaceLocation, WorkspaceSymbol,
};

#[derive(Clone)]
pub(crate) struct CallEdge {
    pub(crate) caller: String,
    pub(crate) caller_uri: Url,
    pub(crate) callee: String,
    pub(crate) call_range: Range,
}

pub(crate) fn call_edges_for_file(uri: &Url, file: &File) -> Vec<CallEdge> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    parsed
        .call_sites
        .iter()
        .filter_map(|call| {
            let caller = call.caller.clone()?;
            Some(CallEdge {
                caller,
                caller_uri: uri.clone(),
                callee: call.callee.clone(),
                call_range: Range::new(
                    file.source.position_at(call.callee_span.start),
                    file.source.position_at(call.callee_span.end),
                ),
            })
        })
        .collect()
}

pub(crate) fn call_edges<'a, I>(files: I) -> Vec<CallEdge>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut out = Vec::new();
    for (uri, file) in files {
        out.extend(call_edges_for_file(uri, file));
    }
    out
}

/// Collect call edges where the callee matches `target`, skipping unrelated edges.
pub(crate) fn call_edges_to<'a, I>(files: I, target: &str) -> Vec<CallEdge>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut out = Vec::new();
    for (uri, file) in files {
        let Some(parsed) = file.parsed() else { continue };
        for call in &parsed.call_sites {
            if call.callee != target {
                continue;
            }
            if let Some(caller) = call.caller.clone() {
                out.push(CallEdge {
                    caller,
                    caller_uri: uri.clone(),
                    callee: call.callee.clone(),
                    call_range: Range::new(
                        file.source.position_at(call.callee_span.start),
                        file.source.position_at(call.callee_span.end),
                    ),
                });
            }
        }
    }
    out
}

/// Collect call edges where the caller matches `source`, skipping unrelated edges.
pub(crate) fn call_edges_from<'a, I>(files: I, source: &str) -> Vec<CallEdge>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut out = Vec::new();
    for (uri, file) in files {
        let Some(parsed) = file.parsed() else { continue };
        for call in &parsed.call_sites {
            if call.caller.as_deref() != Some(source) {
                continue;
            }
            out.push(CallEdge {
                caller: source.to_string(),
                caller_uri: uri.clone(),
                callee: call.callee.clone(),
                call_range: Range::new(
                    file.source.position_at(call.callee_span.start),
                    file.source.position_at(call.callee_span.end),
                ),
            });
        }
    }
    out
}

pub(crate) fn call_hierarchy_item<'a, I>(
    files: I,
    uri_hint: &Url,
    name: &str,
) -> Option<CallHierarchyItem>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut best: Option<(usize, Url, Range, Option<String>)> = None;
    for (uri, file) in files {
        let detail =
            find_callable_signature(std::iter::once((uri, file)), uri, name).map(|s| s.label);
        for span in symbol_definition_spans(file, name) {
            let range = range_from_span(file, span);
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
    Some(CallHierarchyItem {
        name: name.to_string(),
        kind: SymbolKind::FUNCTION,
        tags: None,
        detail,
        uri,
        range,
        selection_range: range,
        data: Some(serde_json::json!({ "name": name })),
    })
}

fn type_decls(file: &File) -> HashMap<String, usize> {
    let mut out = HashMap::new();
    let Some(parsed) = file.parsed() else {
        return out;
    };
    for decl in &parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel {
            continue;
        }
        if matches!(
            decl.kind,
            sail_parser::DeclKind::Type
                | sail_parser::DeclKind::Struct
                | sail_parser::DeclKind::Union
                | sail_parser::DeclKind::Enum
                | sail_parser::DeclKind::Bitfield
                | sail_parser::DeclKind::Newtype
        ) {
            out.insert(decl.name.clone(), decl.span.start);
        }
    }
    out
}

fn symbol_definition_spans(file: &File, symbol_key: &str) -> Vec<Span> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    let mut spans = parsed
        .decls
        .iter()
        .filter(|decl| {
            decl.name == symbol_key
                && decl.role == DeclRole::Definition
                && match decl.kind {
                    sail_parser::DeclKind::Let
                    | sail_parser::DeclKind::Var
                    | sail_parser::DeclKind::Parameter => decl.scope == Scope::TopLevel,
                    _ => true,
                }
        })
        .map(|decl| decl.span)
        .collect::<Vec<_>>();
    spans.sort_unstable_by_key(|span| (span.start, span.end));
    spans.dedup();
    spans
}

fn type_decls_with_kind(file: &File) -> HashMap<String, (usize, SymbolKind)> {
    let mut out = HashMap::new();
    let Some(parsed) = file.parsed() else {
        return out;
    };
    for decl in &parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel {
            continue;
        }
        let Some(kind) = (match decl.kind {
            sail_parser::DeclKind::Type
            | sail_parser::DeclKind::Struct
            | sail_parser::DeclKind::Union
            | sail_parser::DeclKind::Bitfield
            | sail_parser::DeclKind::Newtype => Some(SymbolKind::STRUCT),
            sail_parser::DeclKind::Enum => Some(SymbolKind::ENUM),
            _ => None,
        }) else {
            continue;
        };
        out.insert(decl.name.clone(), (decl.span.start, kind));
    }
    out
}

pub(crate) fn type_alias_edges(file: &File) -> Vec<(String, String)> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    parsed
        .type_aliases
        .iter()
        .map(|a| (a.sub.clone(), a.sup.clone()))
        .collect()
}

pub(crate) fn type_hierarchy_item<'a, I>(
    files: I,
    uri_hint: &Url,
    name: &str,
) -> Option<TypeHierarchyItem>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut best: Option<(usize, Url, Range, SymbolKind)> = None;
    for (uri, file) in files {
        let Some((offset, kind)) = type_decls_with_kind(file).get(name).copied() else {
            continue;
        };
        let range = Range::new(
            file.source.position_at(offset),
            file.source.position_at(offset + name.len()),
        );
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
        tags: None,
        detail: Some("type".to_string()),
        uri: uri.clone(),
        range: range.clone(),
        selection_range: range,
        data: Some(serde_json::json!({ "name": name })),
    })
}

pub(crate) fn type_supertypes<'a, I>(files: I, uri_hint: &Url, name: &str) -> Vec<TypeHierarchyItem>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    let names: HashSet<String> = files
        .iter()
        .flat_map(|(_, file)| type_alias_edges(file))
        .filter_map(|(sub, sup)| if sub == name { Some(sup) } else { None })
        .collect();

    names
        .into_iter()
        .filter_map(|super_name| type_hierarchy_item(files.iter().copied(), uri_hint, &super_name))
        .collect()
}

pub(crate) fn type_subtypes<'a, I>(files: I, uri_hint: &Url, name: &str) -> Vec<TypeHierarchyItem>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    let names: HashSet<String> = files
        .iter()
        .flat_map(|(_, file)| type_alias_edges(file))
        .filter_map(|(sub, sup)| if sup == name { Some(sub) } else { None })
        .collect();

    names
        .into_iter()
        .filter_map(|sub_name| type_hierarchy_item(files.iter().copied(), uri_hint, &sub_name))
        .collect()
}

pub(crate) fn type_name_candidates_at_position(
    file: &File,
    position: tower_lsp::lsp_types::Position,
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

pub(crate) fn typed_bindings(file: &File) -> HashMap<String, String> {
    let mut out = HashMap::new();
    if let (Some(parsed), Some(type_check)) = (file.parsed(), file.type_check()) {
        for decl in &parsed.decls {
            if !matches!(
                decl.kind,
                sail_parser::DeclKind::Parameter
                    | sail_parser::DeclKind::Let
                    | sail_parser::DeclKind::Var
            ) {
                continue;
            }

            if let Some(ty) = type_check.binding_type_text(decl.span) {
                out.insert(decl.name.clone(), ty.to_string());
            }
        }
    }

    let Some(parsed) = file.parsed() else {
        return out;
    };
    let text = file.source.text();
    for binding in &parsed.typed_bindings {
        out.entry(binding.name.clone()).or_insert_with(|| {
            text[binding.ty_span.start..binding.ty_span.end]
                .trim()
                .to_string()
        });
    }
    out
}

pub(crate) fn parse_named_type(text: &str) -> Option<String> {
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

pub(crate) fn type_definition_locations<'a, I>(
    files: I,
    uri_hint: &Url,
    ty_name: &str,
) -> Vec<Location>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut locations = files
        .into_iter()
        .filter_map(|(uri, file)| {
            type_decls(file).get(ty_name).copied().map(|offset| {
                location_from_span(uri, file, Span::new(offset, offset + ty_name.len()))
            })
        })
        .collect::<Vec<_>>();
    locations.sort_by_key(|location| {
        Reverse(
            match (uri_hint.path_segments(), location.uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            },
        )
    });
    locations
}

pub(crate) fn implementation_locations<'a, I>(files: I, uri_hint: &Url, name: &str) -> Vec<Location>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
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
                    sail_parser::DeclKind::Function
                        | sail_parser::DeclKind::Mapping
                        | sail_parser::DeclKind::Overload
                )
            {
                continue;
            }
            locations.push(Location::new(
                uri.clone(),
                Range::new(
                    file.source.position_at(decl.span.start),
                    file.source.position_at(decl.span.end),
                ),
            ));
        }
    }

    locations.sort_by_key(|location| {
        Reverse(
            match (uri_hint.path_segments(), location.uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            },
        )
    });
    locations
}

pub(crate) fn symbol_definition_locations<'a, I>(
    files: I,
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<Location>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut definitions = files
        .into_iter()
        .flat_map(|(uri, file)| {
            symbol_definition_spans(file, symbol_key)
                .into_iter()
                .map(move |span| location_from_span(uri, file, span))
        })
        .collect::<Vec<_>>();

    definitions.sort_by_key(|location| {
        Reverse(
            match (uri_hint.path_segments(), location.uri.path_segments()) {
                (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
                _ => 0,
            },
        )
    });
    definitions
}

pub(crate) fn symbol_declaration_locations<'a, I>(
    files: I,
    uri_hint: &Url,
    symbol_key: &str,
) -> Vec<Location>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
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
                .map(move |decl| location_from_span(uri, file, decl.span))
                .collect::<Vec<_>>()
                .into_iter()
        })
        .collect::<Vec<_>>();

    declarations.sort_by_key(|location| {
        Reverse(
            match (uri_hint.path_segments(), location.uri.path_segments()) {
                (Some(p0), Some(p1)) => p0.zip(p1).take_while(|(a, b)| a == b).count(),
                _ => 0,
            },
        )
    });
    declarations
}

pub(crate) fn resolve_workspace_symbol<'a, I>(
    mut symbol: WorkspaceSymbol,
    files: I,
) -> WorkspaceSymbol
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    if matches!(symbol.location, OneOf::Left(_)) {
        return symbol;
    }

    let target_uri = match &symbol.location {
        OneOf::Right(WorkspaceLocation { uri }) => uri,
        OneOf::Left(_) => return symbol,
    };

    for (uri, file) in files {
        if uri != target_uri {
            continue;
        }
        if let Some(decl) = extract_symbol_decls(file)
            .into_iter()
            .find(|decl| decl.name == symbol.name && decl.kind == symbol.kind)
        {
            let range = Range::new(
                file.source.position_at(decl.offset),
                file.source.position_at(decl.offset + decl.name.len()),
            );
            symbol.location = OneOf::Left(Location::new(uri.clone(), range));
            return symbol;
        }
        if let Some(span) = symbol_definition_spans(file, &symbol.name).first().copied() {
            symbol.location = OneOf::Left(Location::new(uri.clone(), range_from_span(file, span)));
            return symbol;
        }
    }
    symbol
}

fn basename_from_uri(uri: &str) -> Option<String> {
    Url::parse(uri).ok().and_then(|url| {
        url.path_segments()
            .and_then(|mut segments| segments.next_back().map(str::to_string))
    })
}

pub(crate) fn will_rename_file_edits<'a, I>(
    files: I,
    rename_params: &RenameFilesParams,
) -> Option<HashMap<Url, Vec<TextEdit>>>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let rename_pairs = rename_params
        .files
        .iter()
        .filter_map(|rename| {
            let old_name = basename_from_uri(&rename.old_uri)?;
            let new_name = basename_from_uri(&rename.new_uri)?;
            if old_name == new_name {
                return None;
            }
            Some((format!("\"{old_name}\""), format!("\"{new_name}\"")))
        })
        .collect::<Vec<_>>();

    if rename_pairs.is_empty() {
        return None;
    }

    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for (uri, file) in files {
        let text = file.source.text();
        let mut edits = Vec::new();
        for (old_text, new_text) in &rename_pairs {
            for (start, _) in text.match_indices(old_text) {
                let end = start + old_text.len();
                edits.push(TextEdit {
                    range: Range::new(file.source.position_at(start), file.source.position_at(end)),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbol_locations_use_identifier_spans() {
        let source = "val foo : int\nfunction foo() = 0\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();

        let declarations =
            symbol_declaration_locations(std::iter::once((&uri, &file)), &uri, "foo");
        let definitions = symbol_definition_locations(std::iter::once((&uri, &file)), &uri, "foo");

        assert_eq!(declarations.len(), 1);
        assert_eq!(definitions.len(), 1);

        let decl_start = source.find("foo :").unwrap();
        let def_start = source.rfind("foo()").unwrap();
        assert_eq!(
            declarations[0].range,
            Range::new(
                file.source.position_at(decl_start),
                file.source.position_at(decl_start + 3),
            )
        );
        assert_eq!(
            definitions[0].range,
            Range::new(
                file.source.position_at(def_start),
                file.source.position_at(def_start + 3),
            )
        );
    }
}
