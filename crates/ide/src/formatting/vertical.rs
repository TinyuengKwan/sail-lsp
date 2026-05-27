//! Vertical alignment — aligning struct fields, bitfield fields,
//! mapping arms, and register declarations.
//!
//! Items implementing `AlignedItem` can be formatted with their
//! separators (`:`, `<->`, `=>`) vertically aligned.

use ide_db::line_index::TextRange;

use super::rewrite::{RewriteContext, RewriteResult};
use super::shape::Shape;

/// Trait for items that support vertical alignment.
pub(crate) trait AlignedItem {
    /// Whether to skip this item during alignment (e.g., comment-only lines).
    fn skip(&self) -> bool;

    /// Source range of this item.
    fn get_range(&self) -> TextRange;

    /// Rewrite just the prefix (text before the alignment separator).
    /// Used to measure maximum prefix width.
    fn rewrite_prefix(
        &self,
        context: &RewriteContext<'_>,
        shape: Shape,
    ) -> RewriteResult;

    /// Rewrite the full item, padding the prefix to `prefix_max_width`.
    fn rewrite_aligned_item(
        &self,
        context: &RewriteContext<'_>,
        shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult;
}

/// Format a slice of aligned items with padding to align separators.
///
/// Algorithm:
/// 1. Rewrite each prefix to measure its width.
/// 2. Find the maximum prefix width.
/// 3. Rewrite each item with padding to that max width.
/// 4. Join with newlines.
pub(crate) fn rewrite_with_alignment<T: AlignedItem>(
    items: &[T],
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    if items.is_empty() {
        return Some(String::new());
    }

    // Each group is aligned independently.
    let groups = group_aligned_items(items);
    let mut all_results = Vec::new();

    for group in &groups {
        let formatted = rewrite_aligned_group(group, context, shape)?;
        all_results.push(formatted);
    }

    // Join groups with a blank line between them.
    Some(all_results.join("\n\n"))
}

/// Format a single group of aligned items.
fn rewrite_aligned_group<T: AlignedItem>(
    items: &[T],
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    if items.is_empty() {
        return Some(String::new());
    }

    // Phase 1: measure maximum prefix width.
    let mut max_prefix_width = 0usize;
    for item in items {
        if item.skip() {
            continue;
        }
        match item.rewrite_prefix(context, shape) {
            Ok(prefix) => {
                max_prefix_width = max_prefix_width.max(prefix.len());
            }
            Err(_) => continue,
        }
    }

    // If max_prefix exceeds half the available width, fall back to
    // no alignment.
    let align_threshold = shape.width / 2;
    if max_prefix_width > align_threshold {
        // Too wide to align — rewrite each item without padding.
        let mut result = Vec::new();
        for item in items {
            if item.skip() {
                result.push(context.snippet(item.get_range()).to_string());
                continue;
            }
            match item.rewrite_aligned_item(context, shape, 0) {
                Ok(s) => result.push(s),
                Err(_) => result.push(context.snippet(item.get_range()).to_string()),
            }
        }
        let indent = shape.indent.to_string_inner(context.config);
        return Some(result.iter().map(|s| format!("{indent}{s}")).collect::<Vec<_>>().join("\n"));
    }

    // Phase 2: rewrite each item with aligned padding.
    let indent = shape.indent.to_string_inner(context.config);
    let mut result = Vec::new();
    for item in items {
        if item.skip() {
            // Preserve comments/blank lines verbatim.
            result.push(context.snippet(item.get_range()).to_string());
            continue;
        }
        match item.rewrite_aligned_item(context, shape, max_prefix_width) {
            Ok(s) => result.push(format!("{indent}{s}")),
            Err(_) => {
                // Fallback: emit source verbatim.
                result.push(context.snippet(item.get_range()).to_string());
            }
        }
    }

    Some(result.join("\n"))
}

/// Split aligned items into groups separated by blank lines.
/// Each group is aligned independently.
pub(crate) fn group_aligned_items<T: AlignedItem>(items: &[T]) -> Vec<&[T]> {
    if items.is_empty() {
        return Vec::new();
    }

    let mut groups: Vec<&[T]> = Vec::new();
    let mut start = 0;

    for i in 1..items.len() {
        // Detect group boundary: gap of 2+ lines between items
        // (byte distance > 2 suggests a blank line separator).
        let prev_end: usize = items[i - 1].get_range().end().into();
        let curr_start: usize = items[i].get_range().start().into();
        if curr_start.saturating_sub(prev_end) > 2 {
            groups.push(&items[start..i]);
            start = i;
        }
    }

    // Push the last group.
    groups.push(&items[start..]);
    groups
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::ide_types::FormatOptions;
    use super::super::snippet::SnippetProvider;

    /// Test helper: a simple aligned item with known prefix and suffix.
    struct TestField {
        prefix: String,
        suffix: String,
        range: TextRange,
    }

    impl AlignedItem for TestField {
        fn skip(&self) -> bool { false }
        fn get_range(&self) -> TextRange { self.range }
        fn rewrite_prefix(&self, _ctx: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
            Ok(self.prefix.clone())
        }
        fn rewrite_aligned_item(
            &self, _ctx: &RewriteContext<'_>, _shape: Shape, prefix_max_width: usize,
        ) -> RewriteResult {
            let padding = prefix_max_width.saturating_sub(self.prefix.len());
            Ok(format!("{}{} : {}", self.prefix, " ".repeat(padding), self.suffix))
        }
    }

    #[test]
    fn align_two_fields() {
        let source = "x : int,\ny_offset : bits(32),\n";
        let snippet = SnippetProvider::new(source.to_string());
        let config = FormatOptions::default();
        let ctx = super::super::rewrite::RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);

        let items = vec![
            TestField {
                prefix: "x".into(), suffix: "int".into(),
                range: base_db::text_range(0, 8),
            },
            TestField {
                prefix: "y_offset".into(), suffix: "bits(32)".into(),
                range: base_db::text_range(9, 28),
            },
        ];

        let result = rewrite_with_alignment(&items, &ctx, shape).unwrap();
        assert!(result.contains("x        : int"), "got: {result}");
        assert!(result.contains("y_offset : bits(32)"), "got: {result}");
    }

    #[test]
    fn align_empty() {
        let snippet = SnippetProvider::new(String::new());
        let config = FormatOptions::default();
        let ctx = super::super::rewrite::RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let items: Vec<TestField> = vec![];
        let result = rewrite_with_alignment(&items, &ctx, shape).unwrap();
        assert_eq!(result, "");
    }
}
