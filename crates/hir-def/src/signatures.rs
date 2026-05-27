//! Function and mapping signatures.
//! Stores the public surface of callables (function/mapping/val spec)
//! as structured data rather than raw signature strings. Used by
//! type inference, hover, completion, and signature help.

use crate::item_tree::{ItemTree, ModItem};
use crate::name::Name;
use parser::Span;

/// Structured signature for a function or val spec.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionSignature {
    /// Function name.
    pub name: Name,
    /// Raw signature text (parameter types + return type).
    pub signature_text: String,
    /// Source span of the definition.
    pub span: Span,
    /// Whether this is a function clause (vs a standalone function).
    pub is_clause: bool,
    /// Doc comment, if any.
    pub doc: Option<String>,
    /// The kind of item this signature comes from.
    pub kind: SignatureKind,
}

/// What kind of callable this signature represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SignatureKind {
    /// A function definition.
    Function,
    /// A val specification (type-only signature).
    ValSpec,
    /// A mapping definition.
    Mapping,
}

impl FunctionSignature {
    /// Extract signatures from an ItemTree.
    ///
    /// Returns one `FunctionSignature` per function, val spec, or mapping.
    pub fn all_from_item_tree(tree: &ItemTree) -> Vec<FunctionSignature> {
        let mut sigs = Vec::new();
        for item in tree.top_level_items() {
            match item {
                ModItem::Function(id) => {
                    let f = tree.function(*id);
                    sigs.push(FunctionSignature {
                        name: f.name.clone(),
                        signature_text: f.signature.clone(),
                        span: f.span,
                        is_clause: f.is_clause,
                        doc: f.doc.clone(),
                        kind: SignatureKind::Function,
                    });
                }
                ModItem::ValSpec(id) => {
                    let v = tree.val_spec(*id);
                    sigs.push(FunctionSignature {
                        name: v.name.clone(),
                        signature_text: v.signature.clone(),
                        span: v.span,
                        is_clause: false,
                        doc: v.doc.clone(),
                        kind: SignatureKind::ValSpec,
                    });
                }
                ModItem::Mapping(id) => {
                    let m = tree.mapping(*id);
                    sigs.push(FunctionSignature {
                        name: m.name.clone(),
                        signature_text: m.signature.clone(),
                        span: m.span,
                        is_clause: m.is_clause,
                        doc: m.doc.clone(),
                        kind: SignatureKind::Mapping,
                    });
                }
                _ => {}
            }
        }
        sigs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_signatures_from_cst() {
        let src = "val foo : int -> bool\nfunction foo(x) = true\n";
        let (root, _) = syntax::parse_text(src);
        let tree = ItemTree::build_from_cst(&root);
        let sigs = FunctionSignature::all_from_item_tree(&tree);
        assert!(sigs.len() >= 2, "should have val + function");
        let names: Vec<&str> = sigs.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"foo"));
    }
}
