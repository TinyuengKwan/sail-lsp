//! Overlap deduplication for SSR matches.
//!
//! When matches overlap, keeps only the outermost match.

use crate::matching::{Match, SsrMatches};

/// Remove overlapping AST-level matches, keeping only the outermost.
pub(crate) fn deduplicate(mut matches: SsrMatches) -> SsrMatches {
    if matches.matches.len() <= 1 {
        return matches;
    }

    matches.matches.sort_by(|a, b| {
        a.range
            .range
            .start()
            .cmp(&b.range.range.start())
            .then_with(|| b.range.range.len().cmp(&a.range.range.len()))
    });

    let mut accepted: Vec<Match> = Vec::new();
    for m in matches.matches {
        let overlaps = accepted.iter().any(|existing| {
            existing.range.file_id == m.range.file_id
                && existing.range.range.start() <= m.range.range.start()
                && m.range.range.end() <= existing.range.range.end()
        });
        if !overlaps {
            accepted.push(m);
        }
    }
    SsrMatches { matches: accepted }
}
