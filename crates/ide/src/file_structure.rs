//! File structure — the document outline.
//!
//! rust-analyzer's placement: `ide/src/file_structure.rs` builds what
//! `textDocument/documentSymbol` answers, and `ide-db/src/symbol_index.rs`
//! is the *workspace* index. This module used to be 92 lines flattening
//! `ide_db::signatures::document_symbols_ide` into a parent-indexed list
//! that nothing consumed — a second representation of an outline the
//! protocol wants as a tree — while the builder itself sat one crate down.

use std::collections::HashMap;

use crate::navigation_target::NavigationTarget;
use ide_db::FileDb;

/// The outline `textDocument/documentSymbol` answers with.
///
/// Enum members become children of their parent enum; every other
/// top-level declaration is a root. Each symbol carries its precise
/// signature text from the `ItemTree` as `detail`, rather than the bare
/// category label.
pub fn file_structure(file: &dyn FileDb) -> Vec<NavigationTarget> {
    use ide_db::ide_types::SymbolKind as IdeSymbolKind;

    let Some(tree) = file.item_tree() else {
        return Vec::new();
    };

    let placeholder = url::Url::parse("file:///").expect("a literal URL parses");
    let target = |name: String,
                  kind: IdeSymbolKind,
                  detail: String,
                  full: base_db::TextRange,
                  focus: base_db::TextRange| {
        // Single-file outline: the caller knows which file it asked about,
        // so the URL is a placeholder.
        NavigationTarget::new(placeholder.clone(), name, kind, full, focus).with_detail(detail)
    };

    let mut roots: Vec<NavigationTarget> = Vec::new();
    // Where each declaration's entry is, so what it generates can join it.
    let mut root_of: HashMap<hir::DefItem, usize> = HashMap::new();
    let mut generated: Vec<(hir::DefItem, NavigationTarget)> = Vec::new();
    for &id in tree.top_level_items() {
        let kind = match id.def_kind(tree) {
            hir::DefKind::Function
            | hir::DefKind::ValSpec
            | hir::DefKind::Mapping
            | hir::DefKind::MappingSpec
            | hir::DefKind::Overload
            | hir::DefKind::Outcome
            | hir::DefKind::ScatteredHead
            | hir::DefKind::ScatteredClause => IdeSymbolKind::Function,
            hir::DefKind::Register | hir::DefKind::Let | hir::DefKind::Var => {
                IdeSymbolKind::Variable
            }
            hir::DefKind::TypeAlias
            | hir::DefKind::Struct
            | hir::DefKind::Union
            | hir::DefKind::Bitfield
            | hir::DefKind::Newtype => IdeSymbolKind::Struct,
            hir::DefKind::Enum => IdeSymbolKind::Enum,
            _ => continue,
        };
        let span = id.span(tree);
        let name_span = id.name_span(tree);
        let mut root = target(
            id.name(tree).to_string(),
            kind,
            id.signature(tree).to_string(),
            base_db::text_range(span.start, span.end),
            base_db::text_range(name_span.start, name_span.end),
        );
        // Members belong to the definition that declares them. Parenting
        // used to be decided by span containment, with "the most recently
        // declared enum" as a fallback when that failed.
        if matches!(id.def_kind(tree), hir::DefKind::Enum | hir::DefKind::Union) {
            for field in id.fields(tree) {
                let r = base_db::text_range(field.name_span.start, field.name_span.end);
                root.children.push(target(
                    field.name.to_string(),
                    IdeSymbolKind::EnumMember,
                    "enum member".to_string(),
                    r,
                    r,
                ));
            }
        }
        match tree.generated.get(&id) {
            Some(origin) => generated.push((origin.owner, root)),
            None => {
                root_of.insert(id, roots.len());
                roots.push(root);
            }
        }
    }

    // What a declaration generates — a bitfield's accessors, an enum's
    // conversions, a mapping's directions — is listed under it, in source
    // order. The item tree records the owner; matching on `_get_`/`_set_`
    // name prefixes left `Mk_B` and every `_mod_F`/`update_F` overload at the
    // top level, seven per bitfield, and listed the rest in reverse.
    for (owner, entry) in generated {
        match root_of.get(&owner) {
            Some(&at) => roots[at].children.push(entry),
            None => roots.push(entry),
        }
    }

    roots
}

#[cfg(test)]
mod tests {
    use crate::fixture;

    /// Through `Analysis::document_symbols`, the one door.
    ///
    /// The builder used to live in `ide_db::symbol_index` — the *workspace*
    /// index — with this module a flat adapter over it that nothing called.
    #[test]
    fn the_outline_names_what_the_file_declares() {
        let source = "\
val add : (int, int) -> int
function add(x, y) = x + y
enum E = {A, B}
";
        let (analysis, files) = fixture::analysis(source);
        let symbols = analysis.document_symbols(files[0].0).expect("no cancellation");

        let names: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"add"), "missing `add`: {names:?}");
        assert!(names.contains(&"E"), "missing `E`: {names:?}");

        // A signature, not the bare category label.
        let add = symbols.iter().find(|s| s.name == "add").expect("add");
        assert!(add.detail.is_some(), "no detail on `add`: {add:?}");

        // Enum members are children of their enum, not roots.
        let e = symbols.iter().find(|s| s.name == "E").expect("E");
        let members: Vec<&str> = e.children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&members[..2], ["A", "B"], "enum members are not its children");
    }
    /// What a declaration generates sits under it, in order, and nothing
    /// generated is a root of its own.
    #[test]
    fn generated_names_are_children_of_what_generates_them() {
        let source = "//- /main.sail\nbitfield B : bits(8) = { HI : 7 .. 4, LO : 3 .. 0 }\n\
                      enum E = {A, Z}\nfunction f() -> unit = ()\n";
        let (analysis, files) = fixture::analysis(source);
        let symbols = analysis.document_symbols(files[0].0).expect("no cancellation");
        let roots: Vec<&str> = symbols.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(roots, ["B", "E", "f"]);
        let b: Vec<&str> = symbols[0].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(b[0], "Mk_B");
        let at = |n: &str| b.iter().position(|c| *c == n).unwrap_or_else(|| panic!("{n}: {b:?}"));
        assert!(at("_get_B_HI") < at("_set_B_HI") && at("_set_B_HI") < at("_get_B_LO"), "{b:?}");
        assert!(at("update_HI") > at("_get_B_HI"), "{b:?}");
        let e: Vec<&str> = symbols[1].children.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(&e[..2], ["A", "Z"], "members first: {e:?}");
        assert!(e.len() > 2, "the enum's conversions are under it too: {e:?}");
    }
}
