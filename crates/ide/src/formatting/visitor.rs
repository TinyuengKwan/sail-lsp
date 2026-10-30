//! FmtVisitor — CST-walking formatter that accumulates output.
//!
//! Walks the rowan CST depth-first, formatting each definition and
//! preserving unformatted spans (comments, whitespace) between nodes.

use parser::SyntaxKind as SK;
use rowan::NodeOrToken;
use syntax::SyntaxNode;

use super::report::FormatReport;
use super::rewrite::RewriteContext;
use super::shape::{Indent, Shape};
use super::snippet::SnippetProvider;
use super::FormatOptions;

/// Check whether `node` has a `$[skip_format]` attribute among its children.
///
/// Looks for the token sequence DOLLAR → L_BRACK → IDENT("skip_format") → R_BRACK
/// anywhere in the node's direct children.
fn has_skip_format_attr(node: &SyntaxNode) -> bool {
    let tokens: Vec<_> = node
        .children_with_tokens()
        .filter_map(|elem| match elem {
            NodeOrToken::Token(t) => Some(t),
            _ => None,
        })
        .collect();
    tokens.windows(4).any(|w| {
        w[0].kind() == SK::DOLLAR
            && w[1].kind() == SK::L_BRACK
            && w[2].kind() == SK::IDENT
            && w[2].text() == "skip_format"
            && w[3].kind() == SK::R_BRACK
    })
}

/// Optional line range for range-formatting.
///
/// `None` means "format all lines". `Some((lo, hi))` means only
/// format lines in the inclusive range [lo, hi] (0-based).
/// Nodes outside this range are emitted verbatim.
pub(crate) type FileLines = Option<(usize, usize)>;

/// CST visitor that accumulates formatted output.
pub(crate) struct FmtVisitor<'a> {
    /// Accumulated formatted output.
    pub(crate) buffer: String,
    /// Byte offset up to which source has been consumed.
    pub(crate) last_pos: usize,
    /// Current block indentation.
    pub(crate) block_indent: Indent,
    /// Formatting configuration.
    pub(crate) config: &'a FormatOptions,
    /// Source text access.
    pub(crate) snippet_provider: &'a SnippetProvider,
    /// Error accumulator.
    pub(crate) report: FormatReport,
    /// Optional line range to format. Nodes outside are emitted verbatim.
    ///
    /// rustfmt's `out_of_file_lines_range!` macro checks this to skip
    /// formatting nodes outside the range.
    pub(crate) file_lines: FileLines,
}

impl<'a> FmtVisitor<'a> {
    /// Create a new visitor for the given source (format all lines).
    pub(crate) fn new(config: &'a FormatOptions, snippet_provider: &'a SnippetProvider) -> Self {
        Self {
            buffer: String::with_capacity(snippet_provider.len()),
            last_pos: 0,
            block_indent: Indent::empty(),
            config,
            snippet_provider,
            report: FormatReport::new(),
            file_lines: None,
        }
    }

    /// Create a visitor that only formats lines in the given range.
    ///
    /// Lines outside the range are emitted verbatim (no formatting).
    pub(crate) fn with_file_lines(
        config: &'a FormatOptions,
        snippet_provider: &'a SnippetProvider,
        file_lines: FileLines,
    ) -> Self {
        Self {
            buffer: String::with_capacity(snippet_provider.len()),
            last_pos: 0,
            block_indent: Indent::empty(),
            config,
            snippet_provider,
            report: FormatReport::new(),
            file_lines,
        }
    }

    /// Check if a node's span is outside the configured file_lines range.
    ///
    /// Returns true if the node should be skipped (emitted verbatim).
    fn out_of_file_lines_range(&self, node: &SyntaxNode) -> bool {
        let Some((lo, hi)) = self.file_lines else {
            return false; // None = format all
        };
        let range = node.text_range();
        let text = self.snippet_provider.entire_snippet();
        let start: usize = range.start().into();
        let end: usize = range.end().into();
        // Convert byte offsets to line numbers (0-based).
        let start_line = text[..start].matches('\n').count();
        let end_line = text[..end].matches('\n').count();
        // Out of range if the node's lines don't intersect [lo, hi].
        end_line < lo || start_line > hi
    }

    /// Current Shape (available width at current indent).
    pub(crate) fn shape(&self) -> Shape {
        Shape::indented(self.block_indent, self.config)
    }

