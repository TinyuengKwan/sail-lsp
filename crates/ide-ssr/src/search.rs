//! Scan expression nodes, with cancellation points before structural matching.

use syntax::SyntaxNode;

use crate::matching::{Match, Matcher};
use crate::resolving::ResolvedRule;

/// Search for AST-level matches of a resolved rule in a syntax tree.
///
/// attempts matching at each node.
pub(crate) fn search_in_tree(
    db: &dyn salsa::Database,
    rule: &ResolvedRule,
    root: &SyntaxNode,
) -> Vec<Match> {
    let matcher = Matcher::new(&rule.pattern.placeholders_by_stand_in);
    let pattern = &rule.pattern.node;
    let mut matches = Vec::new();

    for node in root.descendants().filter(|node| rule.kind.accepts(node.kind())) {
        db.unwind_if_revision_cancelled();
        // Skip trivia-only or too-small nodes.
        if node.text_range().len() == rowan::TextSize::from(0u32) {
            continue;
        }
        if let Ok(captures) = matcher.try_match(pattern, &node) {
            matches.push(Match {
                token_mapping: matcher.token_mapping(pattern, &node),
                matched_node: node.clone(),
                placeholder_values: captures,
            });
        }
    }

    matches
}
