//! Assist integration: extract SSR rules from comments.
//! Comment format: `// pattern ==>> replacement`

use rowan::TextSize;

use crate::MatchFinder;

/// Extract an SSR rule from a comment at the given position.
///
/// Finds the comment token at `pos`, strips the `//` prefix, and
/// attempts to parse the remainder as an SSR rule.
///
/// Returns the MatchFinder with the rule added, and the range of the comment.
pub fn ssr_from_comment(
    db: &dyn salsa::Database,
    ft: base_db::FileText,
    pos: TextSize,
) -> Option<(MatchFinder<'_>, rowan::TextRange)> {
    // Get the file text.
    let text: &str = ft.text(db);

    // Find the line containing `pos`.
    let offset = u32::from(pos) as usize;
    if offset >= text.len() {
        return None;
    }

    // Find line start and end.
    let line_start = text[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[offset..].find('\n').map(|i| offset + i).unwrap_or(text.len());
    let line = &text[line_start..line_end];

    // Check if line is a comment with SSR syntax.
    let trimmed = line.trim_start();
    let comment_content = if let Some(rest) = trimmed.strip_prefix("//") {
        rest.trim()
    } else if let Some(rest) = trimmed.strip_prefix("/*") {
        rest.trim_end_matches("*/").trim()
    } else {
        return None;
    };

    // Must contain the SSR separator.
    if !comment_content.contains("==>>") {
        return None;
    }

    // Try to create a MatchFinder and add the rule.
    let mut finder = MatchFinder::in_context(db, ft).ok()?;
    let rule: crate::SsrRule = comment_content.parse().ok()?;
    finder.add_rule(rule).ok()?;

    let comment_range =
        rowan::TextRange::new(TextSize::from(line_start as u32), TextSize::from(line_end as u32));

    Some((finder, comment_range))
}
