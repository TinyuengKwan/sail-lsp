//! What a callable's signature says, collected from the files that declare it.
//!
//! Built on [`FileDb`] so the feature crates — calls, lenses, completion —
//! can ask without depending on the server.
//!
//! This was `symbol_index`, which is rust-analyzer's name for the *workspace
//! symbol index* — a different thing, living next door in
//! `workspace_index`. Two modules for one of RA's, with the matching name on
//! the one that does not do RA's job, and the RA shape report explained the
//! low score with "same role, built on salsa here", which was simply wrong.
//! The index is `symbol_index` now and this is what it holds.

use crate::ide_types::SymbolKind;
use crate::{CallableSignature, FileDb};
use std::collections::HashMap;
use url::Url;

/// Where each name in `source` is defined, by byte offset.
///
/// Parses and lowers `source` on its own, for callers that hold text
/// rather than a database.
pub fn add_definitions(source: &str, definitions: &mut HashMap<String, usize>) {
    let (cst_root, _) = syntax::parse_text(source);
    let tree = hir::ItemTree::build_from_cst(&cst_root);
    for &id in tree.top_level_items() {
        let name = id.name(&tree);
        if name.as_str().is_empty() {
            continue;
        }
        let start = id.span(&tree).start;
        definitions.entry(name.to_string()).or_insert(start);
    }
}

/// Walk a file's `parsed().callable_heads` and produce one
/// [`CallableSignature`] per function / val / mapping definition.
/// Stage — extracted from `sail_server::symbols::analysis`.
pub fn collect_callable_signatures(file: &dyn FileDb) -> Vec<CallableSignature> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    collect_callable_signatures_from(parsed, file.text())
}

pub use hir::callable_info::{build_signature_index_from_parsed, collect_callable_signatures_from};

/// Read cached semantic metadata, with a syntax-only fallback for text fixtures.
pub fn signature_index<F: FileDb + ?Sized>(
    file: &F,
) -> std::sync::Arc<HashMap<String, CallableSignature>> {
    if let Some((db, ft)) = file.file_text() {
        return hir::Semantics::new(db).signature_index(ft);
    }
    std::sync::Arc::new(
        file.parsed()
            .map(|parsed| build_signature_index_from_parsed(parsed, file.text()))
            .unwrap_or_default(),
    )
}

/// Find ALL callable signatures for `name` across `files`.
/// Unlike `find_callable_signature` (which returns only the best),
/// this returns every matching signature for overload display in hover.
pub fn find_all_callable_signatures<'a, F, I>(files: I, name: &str) -> Vec<CallableSignature>
where
    F: FileDb + ?Sized + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut results = Vec::new();
    let mut seen_labels = std::collections::HashSet::new();
    for (_uri, file) in files {
        let sig_index = signature_index(file);
        if let Some(sig) = sig_index.get(name) {
            if seen_labels.insert(sig.label.clone()) {
                results.push(sig.clone());
            }
        }
    }
    results
}

/// Lightweight symbol-declaration view used to drive document outlines
/// and code lenses. Walks `parsed().decls` and filters local-scope
/// entries unless they're enum members.
#[derive(Clone, Debug)]
pub struct SymbolDecl {
    pub name: String,
    pub kind: SymbolKind,
    pub detail: &'static str,
    pub offset: usize,
}

/// Walk the per-file `ParsedFile::decls` and return one [`SymbolDecl`]
/// per top-level declaration. Local-scope declarations are dropped
/// (except for enum members, which are emitted so the document
/// symbol tree can parent them under the enum).
pub fn extract_symbol_decls(file: &dyn FileDb) -> Vec<SymbolDecl> {
    // From the item tree alone. The scrape path that used to run first
    // filtered on a `Scope` the tree does not need — every top-level item is
    // top level by construction — and dropped nothing else the tree has.
    if let Some(item_tree) = file.item_tree() {
        return item_tree
            .top_level_items()
            .iter()
            .filter(|id| !id.is_clause(item_tree)) // skip clauses for workspace symbol
            .filter_map(|id| {
                let (kind, detail) = match id.def_kind(item_tree) {
                    hir::DefKind::Function => (SymbolKind::Function, "function"),
                    hir::DefKind::ValSpec => (SymbolKind::Function, "value"),
                    hir::DefKind::Mapping | hir::DefKind::MappingSpec => {
                        (SymbolKind::Function, "mapping")
                    }
                    hir::DefKind::Overload => (SymbolKind::Function, "overload"),
                    hir::DefKind::TypeAlias
                    | hir::DefKind::Struct
                    | hir::DefKind::Union
                    | hir::DefKind::Bitfield
                    | hir::DefKind::Newtype => (SymbolKind::Struct, "type"),
                    hir::DefKind::Enum => (SymbolKind::Enum, "enum"),
                    hir::DefKind::Register | hir::DefKind::Let | hir::DefKind::Var => {
                        (SymbolKind::Variable, "register")
                    }
                    hir::DefKind::ScatteredHead => (SymbolKind::Function, "scattered"),
                    _ => return None,
                };
                Some(SymbolDecl {
                    name: id.name(item_tree).as_str().to_string(),
                    kind,
                    detail,
                    offset: id.span(item_tree).start,
                })
            })
            // An `enum`'s members follow it, so the document-symbol tree can
            // parent them under it — the same order the scrape produced.
            .flat_map(|decl| {
                let members = item_tree
                    .top_level_items()
                    .iter()
                    .find(|id| {
                        id.name(item_tree).as_str() == decl.name
                            && matches!(id.def_kind(item_tree), hir::DefKind::Enum)
                    })
                    .map(|id| {
                        id.fields(item_tree)
                            .iter()
                            .map(|f| SymbolDecl {
                                name: f.name.to_string(),
                                kind: SymbolKind::EnumMember,
                                detail: "enum member",
                                offset: f.name_span.start,
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                std::iter::once(decl).chain(members)
            })
            .collect();
    }

    Vec::new()
}

/// Find the best [`CallableSignature`] for `name` across `files`.
/// Scoring favours signatures from files whose URL path shares more
/// segments with `uri` (locality), then prefers `val` specs over
/// bare function definitions, then signatures that look richer
/// (`->`, `forall`).
pub fn find_callable_signature<'a, F, I>(
    files: I,
    uri: &Url,
    name: &str,
) -> Option<CallableSignature>
where
    F: FileDb + ?Sized + 'a,
    I: IntoIterator<Item = (&'a Url, &'a F)>,
{
    let mut best: Option<(usize, CallableSignature)> = None;
    for (candidate_uri, candidate_file) in files {
        let sig_index = signature_index(candidate_file);
        let Some(sig) = sig_index.get(name) else {
            continue;
        };
        let mut score = match (uri.path_segments(), candidate_uri.path_segments()) {
            (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
            _ => 0,
        } * 10;
        if sig.label.starts_with("val") {
            score += 5;
        }
        if sig.label.contains("->") {
            score += 2;
        }
        if sig.label.contains("forall") {
            score += 1;
        }
        match &best {
            Some((best_score, _)) if *best_score > score => {}
            _ => best = Some((score, sig.clone())),
        }
    }
    best.map(|(_, sig)| sig)
}
