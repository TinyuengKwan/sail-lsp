//! Workspace-wide symbol lookup helpers built on top of [`FileDb`].
//!
//! Stage — extracted from `sail_server::symbols::analysis` so
//! feature crates (calls / lenses / completion) can call them through
//! `&dyn FileDb` without depending on `sail_server`.

use crate::helpers::Parameter;
use crate::ide_types::SymbolKind;
use crate::{CallableSignature, FileDb};
use parser::Span;
use std::collections::HashMap;
use url::Url;

/// Walk a parsed file and seed the per-file symbol-definitions map
/// (used by the rename / find-references plumbing in `state::file`).
/// Stage — extracted from `sail_server::symbols::analysis`.
pub fn add_parsed_definitions(
    parsed: &syntax::parser_lower::ParsedFile,
    definitions: &mut HashMap<String, usize>,
) {
    use syntax::parser_lower::{DeclKind, DeclRole, Scope};
    for decl in &parsed.decls {
        if decl.role != DeclRole::Definition {
            continue;
        }
        let include = match decl.kind {
            DeclKind::Let | DeclKind::Var | DeclKind::Parameter => decl.scope == Scope::TopLevel,
            _ => true,
        };
        if !include {
            continue;
        }

        definitions.entry(decl.name.clone()).or_insert(decl.span.start);
        if decl.kind == DeclKind::Bitfield {
            definitions.entry(format!("Mk_{}", decl.name)).or_insert(decl.span.start);
        }
    }
}

/// Lex+parse+lower a token stream and feed the resulting
/// `ParsedFile` to `add_parsed_definitions`. Originally a
/// `#[cfg(test)]` helper inside `sail_server::symbols::analysis`;
/// promoted to a regular pub fn during the move so that
/// downstream consumers (sail_server's integration tests) can call
/// it without ide-db opting into a test gate.
pub fn add_definitions(source: &str, definitions: &mut HashMap<String, usize>) {
    let (cst_root, _) = syntax::parse_text(source);
    let parsed = syntax::cst_lower::parsed_file_from_cst(&cst_root, source);
    add_parsed_definitions(&parsed, definitions);
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

/// Pure version of `collect_callable_signatures` that takes parsed
/// data directly, without requiring `FileDb`. Used by hir-ty for
/// workspace context building (avoids hir-ty → ide-db dependency).
pub fn collect_callable_signatures_from(
    parsed: &syntax::parser_lower::ParsedFile,
    text: &str,
) -> Vec<CallableSignature> {
    use syntax::parser_lower::DeclKind;
    let mut out = Vec::new();
    for head in &parsed.callable_heads {
        if !matches!(head.kind, DeclKind::Function | DeclKind::Value | DeclKind::Mapping) {
            continue;
        }

        let label = span_text(text, head.label_span).to_string();
        let params = head
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let ty_text = param.ty_span.map(|span| span_text(text, span).to_string());
                let name = match (param.name.as_deref(), ty_text.as_deref()) {
                    (Some(name), Some(ty)) => format!("{name} : {ty}"),
                    (Some(name), None) => name.to_string(),
                    (None, Some(ty)) => format!("arg{}: {}", idx + 1, ty),
                    (None, None) => format!("arg{}", idx + 1),
                };
                Parameter { name, is_implicit: span_text(text, param.span).contains("implicit") }
            })
            .collect::<Vec<_>>();
        let return_type = head.return_type_span.map(|span| span_text(text, span).to_string());
        out.push(CallableSignature { name: head.name.clone(), label, params, return_type });
    }
    out
}

/// Build a name -> signature index from a ParsedFile + source text.
///
/// Salsa-friendly variant that doesn't need `&dyn FileDb`. Used by
/// `db_query::signature_index`.
pub fn build_signature_index_from_parsed(
    parsed: &syntax::parser_lower::ParsedFile,
    text: &str,
) -> HashMap<String, CallableSignature> {
    use syntax::parser_lower::DeclKind;
    let mut sigs = Vec::new();
    for head in &parsed.callable_heads {
        if !matches!(head.kind, DeclKind::Function | DeclKind::Value | DeclKind::Mapping) {
            continue;
        }
        let label = span_text(text, head.label_span).to_string();
        let params = head
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let ty_text = param.ty_span.map(|span| span_text(text, span).to_string());
                let name = match (param.name.as_deref(), ty_text.as_deref()) {
                    (Some(name), Some(ty)) => format!("{name} : {ty}"),
                    (Some(name), None) => name.to_string(),
                    (None, Some(ty)) => format!("arg{}: {}", idx + 1, ty),
                    (None, None) => format!("arg{}", idx + 1),
                };
                Parameter { name, is_implicit: span_text(text, param.span).contains("implicit") }
            })
            .collect::<Vec<_>>();
        let return_type = head.return_type_span.map(|span| span_text(text, span).to_string());
        sigs.push(CallableSignature { name: head.name.clone(), label, params, return_type });
    }
    let mut index = HashMap::with_capacity(sigs.len());
    for sig in sigs {
        let dominated = index.get(&sig.name).is_some_and(|existing: &CallableSignature| {
            existing.label.starts_with("val") && !sig.label.starts_with("val")
        });
        if !dominated {
            index.insert(sig.name.clone(), sig);
        }
    }
    index
}

