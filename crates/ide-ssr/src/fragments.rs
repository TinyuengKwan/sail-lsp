//! Parse an expression fragment in a minimal Sail binding.
use ide_db::syntax_helpers::node_ext::is_expr_kind;
use syntax::SyntaxNode;

pub(crate) fn expr(s: &str) -> Option<SyntaxNode> {
    // Try wrapping as: `let __ssr_frag__ = <expr>`
    let wrapped = format!("let __ssr_frag__ = {s}");
    let (root, errors) = syntax::parse_text(&wrapped);
    if !errors.is_empty() {
        return None;
    }
    // Find the expression node (child of the let binding, after the '=').
    let node = root.descendants().find(|node| is_expr_kind(node.kind()))?;
    (node.text().to_string().trim() == s.trim()).then_some(node)
}
