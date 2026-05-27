//! Fuzz testing entry points.

use crate::{SourceFile, SyntaxNode};

/// Fuzz target: parse arbitrary bytes as a Sail source file.
///
/// Returns `None` if the input isn't valid UTF-8.
pub fn fuzz_syntax_tree(text: &str) -> Option<SyntaxNode> {
    let parse = SourceFile::parse(text);
    let node = parse.syntax_node();
    // Smoke-test: iterate all descendants to ensure no panics.
    for _ in node.descendants() {}
    Some(node)
}
