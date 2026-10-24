//! Go to type definition — navigates to the type of a binding.

use crate::navigation::type_decls;
use ide_db::FileDb;
use parser::Span;
use std::cmp::Reverse;
use std::collections::HashMap;
use url::Url;

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

pub(crate) fn goto_type_definition<'a, F, I>(
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

/// Every local binding in this file, with the type inference gave it.
///
/// From the bodies, which is where a binding *is*: a `Pat::Bind` with a
/// recorded span, one arena per callable. The scrape walked `decls` for
/// three `DeclKind`s and then fell back to a second list, `typed_bindings`,
/// whose type came from slicing the source text between two offsets.
pub fn typed_bindings(file: &dyn FileDb) -> HashMap<String, String> {
    let mut out = HashMap::new();
    // A top-level `let x : T = ..` lowers only its initialiser — the binding
    // itself is not a `Pat` anywhere — so its declared type comes from the
    // item tree, which recorded the whole signature.
    if let Some(tree) = file.item_tree() {
        for &id in tree.top_level_items() {
            if !matches!(id.def_kind(tree), hir::DefKind::Let | hir::DefKind::Var) {
                continue;
            }
            let sig = id.signature(tree);
            let Some((_, ty)) = sig.split_once(':') else { continue };
            let ty = ty.split('=').next().unwrap_or(ty).trim();
            if !ty.is_empty() {
                out.entry(id.name(tree).to_string()).or_insert_with(|| ty.to_string());
            }
        }
    }
    let Some(bodies) = file.bodies() else {
        return out;
    };
    for entry in bodies.entries() {
        // The written annotation first: `Pat::Typed` carries it verbatim, so
        // `let x : my_type = ..` reads back as `my_type` without inference
        // having to agree.
        for (_, pat) in entry.body.iter_pats() {
            let hir::Pat::Typed { inner, ty } = pat else { continue };
            let Some(hir::Pat::Bind(name)) = entry.body.pat(*inner) else { continue };
            if !ty.trim().is_empty() {
                out.entry(name.to_string()).or_insert_with(|| ty.trim().to_string());
            }
        }
        // Then whatever inference worked out, for the bindings with no
        // annotation of their own.
        for (pat_id, pat) in entry.body.iter_pats() {
            let hir::Pat::Bind(name) = pat else { continue };
            let Some(span) = entry.source_map.pat_syntax(pat_id) else { continue };
            if let Some(ty) = file.binding_type_text(span) {
                out.entry(name.to_string()).or_insert(ty);
            }
        }
    }
    out
}