    /// Push formatted text to the output buffer.
    pub(crate) fn push_str(&mut self, s: &str) {
        self.buffer.push_str(s);
    }

    /// Emit trailing content in a node's range that rewrite functions don't cover.
    ///
    /// The Sail parser attaches trailing tokens (attributes like `$[wavedrom ...]`,
    /// blank lines) to the preceding definition node as bare tokens (DOLLAR,
    /// L_BRACK, IDENT, ...) rather than as a child ATTRIBUTE node. Rewrite
    /// functions reconstruct the definition up to its last child *node*
    /// (e.g., BLOCK_EXPR ending at `}`), so any trailing tokens after that
    /// last child node are not included in the rewritten output.
    ///
    /// This method finds the end of the last child node and emits everything
    /// from there to the end of the parent node verbatim.
    fn emit_trailing_content(&mut self, node: &SyntaxNode) {
        let range = node.text_range();
        let node_end: usize = range.end().into();

        // Find the end of the last child *node* (not token). Child nodes are
        // the structural parts of the definition (TYPE_ARROW, BLOCK_EXPR, NAME,
        // BIN_EXPR, etc.). Bare tokens after the last child node are trailing
        // content the parser attached (attributes, trivia).
        let last_child_node_end: usize =
            node.children().last().map(|c| c.text_range().end().into()).unwrap_or(node_end);

        if last_child_node_end < node_end {
            let trailing = &self.snippet_provider.entire_snippet()[last_child_node_end..node_end];
            self.push_str(trailing);
        }
    }

    /// Walk the entire source file CST.
    ///
    /// Iterates top-level children of SOURCE_FILE, dispatching to
    /// visit_definition() for each DEFINITION/NAMED_DEF/CALLABLE_DEF.
    pub(crate) fn walk_source_file(&mut self, root: &SyntaxNode) {
        assert_eq!(root.kind(), SK::SOURCE_FILE);

        for child in root.children() {
            let range = child.text_range();
            let start: usize = range.start().into();
            let end: usize = range.end().into();

            // rustfmt uses format_missing_with_indent inside blocks.
            // At top level we use plain format_missing (no extra indent).
            self.format_missing(start);

            // Dispatch based on node kind.
            match child.kind() {
                SK::NAMED_DEF
                | SK::CALLABLE_DEF
                | SK::CALLABLE_SPEC
                | SK::TYPE_ALIAS_DEF
                | SK::SCATTERED_DEF
                | SK::SCATTERED_CLAUSE_DEF
                | SK::FIXITY_DEF
                | SK::DEFAULT_DEF
                | SK::DIRECTIVE_DEF
                | SK::END_DEF
                | SK::CONSTRAINT_DEF
                | SK::OUTCOME_DEF => {
                    self.visit_definition(&child);
                }
                _ => {
                    // Unknown node: emit source verbatim.
                    self.push_str(self.snippet_provider.span_to_snippet(range));
                }
            }

            self.last_pos = end;
        }

        // Emit trailing content (final comments, newlines).
        self.format_missing(self.snippet_provider.len());
    }

    /// Visit a single definition node.
    ///
    /// Dispatches to items.rs for NAMED_DEF (struct/bitfield/enum/union/register)
    /// and CALLABLE_DEF (mapping). Falls back to verbatim source on None.
    ///
    /// (mirrors `skip_out_of_file_lines_range_visitor!` at utils.rs:392-399).
    fn visit_definition(&mut self, node: &SyntaxNode) {
        let range = node.text_range();

        // If this node is outside the configured range, emit source verbatim
        // and skip formatting — exactly as rustfmt does.
        if self.out_of_file_lines_range(node) {
            self.push_str(self.snippet_provider.span_to_snippet(range));
            return;
        }

        // $[skip_format] — emit verbatim, skip all formatting.
        if has_skip_format_attr(node) {
            self.push_str(self.snippet_provider.span_to_snippet(range));
            return;
        }

        let ctx = self.rewrite_context();
        let shape = self.shape();

        let rewritten = match node.kind() {
            SK::NAMED_DEF => super::items::rewrite_named_def(node, &ctx, shape),
            SK::CALLABLE_DEF => {
                // Try mapping first (existing), then function signature (S31).
                super::items::rewrite_mapping_def(node, &ctx, shape)
                    .or_else(|| super::items::rewrite_function_def(node, &ctx, shape))
            }
            SK::CALLABLE_SPEC => super::items::rewrite_val_spec(node, &ctx, shape),
            SK::SCATTERED_CLAUSE_DEF => {
                super::items::rewrite_scattered_clause_def(node, &ctx, shape)
            }
            _ => None,
        };

        match rewritten {
            Some(formatted) => {
                // A definition whose signature was rewritten still needs its
                // expressions formatted. This used to push `formatted`
                // untouched, so the expression pass only ran on definitions
                // nothing else changed — and on a file where it did change
                // one, formatting again produced different text.
                let formatted = self.rewrite_exprs_of(&formatted);
                self.push_str(&formatted);
                if !matches!(node.kind(), SK::CALLABLE_DEF | SK::CALLABLE_SPEC) {
                    self.emit_trailing_content(node);
                }
            }
            None => {
                // No definition-level rewrite. Emit source but apply
                // expression-level formatting within the body.
                let source = self.snippet_provider.span_to_snippet(range);
                let formatted = self.apply_expr_rewrites(node, source);
                self.push_str(&formatted);
            }
        }
    }

