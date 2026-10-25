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

/// Signatures selected at a program point. `None` means this is an
/// unconfigured buffer; an empty vector in a configured project is final.
pub fn callable_signatures_at(
    db: &dyn salsa::Database,
    file: base_db::FileText,
    offset: usize,
    name: &str,
) -> Option<Vec<CallableSignature>> {
    use hir::source_order::{
        self,
        bindings::{self, Namespace},
    };
    source_order::declarations_for(db, file)?;
    let sema = hir::Semantics::new(db);
    let local = [offset, offset.saturating_sub(1)].into_iter().find_map(|at| {
        match sema.resolve_path(db, file, at, name) {
            Some(hir::PathResolution::Local(local)) => Some(local),
            _ => None,
        }
    });
    // Incomplete code can bind a local under a global callable's name.
    // It must not display the global signature as that local's signature.
    if local.is_some() {
        return Some(Vec::new());
    }
    let Some(at) = source_order::at_offset(db, file, offset)
        .or_else(|| source_order::at_offset(db, file, offset.saturating_sub(1)))
    else {
        return Some(Vec::new());
    };
    if matches!(source_order::program(db, file), Some(source_order::Program::Inline(_)))
        && !bindings::contains_name(db, at, &hir::Name::new(name))
    {
        return None; // Preserve external workspace lookup for an ad-hoc buffer.
    }
    fn collect(
        db: &dyn salsa::Database,
        at: source_order::Declaration,
        name: &str,
        seen: &mut std::collections::HashSet<String>,
        out: &mut Vec<CallableSignature>,
    ) {
        if !seen.insert(name.to_owned()) {
            return;
        }
        let Some(decl) = bindings::resolve(db, at, hir::Name::new(name), Namespace::Callable)
        else {
            return;
        };
        let Some(node) = source_order::node(db, decl) else { return };
        if let Some(members) = bindings::overload_leaf_members(db, at, hir::Name::new(name)) {
            for member in members {
                collect(db, at, member.as_str(), seen, out);
            }
            return;
        }
        let definition = hir::def_at_declaration_in_namespace(db, decl, name, Namespace::Callable);
        let generated = definition.as_ref().is_some_and(|def| {
            def.location().item(db).is_some_and(|(tree, item)| tree.generated.contains_key(&item))
        });
        if generated || node.kind() == parser::SyntaxKind::NAMED_DEF {
            if let Some(signature) = definition.and_then(|def| def.location().lookup_signature(db))
            {
                let (root, _) = syntax::parse_text(&signature);
                let parsed = syntax::cst_lower::parsed_file_from_cst(&root, &signature);
                out.extend(collect_callable_signatures_from(&parsed, &signature));
            }
            return;
        }
        let Some(parsed) = syntax::parse_query::parsed_file_cst(db, decl.file) else { return };
        let selected = syntax::parser_lower::ParsedFile {
            callable_heads: parsed
                .0
                .callable_heads
                .iter()
                .filter(|head| {
                    head.name == name
                        && node.text_range().contains((head.label_span.start as u32).into())
                })
                .cloned()
                .collect(),
            ..Default::default()
        };
        out.extend(collect_callable_signatures_from(&selected, decl.file.text(db)));
    }
    let mut result = Vec::new();
    collect(db, at, name, &mut Default::default(), &mut result);
    Some(result)
}

/// Parameter bindings of the implementations owned by the callable selected
/// at this use site. Signature display labels are not parameter names.
pub fn callable_parameter_names_at(
    db: &dyn salsa::Database,
    file: base_db::FileText,
    offset: usize,
    name: &str,
) -> Option<Vec<Vec<String>>> {
    use crate::defs::Definition;
    use hir::source_order::{
        self,
        bindings::{self, Namespace},
    };
    source_order::declarations_for(db, file)?;
    let at = source_order::at_offset(db, file, offset)?;
    if matches!(source_order::program(db, file), Some(source_order::Program::Inline(_)))
        && !bindings::contains_name(db, at, &hir::Name::new(name))
    {
        return None;
    }
    let members = bindings::overload_leaf_members(db, at, hir::Name::new(name))
        .unwrap_or_else(|| vec![hir::Name::new(name)]);
    let mut result = Vec::new();
    for member in members {
        let Some(selected) = bindings::resolve(db, at, member.clone(), Namespace::Callable)
            .and_then(|decl| {
                hir::def_at_declaration_in_namespace(db, decl, member.as_str(), Namespace::Callable)
            })
            .and_then(Definition::from_def)
        else {
            continue;
        };
        let canonical = selected.canonical(db);
        for decl in bindings::declarations_named_at(db, at, member.clone(), Namespace::Callable) {
            let Some(def) = hir::def_at_declaration_in_namespace(
                db,
                decl,
                member.as_str(),
                Namespace::Callable,
            )
            .and_then(Definition::from_def) else {
                continue;
            };
            if !matches!(
                def,
                Definition::Function(_) | Definition::Mapping(_) | Definition::ScatteredClause(_)
            ) || def.canonical(db) != canonical
            {
                continue;
            }
            let Some(node) = source_order::node(db, decl) else { continue };
            let Some(parsed) = syntax::parse_query::parsed_file_cst(db, decl.file) else {
                continue;
            };
            for head in &parsed.0.callable_heads {
                if head.name == member.as_str()
                    && node.text_range().contains((head.label_span.start as u32).into())
                {
                    result.push(
                        head.params
                            .iter()
                            .map(|param| param.name.clone().unwrap_or_else(|| "_".into()))
                            .collect(),
                    );
                }
            }
        }
    }
    Some(result)
}

/// Substitute argument-type information into a signature label so
/// generic type variables (`'n`, `'m`) get specialised when the
/// arguments fix them.
pub fn instantiate_signature(sig: &CallableSignature, arg_types: &[Option<String>]) -> String {
    let mut subst = std::collections::HashMap::new();

    for (param, arg_ty) in sig.params.iter().zip(arg_types.iter()) {
        let param_name = &param.name;
        let Some(arg_ty) = arg_ty else { continue };

        if let Some(n_start) = param_name.find('\'') {
            let var_name = &param_name[n_start..];
            let v_end = var_name[1..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|i| i + 1)
                .unwrap_or(var_name.len());
            let var = &var_name[..v_end];

            if !var.is_empty() {
                if arg_ty.starts_with("bits(") {
                    let actual = &arg_ty[5..arg_ty.len() - 1];
                    subst.insert(var.to_string(), actual.to_string());
                } else if arg_ty == "int" || arg_ty == "bool" {
                    subst.insert(var.to_string(), arg_ty.clone());
                }
            }
        }
    }

    let mut label = sig.label.clone();
    for (var, val) in subst {
        label = label.replace(&var, &val);
    }
    label
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