/// Build a name -> signature index for a single file.
/// Prefers `val` specs over bare function definitions when the same
/// name appears in both forms.
pub fn build_signature_index(file: &dyn FileDb) -> HashMap<String, CallableSignature> {
    let sigs = collect_callable_signatures(file);
    let mut index = HashMap::with_capacity(sigs.len());
    for sig in sigs {
        let dominated = index.get(&sig.name).is_some_and(|existing: &CallableSignature| {
            existing.label.starts_with("val") && !sig.label.starts_with("val")
        });
        if !dominated {
            index.insert(sig.name.clone(), sig);
        }
    }
    index
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
        let Some(sig_index) = file.signature_index() else {
            continue;
        };
        if let Some(sig) = sig_index.get(name) {
            if seen_labels.insert(sig.label.clone()) {
                results.push(sig.clone());
            }
        }
    }
    results
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

fn span_text(text: &str, span: Span) -> &str {
    text[span.start..span.end].trim()
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
    use syntax::parser_lower::{DeclKind, Scope};

    if let Some(parsed) = file.parsed() {
        let decls: Vec<SymbolDecl> = parsed
            .decls
            .iter()
            .filter_map(|decl| {
                if decl.scope == Scope::Local && !matches!(decl.kind, DeclKind::EnumMember) {
                    return None;
                }
                let (kind, detail) = match decl.kind {
                    DeclKind::Function => (SymbolKind::Function, "function"),
                    DeclKind::Value => (SymbolKind::Function, "value"),
                    DeclKind::Mapping => (SymbolKind::Function, "mapping"),
                    DeclKind::Overload => (SymbolKind::Function, "overload"),
                    DeclKind::Register => (SymbolKind::Variable, "register"),
                    DeclKind::Parameter => (SymbolKind::Variable, "parameter"),
                    DeclKind::Type
                    | DeclKind::Struct
                    | DeclKind::Union
                    | DeclKind::Bitfield
                    | DeclKind::Newtype => (SymbolKind::Struct, "type"),
                    DeclKind::Enum => (SymbolKind::Enum, "enum"),
                    DeclKind::EnumMember => (SymbolKind::EnumMember, "enum member"),
                    DeclKind::Let | DeclKind::Var => (SymbolKind::Variable, "binding"),
                };
                Some(SymbolDecl { name: decl.name.clone(), kind, detail, offset: decl.span.start })
            })
            .collect();
        if !decls.is_empty() {
            return decls;
        }
    }

    // Fallback: use ItemTree when parsed() is not available (e.g., SalsaFile path)
    if let Some(item_tree) = file.item_tree() {
        return item_tree
            .top_level_items()
            .iter()
            .filter(|id| !id.is_clause(item_tree)) // skip clauses for workspace symbol
            .filter_map(|id| {
                let (kind, detail) = match id.item_kind(item_tree) {
                    hir_def::ItemKind::Function => (SymbolKind::Function, "function"),
                    hir_def::ItemKind::ValSpec => (SymbolKind::Function, "value"),
                    hir_def::ItemKind::Mapping | hir_def::ItemKind::MappingSpec => {
                        (SymbolKind::Function, "mapping")
                    }
                    hir_def::ItemKind::Overload => (SymbolKind::Function, "overload"),
                    hir_def::ItemKind::TypeAlias
                    | hir_def::ItemKind::Struct
                    | hir_def::ItemKind::Union
                    | hir_def::ItemKind::Bitfield
                    | hir_def::ItemKind::Newtype => (SymbolKind::Struct, "type"),
                    hir_def::ItemKind::Enum => (SymbolKind::Enum, "enum"),
                    hir_def::ItemKind::Register
                    | hir_def::ItemKind::Let
                    | hir_def::ItemKind::Var => (SymbolKind::Variable, "register"),
                    hir_def::ItemKind::ScatteredHead => (SymbolKind::Function, "scattered"),
                    _ => return None,
                };
                Some(SymbolDecl {
                    name: id.name(item_tree).as_str().to_string(),
                    kind,
                    detail,
                    offset: id.span(item_tree).start,
                })
            })
            .collect();
    }

    Vec::new()
}