    /// Normalise operator spacing inside a definition's source text.
    ///
    /// `BIN_EXPR` only. This used to claim it scanned for `MATCH_EXPR` and
    /// `IF_EXPR` too, which is why `expr::rewrite_match_expr` and
    /// `expr::rewrite_if_expr` have no caller — the comment described an
    /// intent, and the loop below never had those arms. Wiring
    /// `rewrite_match_expr` as it stands would break every single-line match
    /// onto several lines; it needs `rewrite_if_expr`'s "only when already
    /// multi-line" guard and a measurement first.
    fn apply_expr_rewrites(&self, node: &SyntaxNode, source: &str) -> String {
        apply_expr_rewrites_with(node, source, self.snippet_provider)
    }

    /// Format the expressions of a definition this visitor has already
    /// rewritten.
    ///
    /// `apply_expr_rewrites` maps offsets taken from a CST onto the string it
    /// is handed, so it can only run against the text that CST was parsed
    /// from. The `Some(formatted)` arm above produces *new* text, and used to
    /// return it untouched — so a definition whose signature needed rewriting
    /// never had its expressions formatted, and the expression pass landed on
    /// the *second* format instead. Re-parsing the definition is what makes
    /// both happen in one pass.
    fn rewrite_exprs_of(&self, text: &str) -> String {
        let (root, errors) = syntax::parse_text(text);
        if !errors.is_empty() {
            return text.to_string();
        }
        let snippet = super::snippet::SnippetProvider::new(text.to_string());
        let mut out = text.to_string();
        // Definitions are disjoint, so applying them last-first keeps the
        // earlier offsets valid.
        let defs: Vec<SyntaxNode> = root.children().collect();
        for def in defs.into_iter().rev() {
            let range = def.text_range();
            let (start, end) = (usize::from(range.start()), usize::from(range.end()));
            if end > out.len() {
                continue;
            }
            let rewritten = apply_expr_rewrites_with(&def, &out[start..end], &snippet);
            out.replace_range(start..end, &rewritten);
        }
        out
    }

    /// Create a RewriteContext for use by Rewrite implementations.
    pub(crate) fn rewrite_context(&self) -> RewriteContext<'a> {
        RewriteContext::new(self.config, self.snippet_provider)
    }
}

