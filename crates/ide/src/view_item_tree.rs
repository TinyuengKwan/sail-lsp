//! Debug command: view the ItemTree for a file.
//!
//! per-file public surface (signatures, kinds, doc comments)
//! as a formatted string for debugging.

use ide_db::FileDb;

/// Pretty-print the ItemTree for a file.
///
/// Registered as custom LSP command `sail-lsp/viewItemTree`.
pub fn view_item_tree(file: &dyn FileDb) -> String {
    let Some(tree) = file.item_tree() else {
        return "(no item tree available)".to_string();
    };

    let mut lines = Vec::new();
    lines.push(format!(
        "// ItemTree: {} entries, signature_hash: {:016x}",
        tree.len(),
        tree.signature_hash
    ));
    lines.push(String::new());

    for (idx, &id) in tree.top_level_items().iter().enumerate() {
        let doc_marker = if id.doc(tree).is_some() { " [doc]" } else { "" };
        let clause_marker = if id.is_clause(tree) { " [clause]" } else { "" };
        let member = id.member_name(tree).map(|m| format!(" member={m}")).unwrap_or_default();
        let span = id.span(tree);
        lines.push(format!(
            "{idx:>3}  {:?}{clause_marker}{member}{doc_marker}",
            id.item_kind(tree),
        ));
        lines.push(format!("      {}", id.signature(tree)));
        lines.push(format!("      span: {}..{}", span.start, span.end));
    }

    if !tree.fixities.is_empty() {
        lines.push(String::new());
        lines.push(format!("// Fixities: {} declarations", tree.fixities.len()));
        for fix in &tree.fixities {
            lines.push(format!("  {:?} {} level={}", fix.assoc, fix.operator, fix.level));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn view_simple_item_tree() {
        let file = TestFile::new("val foo : int -> int\nfunction foo(x) = x + 1\n");
        let output = view_item_tree(&file);
        assert!(output.contains("ItemTree:"), "should have header");
        assert!(output.contains("foo"), "should contain function name");
        assert!(output.contains("ValSpec"), "should contain ValSpec kind");
    }

    #[test]
    fn view_empty_file() {
        let file = TestFile::new("");
        let output = view_item_tree(&file);
        assert!(output.contains("0 entries") || output.contains("no item tree"));
    }
}
