//! Usage-index-accelerated search for SSR matches.
//!
//! Strategy:
//!   1. For rules with resolved paths → use find_usages to locate candidates
//!   2. Else → slow scan all nodes in scope
//!   3. At each candidate location, attempt matching against the pattern.

use hir_def::in_file::FileRange;
use syntax::SyntaxNode;

use crate::matching::{Match, Matcher, SsrMatches};
use crate::resolving::ResolvedRule;

/// Cache of usages for definitions found in patterns.
#[allow(dead_code)]
pub(crate) struct UsageCache {
    usages: Vec<(hir::PathResolution, Vec<FileRange>)>,
}

impl UsageCache {
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self { usages: Vec::new() }
    }
}

/// Search for AST-level matches of a resolved rule in a syntax tree.
///
/// attempts matching at each node.
pub(crate) fn search_in_tree(
    rule: &ResolvedRule,
    root: &SyntaxNode,
    file_id: base_db::FileId,
) -> SsrMatches {
    let matcher = Matcher::new(rule.pattern.placeholders_by_stand_in.clone(), rule.index);
    let pattern = &rule.pattern.node;
    let mut matches = Vec::new();

    for node in root.descendants() {
        // Skip trivia-only or too-small nodes.
        if node.text_range().len() == rowan::TextSize::from(0u32) {
            continue;
        }
        if let Ok(captures) = matcher.try_match(pattern, &node) {
            matches.push(Match {
                range: FileRange { file_id, range: node.text_range() },
                matched_node: node.clone(),
                placeholder_values: captures,
                ignored_comments: Vec::new(),
                rendered_template_paths: rustc_hash::FxHashMap::default(),
                rule_index: rule.index,
                depth: 0,
            });
        }
    }

    SsrMatches { matches }
}