/// Build a hierarchical [`DocumentSymbol`] tree for `file`. Enum
/// members become children of their parent enum; all other
/// top-level decls are roots. When the per-file `ItemTree` is
/// available, each emitted symbol carries the precise signature
/// text in its `detail` field instead of the bare category label.
///
/// Stage — extracted from `sail_server::symbols::analysis`.
/// Document symbols — returns internal NavigationTarget (framework-independent).
pub fn document_symbols_ide(file: &dyn FileDb) -> Vec<crate::ide_types::NavigationTarget> {
    use crate::ide_types::{NavigationTarget, SymbolKind as IdeSymbolKind};

    use syntax::parser_lower::{DeclKind, Scope};

    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    let item_spans: Vec<(usize, usize)> = if let Some(it) = file.item_tree() {
        it.top_level_items()
            .iter()
            .map(|id| {
                let s = id.span(it);
                (s.start, s.end)
            })
            .collect()
    } else {
        Vec::new()
    };
    let item_tree_ref = file.item_tree();
    let item_tree_index: HashMap<&str, hir_def::item_tree::ModItem> =
        if let Some(it) = item_tree_ref {
            it.top_level_items().iter().map(|&id| (id.name(it).as_str(), id)).collect()
        } else {
            HashMap::new()
        };

    let mut roots: Vec<NavigationTarget> = Vec::new();
    let mut enum_indices: HashMap<String, usize> = HashMap::new();

    for decl in &parsed.decls {
        if decl.scope == Scope::Local && !matches!(decl.kind, DeclKind::EnumMember) {
            continue;
        }
        let (kind, fallback_detail) = match decl.kind {
            DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload => {
                (IdeSymbolKind::Function, "function")
            }
            DeclKind::Register | DeclKind::Parameter | DeclKind::Let | DeclKind::Var => {
                (IdeSymbolKind::Variable, "binding")
            }
            DeclKind::Type
            | DeclKind::Struct
            | DeclKind::Union
            | DeclKind::Bitfield
            | DeclKind::Newtype => (IdeSymbolKind::Struct, "type"),
            DeclKind::Enum => (IdeSymbolKind::Enum, "enum"),
            DeclKind::EnumMember => (IdeSymbolKind::EnumMember, "enum member"),
        };
        let detail = item_tree_index
            .get(decl.name.as_str())
            .map(|id| {
                let it = item_tree_ref.as_ref().unwrap();
                id.signature(it).to_string()
            })
            .unwrap_or(fallback_detail.to_string());
        let focus_range = base_db::text_range(decl.span.start, decl.span.start + decl.name.len());
        let full_range = item_spans
            .iter()
            .find(|(s, e)| *s <= decl.span.start && decl.span.end <= *e)
            .map(|(s, e)| base_db::text_range(*s, *e))
            .unwrap_or(focus_range);

        let target = NavigationTarget {
            file_id: None,
            url: url::Url::parse("file:///").unwrap(), // placeholder, not meaningful for single-file
            name: decl.name.clone(),
            kind,
            full_range,
            focus_range,
            detail: Some(detail),
            docs: None,
            children: Vec::new(),
        };

        if decl.kind == DeclKind::EnumMember {
            // Find the parent enum whose span contains this member
            let parent_idx = enum_indices
                .iter()
                .filter(|(_, &idx)| {
                    let parent = &roots[idx];
                    base_db::range_start(parent.full_range) <= decl.span.start
                        && decl.span.end <= base_db::range_end(parent.full_range)
                })
                .max_by_key(|(_, &idx)| roots[idx].full_range.start())
                .map(|(_, &idx)| idx)
                // Fallback: use the most recently declared enum
                .or_else(|| enum_indices.values().max().copied());

            if let Some(idx) = parent_idx {
                if let Some(parent) = roots.get_mut(idx) {
                    parent.children.push(target);
                    continue;
                }
            }
        }
        if decl.kind == DeclKind::Enum || decl.kind == DeclKind::Union {
            enum_indices.insert(decl.name.clone(), roots.len());
        }
        roots.push(target);
    }

    // Nest bitfield synthetic accessors under their parent bitfield.
    // ItemTree generates _get_/update_/set_ entries for each bitfield field.
    // Group them as children of the bitfield entry in the outline.
    let mut bitfield_indices: HashMap<String, usize> = HashMap::new();
    for (idx, target) in roots.iter().enumerate() {
        if target.kind == IdeSymbolKind::Struct
            && target.detail.as_deref().is_some_and(|d| d.starts_with("bitfield"))
        {
            bitfield_indices.insert(target.name.clone(), idx);
        }
    }
    // Move synthetic bitfield accessors into their parent's children
    let mut to_remove = Vec::new();
    for (idx, target) in roots.iter().enumerate() {
        for (bf_name, &parent_idx) in &bitfield_indices {
            if target.name.contains(&format!("_{bf_name}_"))
                && (target.name.starts_with("_get_")
                    || target.name.starts_with("_update_")
                    || target.name.starts_with("_set_")
                    || target.name.starts_with("Mk_"))
            {
                to_remove.push((idx, parent_idx));
                break;
            }
        }
    }
    // Apply nesting (reverse order to preserve indices)
    for &(child_idx, parent_idx) in to_remove.iter().rev() {
        if child_idx < roots.len() {
            let child = roots.remove(child_idx);
            // Adjust parent_idx if it was after the removed child
            let adjusted_parent = if parent_idx > child_idx { parent_idx - 1 } else { parent_idx };
            if let Some(parent) = roots.get_mut(adjusted_parent) {
                parent.children.push(child);
            }
        }
    }

    roots
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
        let Some(sig_index) = candidate_file.signature_index() else {
            continue;
        };
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
