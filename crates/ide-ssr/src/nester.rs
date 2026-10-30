//! Keep outermost matches within a single syntax tree.
use crate::matching::Match;

pub(crate) fn deduplicate(mut matches: Vec<Match>) -> Vec<Match> {
    matches.sort_by_key(|m| {
        let range = m.matched_node.text_range();
        (range.start(), std::cmp::Reverse(range.end()))
    });
    let mut previous: Option<syntax::TextRange> = None;
    matches.retain(|m| {
        let range = m.matched_node.text_range();
        if previous.is_some_and(|outer| outer.contains_range(range)) {
            false
        } else {
            previous = Some(range);
            true
        }
    });
    matches
}