/// Replace each expression this formatter normalises inside `source`.
///
/// `source` must be the text `node` was parsed from: every offset is taken
/// from the CST and applied to the string.
fn apply_expr_rewrites_with(node: &SyntaxNode, source: &str, snippet: &SnippetProvider) -> String {
    let node_start: usize = node.text_range().start().into();

    // Collect replacements (offset_in_source, len, new_text).
    // Process in reverse order to preserve offsets.
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();

    for descendant in node.descendants() {
        let kind = descendant.kind();
        let desc_range = descendant.text_range();
        let rel_start: usize = usize::from(desc_range.start()) - node_start;
        let rel_end: usize = usize::from(desc_range.end()) - node_start;

        if rel_end > source.len() {
            continue;
        }

        if kind == SK::BIN_EXPR {
            // Only normalize single-line binary expressions.
            // Multi-line BIN_EXPR have intentional line breaks
            // (e.g., `<->` continuation) that must be preserved.
            let original = &source[rel_start..rel_end];
            if original.contains('\n') {
                continue;
            }
            if let Some(formatted) = super::expr::normalize_binexpr_spacing(&descendant, snippet) {
                if formatted.trim() != original.trim() {
                    replacements.push((rel_start, rel_end - rel_start, formatted));
                }
            }
        }
    }

    if replacements.is_empty() {
        return source.to_string();
    }

    // One replacement per region. An outer `BIN_EXPR` covers its inner
    // ones and already carries their normalisation (`normalized_child`),
    // so splicing the inner one first would shift the outer's recorded
    // length and the outer splice would then cut the wrong range.
    replacements.sort_by_key(|r| (r.0, std::cmp::Reverse(r.1)));
    let mut kept: Vec<(usize, usize, String)> = Vec::new();
    for r in replacements {
        if kept.last().is_some_and(|(o, l, _)| r.0 >= *o && r.0 + r.1 <= o + l) {
            continue;
        }
        kept.push(r);
    }
    let mut replacements = kept;

    // Apply replacements in reverse order.
    replacements.sort_by_key(|b| std::cmp::Reverse(b.0));
    let mut result = source.to_string();
    for (offset, len, new_text) in replacements {
        if offset + len <= result.len() {
            result.replace_range(offset..offset + len, &new_text);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format_with_visitor(text: &str) -> String {
        let snippet = SnippetProvider::new(text.to_string());
        let config = FormatOptions::default();
        let (root, _) = syntax::parse_text(text);
        let mut visitor = FmtVisitor::new(&config, &snippet);
        visitor.walk_source_file(&root);
        visitor.buffer
    }

    #[test]
    fn passthrough_simple() {
        let input = "val foo : int -> int\n";
        let output = format_with_visitor(input);
        assert_eq!(output, input);
    }

    #[test]
    fn passthrough_function() {
        let input = "function foo(x : int) -> int = x + 1\n";
        let output = format_with_visitor(input);
        assert_eq!(output, input);
    }

    #[test]
    fn passthrough_with_comments() {
        let input = "// header comment\nval foo : int\n// trailing\n";
        let output = format_with_visitor(input);
        assert_eq!(output, input);
    }

    fn format_with_file_lines(text: &str, file_lines: FileLines) -> String {
        let snippet = SnippetProvider::new(text.to_string());
        let config = FormatOptions::default();
        let (root, _) = syntax::parse_text(text);
        let mut visitor = FmtVisitor::with_file_lines(&config, &snippet, file_lines);
        visitor.walk_source_file(&root);
        visitor.buffer
    }

    #[test]
    fn file_lines_none_formats_all() {
        let input = "val foo : int\nval bar : bits(32)\n";
        let output = format_with_file_lines(input, None);
        assert_eq!(output, input); // passthrough, same result
    }

    #[test]
    fn file_lines_skips_out_of_range_nodes() {
        // Line 0: val foo : int
        // Line 1: struct S = {
        // Line 2:   x : int,
        // Line 3:   y_offset : bits(32),
        // Line 4: }
        // Line 5: val bar : bool
        let input =
            "val foo : int\nstruct S = {\n  x : int,\n  y_offset : bits(32),\n}\nval bar : bool\n";
        // Only format lines 1-4 (the struct). Lines 0 and 5 should be verbatim.
        let output = format_with_file_lines(input, Some((1, 4)));
        // val foo and val bar should be untouched
        assert!(output.starts_with("val foo : int\n"), "got: {output}");
        assert!(output.contains("val bar : bool"), "got: {output}");
    }

    #[test]
    fn out_of_file_lines_range_all_outside() {
        let input = "val foo : int\nval bar : bits(32)\n";
        // Range covers no lines that contain definitions
        let output = format_with_file_lines(input, Some((100, 200)));
        // Everything emitted verbatim
        assert_eq!(output, input);
    }

    #[test]
    fn preserves_dollar_bracket_attribute() {
        let input = "mapping foo : T <-> bits(6) = {\n  A <-> 0b01,\n  B <-> 0b10\n}\n\n$[wavedrom \"test\"]\nmapping clause bar = X(vm, vd)\n  <-> 0b010 @ vm\n  when true\n";

        let output = format_with_visitor(input);
        assert!(
            output.contains("wavedrom"),
            "visitor lost wavedrom!\nInput len: {}\nOutput len: {}\nOutput:\n{output}",
            input.len(),
            output.len()
        );
    }
}
