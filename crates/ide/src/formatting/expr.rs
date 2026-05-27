//! Expression formatting — match arms, if/then/else, binary operators.
//!
//! Implements Rewrite-style formatting for Sail expression nodes
//! using CST node ranges for precise source extraction.

use parser::SyntaxKind as SK;
use syntax::SyntaxNode;

use super::rewrite::RewriteContext;
use super::shape::Shape;
use super::snippet::SnippetProvider;
use super::vertical::{rewrite_with_alignment, AlignedItem};
use ide_db::line_index::TextRange;

/// A match arm parsed from CST or text: `pattern => body`.
#[allow(dead_code)] // Activated when visitor computes inner shape for nested match.
pub(crate) struct MatchArmItem {
    /// Text before `=>` (pattern + optional guard).
    pub(crate) prefix: String,
    /// Text after `=>` (body).
    pub(crate) suffix: String,
    /// Source range of this arm.
    pub(crate) range: TextRange,
    /// Whether this line is a comment (skip alignment).
    pub(crate) is_comment: bool,
}

impl AlignedItem for MatchArmItem {
    fn skip(&self) -> bool {
        self.is_comment
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
    ) -> super::rewrite::RewriteResult {
        Ok(self.prefix.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> super::rewrite::RewriteResult {
        if self.is_comment {
            return Ok(format!("{}{}", self.prefix, self.suffix));
        }
        let padding = prefix_max_width.saturating_sub(self.prefix.len());
        Ok(format!("{}{} => {}", self.prefix, " ".repeat(padding), self.suffix))
    }
}

/// Extract match arms from a MATCH_EXPR node's body text.
///
/// Uses CST text_range for the body block, then parses lines
/// looking for `=>` separators.
#[allow(dead_code)] // Activated when visitor computes inner shape for nested match.
pub(crate) fn extract_match_arms(
    node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Vec<MatchArmItem> {
    let mut arms = Vec::new();

    // Find the body block (between { and })
    let range = node.text_range();
    let text = snippet.span_to_snippet(range);

    // Find opening brace
    let brace_start = match text.find('{') {
        Some(p) => p + 1,
        None => return arms,
    };
    let brace_end = match text.rfind('}') {
        Some(p) => p,
        None => return arms,
    };

    let body = &text[brace_start..brace_end];
    let body_offset: usize = range.start().into();
    let body_start = body_offset + brace_start;

    let mut line_offset = body_start;
    for line in body.split('\n') {
        let trimmed = line.trim();
        let line_len = line.len() + 1; // +1 for \n

        if trimmed.is_empty() {
            line_offset += line_len;
            continue;
        }

        let line_range = base_db::text_range(line_offset, line_offset + line.len());

        if trimmed.starts_with("//") || trimmed.starts_with("/*") {
            arms.push(MatchArmItem {
                prefix: trimmed.to_string(),
                suffix: String::new(),
                range: line_range,
                is_comment: true,
            });
        } else if let Some(arrow_pos) = find_fat_arrow(trimmed) {
            let prefix = trimmed[..arrow_pos].trim_end().to_string();
            let suffix = trimmed[arrow_pos + 2..].trim_start().to_string();
            // Strip trailing comma from suffix
            let suffix = suffix.strip_suffix(',').unwrap_or(&suffix).trim_end().to_string();
            arms.push(MatchArmItem { prefix, suffix, range: line_range, is_comment: false });
        } else {
            // Non-arm line (e.g., continuation) — preserve verbatim
            arms.push(MatchArmItem {
                prefix: trimmed.to_string(),
                suffix: String::new(),
                range: line_range,
                is_comment: true, // skip alignment
            });
        }

        line_offset += line_len;
    }

    arms
}

/// Find `=>` position in a line, skipping strings.
#[allow(dead_code)] // Used by extract_match_arms.
fn find_fat_arrow(line: &str) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len().saturating_sub(1) {
        if bytes[i] == b'"' {
            in_string = !in_string;
        }
        if !in_string && bytes[i] == b'=' && bytes[i + 1] == b'>' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Rewrite a MATCH_EXPR with aligned `=>` arrows.
#[allow(dead_code)] // Activated when visitor computes inner shape for nested match.
pub(crate) fn rewrite_match_expr(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let arms = extract_match_arms(node, context.snippet_provider);
    if arms.is_empty() || arms.iter().all(|a| a.is_comment) {
        return None; // Nothing to align
    }

    // Extract header (everything before {)
    let range = node.text_range();
    let text = context.snippet(range);
    let brace_pos = text.find('{')?;
    let header = text[..=brace_pos].to_string();

    // block_indent for indented arms, visual_indent for arm body
    // continuation, block() to reset alignment for close brace.
    let inner_shape = shape.block_indent(context.config);
    let _arm_body_shape = inner_shape.visual_indent(4); // arm body continuation
    let aligned = rewrite_with_alignment(&arms, context, inner_shape)?;

    // Reconstruct with close brace at block-only indent (no alignment).
    let close_shape = shape.block();
    let indent = close_shape.indent.to_string_inner(context.config);
    let close_brace = format!("{indent}}}");

    // Check for trailing content after }
    let brace_end = text.rfind('}')?;
    let trailer = &text[brace_end + 1..];

    Some(format!("{header}\n{aligned}\n{close_brace}{trailer}"))
}

/// Rewrite an IF_EXPR with aligned then/else.
///
/// Handles two patterns:
/// 1. Single if: `if cond then body else body`
/// 2. Chained: `if c1 then b1 else if c2 then b2 else b3`
///
/// For chained if-else-if, aligns the `if/else if/then` keywords.
#[allow(dead_code)] // Activated when visitor computes inner shape for nested if.
pub(crate) fn rewrite_if_expr(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let range = node.text_range();
    let text = context.snippet(range);

    // Only rewrite multi-line if expressions
    if !text.contains('\n') {
        return None;
    }

    // Check for chained if-else-if pattern
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() < 2 {
        return None;
    }

    // Detect if this is an if-else-if chain by checking for
    // `else if` pattern
    let has_chain = lines.iter().any(|l| {
        let t = l.trim();
        t.starts_with("else if") || t.starts_with("} else if")
    });

    if !has_chain {
        // Simple if/then/else — preserve as-is (handled by indentation)
        return None;
    }

    // If the shape is too narrow, we still format but the overflow will be
    // caught by the ExceedsMaxWidth check in rewrite_result.
    let _cond_shape = shape.sub_width(3); // 3 = "if ".len()

    // For chained if-else-if, ensure consistent alignment:
    // Each `if`/`else if` should align, and `then` should align.
    let indent = shape.indent.to_string_inner(context.config);
    let mut result = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            result.push(String::new());
            continue;
        }

        // Determine if this is a condition line or body line
        if trimmed.starts_with("if ") || trimmed.starts_with("else if ") {
            result.push(format!("{indent}{trimmed}"));
        } else if trimmed.starts_with("else") {
            result.push(format!("{indent}{trimmed}"));
        } else {
            // Body line — add one level of indentation
            let body_indent = shape.indent.block_indent(context.config);
            let body_str = body_indent.to_string_inner(context.config);
            result.push(format!("{body_str}{trimmed}"));
        }
    }

    Some(result.join("\n"))
}

/// Normalize operator spacing in a BIN_EXPR node.
///
/// Ensures single space around binary operators: `&`, `|`, `^`,
/// `==`, `!=`, `>=`, `<=`.
pub(crate) fn normalize_binexpr_spacing(
    node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Option<String> {
    // Walk children_with_tokens to find the operator token
    let mut parts = Vec::new();
    let mut found_op = false;

    for child in node.children_with_tokens() {
        match child.kind() {
            // Binary operators that need spacing
            SK::AMP
            | SK::PIPE
            | SK::CARET
            | SK::EQ_EQ
            | SK::NEQ
            | SK::GE
            | SK::LE
            | SK::PLUS
            | SK::MINUS
            | SK::STAR
            | SK::SLASH
            | SK::PERCENT => {
                let op_text = child.as_token().map(|t| t.text().to_string()).unwrap_or_default();
                // Ensure space before and after
                if let Some(last) = parts.last_mut() {
                    let s: &mut String = last;
                    if !s.ends_with(' ') {
                        s.push(' ');
                    }
                }
                parts.push(op_text);
                parts.push(" ".to_string()); // space after op
                found_op = true;
            }
            // Skip whitespace tokens (we're re-spacing ourselves)
            SK::WHITESPACE => {
                // Only add if not adjacent to an operator we just handled
                if !found_op {
                    if let Some(last) = parts.last() {
                        if !last.ends_with(' ') {
                            parts.push(" ".to_string());
                        }
                    }
                }
                found_op = false;
            }
            _ => {
                found_op = false;
                let text = match child {
                    rowan::NodeOrToken::Node(n) => {
                        snippet.span_to_snippet(n.text_range()).to_string()
                    }
                    rowan::NodeOrToken::Token(t) => t.text().to_string(),
                };
                parts.push(text);
            }
        }
    }

    let result: String = parts.concat();
    let original = snippet.span_to_snippet(node.text_range());

    // Only return if we actually changed something
    if result.trim() != original.trim() {
        Some(result)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::ide_types::FormatOptions;

    #[test]
    fn extract_match_arms_basic() {
        let src = "match x {\n  A => 1,\n  BB => 22,\n  CCC => 333,\n}\n";
        let snippet = SnippetProvider::new(src.to_string());
        let (root, _) = syntax::parse_text(src);
        // Find match expr
        let match_node = root.descendants().find(|n| n.kind() == SK::MATCH_EXPR);
        if let Some(node) = match_node {
            let arms = extract_match_arms(&node, &snippet);
            assert_eq!(arms.len(), 3);
            assert_eq!(arms[0].prefix, "A");
            assert_eq!(arms[1].prefix, "BB");
            assert_eq!(arms[2].prefix, "CCC");
        }
    }

    #[test]
    fn match_arm_alignment() {
        let src = "match x {\n  A => 1,\n  BB => 22,\n  CCC => 333,\n}\n";
        let snippet = SnippetProvider::new(src.to_string());
        let config = FormatOptions::default();
        let ctx = super::super::rewrite::RewriteContext::new(&config, &snippet);
        let shape = super::super::shape::Shape::with_max_width(&config);
        let (root, _) = syntax::parse_text(src);
        let match_node = root.descendants().find(|n| n.kind() == SK::MATCH_EXPR);
        if let Some(node) = match_node {
            let result = rewrite_match_expr(&node, &ctx, shape);
            if let Some(formatted) = result {
                // All => should be at same column
                assert!(formatted.contains("A   => 1"), "got: {formatted}");
                assert!(formatted.contains("BB  => 22"), "got: {formatted}");
                assert!(formatted.contains("CCC => 333"), "got: {formatted}");
            }
        }
    }

    #[test]
    fn find_fat_arrow_basic() {
        assert_eq!(find_fat_arrow("A => 1"), Some(2));
        assert_eq!(find_fat_arrow("  BB => 22"), Some(5));
    }

    #[test]
    fn find_fat_arrow_in_string() {
        // => inside string should not match
        assert_eq!(find_fat_arrow("\"=>\" => x"), Some(5));
    }

    #[test]
    fn single_line_if_unchanged() {
        let src = "if x then y else z";
        let snippet = SnippetProvider::new(src.to_string());
        let config = FormatOptions::default();
        let ctx = super::super::rewrite::RewriteContext::new(&config, &snippet);
        let shape = super::super::shape::Shape::with_max_width(&config);
        let (root, _) = syntax::parse_text(src);
        let if_node = root.descendants().find(|n| n.kind() == SK::IF_EXPR);
        if let Some(node) = if_node {
            // Single-line if should not be rewritten
            assert!(rewrite_if_expr(&node, &ctx, shape).is_none());
        }
    }
}
