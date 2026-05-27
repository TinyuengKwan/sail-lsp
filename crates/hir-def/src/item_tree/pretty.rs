//! Pretty-printing for ItemTree (debug/diagnostic use).

use std::fmt::Write;

use super::{ItemTree, ModItem};

/// Render an ItemTree to a human-readable string for debugging.
///
/// Each top-level item is printed as `Kind name: signature`.
pub fn print_item_tree(tree: &ItemTree) -> String {
    let mut buf = String::new();
    for &item in tree.top_level_items() {
        print_mod_item(&mut buf, tree, item);
    }
    buf
}

fn print_mod_item(buf: &mut String, tree: &ItemTree, item: ModItem) {
    let kind = item.item_kind(tree);
    let name = item.name(tree);
    let sig = item.signature(tree);
    writeln!(buf, "{kind:?} {name}: {sig}").unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_prints_function() {
        let source = "function foo(x : int) -> int = x + 1\n";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let output = print_item_tree(&tree);
        assert!(output.contains("Function foo:"), "got: {output}");
    }

    #[test]
    fn pretty_prints_empty_tree() {
        let source = "";
        let (root, _) = syntax::parse_text(source);
        let tree = ItemTree::build_from_cst(&root);
        let output = print_item_tree(&tree);
        assert!(output.is_empty());
    }
}
