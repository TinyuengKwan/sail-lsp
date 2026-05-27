//! Debug command: view the CST (concrete syntax tree) for a file.
//!
//! the rowan CST with indentation showing the tree structure.

use ide_db::FileDb;

/// Pretty-print the CST for a file as indented text.
///
/// Registered as custom LSP command `sail-lsp/viewSyntaxTree`.
pub fn view_syntax_tree(file: &dyn FileDb) -> String {
    let text = file.text();
    if text.is_empty() {
        return "(empty file)".to_string();
    }

    let (root, errors) = syntax::parse_text(text);
    let mut output = String::new();

    // Walk tree with preorder traversal
    for event in root.preorder_with_tokens() {
        match event {
            rowan::WalkEvent::Enter(node_or_token) => {
                let depth = ancestors_count(&node_or_token);
                let indent = "  ".repeat(depth);
                match node_or_token {
                    rowan::NodeOrToken::Node(node) => {
                        let range = node.text_range();
                        output.push_str(&format!(
                            "{indent}{:?}@{}..{}\n",
                            node.kind(),
                            u32::from(range.start()),
                            u32::from(range.end()),
                        ));
                    }
                    rowan::NodeOrToken::Token(token) => {
                        let range = token.text_range();
                        let text_preview = token.text();
                        let preview = if text_preview.len() > 40 {
                            format!("{}...", &text_preview[..37])
                        } else {
                            text_preview.to_string()
                        };
                        output.push_str(&format!(
                            "{indent}{:?}@{}..{} {:?}\n",
                            token.kind(),
                            u32::from(range.start()),
                            u32::from(range.end()),
                            preview,
                        ));
                    }
                }
            }
            rowan::WalkEvent::Leave(_) => {}
        }
    }

    if !errors.is_empty() {
        output.push_str(&format!("\n// {} parse error(s)\n", errors.len()));
    }

    output
}

fn ancestors_count(
    node_or_token: &rowan::NodeOrToken<syntax::SyntaxNode, syntax::SyntaxToken>,
) -> usize {
    match node_or_token {
        rowan::NodeOrToken::Node(n) => n.ancestors().count().saturating_sub(1),
        rowan::NodeOrToken::Token(t) => t.parent().map(|p| p.ancestors().count()).unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::test_utils::TestFile;

    #[test]
    fn view_simple_cst() {
        let file = TestFile::new("val x : int\n");
        let output = view_syntax_tree(&file);
        assert!(output.contains("SOURCE_FILE"), "should have root node");
        assert!(output.contains("KW_VAL"), "should contain val keyword");
    }

    #[test]
    fn view_empty() {
        let file = TestFile::new("");
        let output = view_syntax_tree(&file);
        assert!(output.contains("empty"));
    }
}
