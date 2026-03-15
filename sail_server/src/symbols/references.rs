use std::collections::HashMap;

use super::analysis::location_from_span;
use crate::state::File;
use sail_parser::{Scope, Span, SymbolOccurrenceKind};
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{Location, Position, Range, TextEdit, Url};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedSymbol {
    pub(crate) name: String,
    pub(crate) kind: SymbolOccurrenceKind,
    pub(crate) scope: Option<Scope>,
    pub(crate) target_span: Option<Span>,
}

fn sort_and_dedup_spans(spans: &mut Vec<(Span, bool)>) {
    spans.sort_unstable_by_key(|(span, is_write)| (span.start, span.end, !*is_write));
    spans.dedup();
}

fn matches_symbol(
    occurrence: &sail_parser::SymbolOccurrence,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> bool {
    if occurrence.kind != symbol.kind {
        return false;
    }
    if !include_declarations && occurrence.role.is_some() {
        return false;
    }

    if let Some(target_span) = symbol.target_span {
        return occurrence.target_span == Some(target_span);
    }

    if symbol.scope == Some(Scope::TopLevel) {
        return occurrence.scope == Some(Scope::TopLevel) && occurrence.name == symbol.name;
    }

    occurrence.name == symbol.name
}

pub(crate) fn resolve_symbol_at(file: &File, position: Position) -> Option<ResolvedSymbol> {
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

pub(crate) fn symbol_spans_for_file(
    file: &File,
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

pub(crate) fn reference_locations<'a, I>(
    files: I,
    current_uri: &Url,
    symbol: &ResolvedSymbol,
    include_declarations: bool,
) -> Vec<Location>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let local_only = symbol.target_span.is_some() || symbol.kind == SymbolOccurrenceKind::TypeVar;
    let mut locations = Vec::new();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in symbol_spans_for_file(file, symbol, include_declarations) {
            locations.push(location_from_span(uri, file, span));
        }
    }

    locations.sort_by_key(|location| {
        std::cmp::Reverse(
            match (current_uri.path_segments(), location.uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(lhs, rhs)| lhs == rhs).count(),
                _ => 0,
            },
        )
    });
    locations
}

pub(crate) fn rename_edits<'a, I>(
    files: I,
    current_uri: &Url,
    symbol: &ResolvedSymbol,
    new_text: &str,
) -> HashMap<Url, Vec<TextEdit>>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let local_only = symbol.target_span.is_some() || symbol.kind == SymbolOccurrenceKind::TypeVar;
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();

    for (uri, file) in files {
        if local_only && uri != current_uri {
            continue;
        }
        for (span, _) in symbol_spans_for_file(file, symbol, true) {
            changes.entry(uri.clone()).or_default().push(TextEdit {
                range: Range::new(
                    file.source.position_at(span.start),
                    file.source.position_at(span.end),
                ),
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

pub(crate) fn normalize_validated_rename(
    token: &sail_parser::Token,
    requested_name: &str,
    keywords: &[&str],
) -> Result<Option<String>> {
    let (base_name, quoted) = if let Some(stripped) = requested_name.strip_prefix('\'') {
        (stripped, true)
    } else {
        (requested_name, false)
    };
    if !is_valid_identifier_name(base_name) {
        return Err(tower_lsp::jsonrpc::Error::invalid_params(
            "new_name must be a valid identifier",
        ));
    }
    if keywords.iter().any(|kw| *kw == base_name) {
        return Err(tower_lsp::jsonrpc::Error::invalid_params(
            "new_name cannot be a Sail keyword",
        ));
    }

    match token {
        sail_parser::Token::TyVal(_) => {
            if quoted {
                Ok(Some(requested_name.to_string()))
            } else {
                Ok(Some(format!("'{base_name}")))
            }
        }
        _ if quoted => Err(tower_lsp::jsonrpc::Error::invalid_params(
            "type variable marker (') is only valid when renaming type variables",
        )),
        _ => Ok(Some(base_name.to_string())),
    }
}
