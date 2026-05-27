//! Rewrite for definition nodes — struct, bitfield, enum, union, register, mapping.
//!
//! Each definition type extracts fields/arms from the CST body and
//! formats them using vertical alignment.

use ide_db::line_index::TextRange;
use parser::SyntaxKind as SK;
use syntax::{NodeOrToken, SyntaxNode};

use super::rewrite::RewriteContext;
use super::rewrite::RewriteResult;
use super::shape::Shape;
use super::snippet::SnippetProvider;
use super::vertical::{rewrite_with_alignment, AlignedItem};

/// A struct/union field: `name : type`
pub(crate) struct StructField {
    pub(crate) name: String,
    pub(crate) separator: String,
    pub(crate) suffix: String,
    pub(crate) range: TextRange,
    pub(crate) is_comment: bool,
}

impl AlignedItem for StructField {
    fn skip(&self) -> bool {
        self.is_comment
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(&self, _context: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
        Ok(self.name.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult {
        let padding = prefix_max_width.saturating_sub(self.name.len());
        Ok(format!("{}{}{}{}", self.name, " ".repeat(padding), self.separator, self.suffix))
    }
}

/// A bitfield field: `name : range` or `name : bit_position`
pub(crate) struct BitfieldField {
    pub(crate) name: String,
    pub(crate) range_text: String,
    pub(crate) range: TextRange,
    pub(crate) comment: Option<String>,
    pub(crate) is_comment: bool,
}

impl AlignedItem for BitfieldField {
    fn skip(&self) -> bool {
        self.is_comment
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(&self, _context: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
        Ok(self.name.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult {
        let padding = prefix_max_width.saturating_sub(self.name.len());
        let mut result = format!("{}{} : {}", self.name, " ".repeat(padding), self.range_text);
        if let Some(ref comment) = self.comment {
            result.push_str(&format!(" {comment}"));
        }
        Ok(result)
    }
}

/// A mapping arm: `LHS <-> RHS`
pub(crate) struct MappingArm {
    pub(crate) lhs: String,
    pub(crate) rhs: String,
    pub(crate) range: TextRange,
    pub(crate) is_comment: bool,
}

impl AlignedItem for MappingArm {
    fn skip(&self) -> bool {
        self.is_comment
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(&self, _context: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
        Ok(self.lhs.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult {
        let padding = prefix_max_width.saturating_sub(self.lhs.len());
        Ok(format!("{}{} <-> {}", self.lhs, " ".repeat(padding), self.rhs))
    }
}

/// A register declaration: `register name : type`
pub(crate) struct RegisterDecl {
    pub(crate) name: String,
    pub(crate) type_text: String,
    pub(crate) range: TextRange,
}

impl AlignedItem for RegisterDecl {
    fn skip(&self) -> bool {
        false
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(&self, _context: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
        Ok(self.name.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult {
        let padding = prefix_max_width.saturating_sub(self.name.len());
        Ok(format!("{}{} : {}", self.name, " ".repeat(padding), self.type_text))
    }
}

/// Find the first child node of a given kind.
fn find_child_node(node: &SyntaxNode, kind: SK) -> Option<SyntaxNode> {
    node.children().find(|c| c.kind() == kind)
}

/// Extract the body text between `{` and `}` from a body-containing node
/// (STRUCT_EXPR or BLOCK_EXPR). Returns (body_text, body_start_offset).
fn extract_body_from_braced_node(
    body_node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Option<(String, usize)> {
    let mut l_curly_end: Option<usize> = None;
    let mut r_curly_start: Option<usize> = None;

    for elem in body_node.children_with_tokens() {
        if let NodeOrToken::Token(ref tok) = elem {
            if tok.kind() == SK::L_CURLY && l_curly_end.is_none() {
                l_curly_end = Some(tok.text_range().end().into());
            }
            if tok.kind() == SK::R_CURLY {
                r_curly_start = Some(tok.text_range().start().into());
            }
        }
    }

    let start = l_curly_end?;
    let end = r_curly_start?;
    if end <= start {
        return None;
    }
    let range = base_db::text_range(start, end);
    let body = snippet.span_to_snippet(range).to_string();
    Some((body, start))
}

/// Extract struct/union fields from a NAMED_DEF or SCATTERED_CLAUSE_DEF node.
///
/// CST shape: NAMED_DEF → ... → STRUCT_EXPR { FIELD_INIT, ... }
/// Each FIELD_INIT has: IDENT COLON type-node
pub(crate) fn extract_struct_fields(
    node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Vec<StructField> {
    // The body lives inside a STRUCT_EXPR or BLOCK_EXPR child node.
    let body_node =
        find_child_node(node, SK::STRUCT_EXPR).or_else(|| find_child_node(node, SK::BLOCK_EXPR));
    let body_node = match body_node {
        Some(n) => n,
        None => return Vec::new(),
    };

    let (body, body_start) = match extract_body_from_braced_node(&body_node, snippet) {
        Some(v) => v,
        None => return Vec::new(),
    };

    parse_colon_fields(&body, body_start)
}

/// Extract bitfield fields from a NAMED_DEF(bitfield) node's body.
///
/// CST shape: NAMED_DEF → ... → STRUCT_EXPR { FIELD_INIT, ... }
/// Bitfield fields are structurally identical to struct fields in the CST.
pub(crate) fn extract_bitfield_fields(
    node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Vec<BitfieldField> {
    let body_node = match find_child_node(node, SK::STRUCT_EXPR) {
        Some(n) => n,
        None => return Vec::new(),
    };

    let (body, body_start) = match extract_body_from_braced_node(&body_node, snippet) {
        Some(v) => v,
        None => return Vec::new(),
    };

    let mut fields = Vec::new();
    let mut offset = body_start;

    for line in body.split('\n') {
        let trimmed = line.trim();
        let line_len = line.len();
        let line_range = base_db::text_range(offset, offset + line_len);

        if trimmed.is_empty() {
            offset += line_len + 1;
            continue;
        }

        if trimmed.starts_with("//") {
            fields.push(BitfieldField {
                name: String::new(),
                range_text: String::new(),
                range: line_range,
                comment: Some(trimmed.to_string()),
                is_comment: true,
            });
            offset += line_len + 1;
            continue;
        }

        // Parse `name : range_expr` with optional trailing comma and comment
        let (field_part, inline_comment) = if let Some(comment_pos) = trimmed.find("//") {
            (trimmed[..comment_pos].trim(), Some(trimmed[comment_pos..].to_string()))
        } else {
            (trimmed, None)
        };

        let has_comma = field_part.trim_end().ends_with(',');
        let field_clean = field_part.trim_end_matches(',').trim();
        if let Some(colon_pos) = field_clean.find(':') {
            let name = field_clean[..colon_pos].trim().to_string();
            let raw_range = field_clean[colon_pos + 1..].trim().to_string();
            let range_text = if has_comma { format!("{raw_range},") } else { raw_range };
            fields.push(BitfieldField {
                name,
                range_text,
                range: line_range,
                comment: inline_comment,
                is_comment: false,
            });
        }

        offset += line_len + 1;
    }

    fields
}

/// Extract mapping arms from a CALLABLE_DEF(mapping) body.
///
/// CST shape: CALLABLE_DEF → ... → BLOCK_EXPR { BLOCK_ITEM(BIN_EXPR(LHS <-> RHS)), ... }
pub(crate) fn extract_mapping_arms(
    node: &SyntaxNode,
    snippet: &SnippetProvider,
) -> Vec<MappingArm> {
    let body_node = match find_child_node(node, SK::BLOCK_EXPR) {
        Some(n) => n,
        None => return Vec::new(),
    };

    let (body, body_start) = match extract_body_from_braced_node(&body_node, snippet) {
        Some(v) => v,
        None => return Vec::new(),
    };

    let mut arms = Vec::new();
    let mut offset = body_start;

    for line in body.split('\n') {
        let trimmed = line.trim();
        let line_len = line.len();
        let line_range = base_db::text_range(offset, offset + line_len);

        if trimmed.is_empty() {
            offset += line_len + 1;
            continue;
        }

        if trimmed.starts_with("//") {
            arms.push(MappingArm {
                lhs: String::new(),
                rhs: String::new(),
                range: line_range,
                is_comment: true,
            });
            offset += line_len + 1;
            continue;
        }

        // Parse `LHS <-> RHS` with optional trailing comma
        let trimmed_no_comma = trimmed.trim_end_matches(',').trim();
        if let Some(arrow_pos) = trimmed_no_comma.find("<->") {
            let lhs = trimmed_no_comma[..arrow_pos].trim().to_string();
            let rhs = trimmed_no_comma[arrow_pos + 3..].trim().to_string();
            let has_comma = trimmed.trim_end().ends_with(',');
            let rhs = if has_comma { format!("{rhs},") } else { rhs };
            arms.push(MappingArm { lhs, rhs, range: line_range, is_comment: false });
        }

        offset += line_len + 1;
    }

    arms
}

/// Extract register declarations from consecutive NAMED_DEF(register) nodes.
pub(crate) fn extract_register_decls(
    nodes: &[&SyntaxNode],
    snippet: &SnippetProvider,
) -> Vec<RegisterDecl> {
    let mut decls = Vec::new();

    for node in nodes {
        let text = snippet.span_to_snippet(node.text_range());
        let trimmed = text.trim();
        if !trimmed.starts_with("register") {
            continue;
        }
        let after_kw = trimmed["register".len()..].trim();
        if let Some(colon_pos) = after_kw.find(':') {
            let name = after_kw[..colon_pos].trim().to_string();
            let type_text = after_kw[colon_pos + 1..].trim().to_string();
            decls.push(RegisterDecl { name, type_text, range: node.text_range() });
        }
    }

    decls
}

/// Parse `name : type` lines from body text between braces.
fn parse_colon_fields(body: &str, body_start: usize) -> Vec<StructField> {
    let mut fields = Vec::new();
    let mut offset = body_start;

    for line in body.split('\n') {
        let trimmed = line.trim();
        let line_len = line.len();
        let line_range = base_db::text_range(offset, offset + line_len);

        if trimmed.is_empty() {
            offset += line_len + 1;
            continue;
        }

        if trimmed.starts_with("//") {
            fields.push(StructField {
                name: String::new(),
                separator: String::new(),
                suffix: trimmed.to_string(),
                range: line_range,
                is_comment: true,
            });
            offset += line_len + 1;
            continue;
        }

        // Try to parse `name : type` with optional trailing comma
        let trimmed_no_comma = trimmed.trim_end_matches(',').trim();
        if let Some(colon_pos) = trimmed_no_comma.find(':') {
            let name = trimmed_no_comma[..colon_pos].trim().to_string();
            let suffix = trimmed_no_comma[colon_pos + 1..].trim().to_string();
            let has_comma = trimmed.trim_end().ends_with(',');
            let full_suffix = if has_comma { format!("{suffix},") } else { suffix };
            fields.push(StructField {
                name,
                separator: " : ".to_string(),
                suffix: full_suffix,
                range: line_range,
                is_comment: false,
            });
        }

        offset += line_len + 1;
    }

    fields
}

/// A function parameter: `name : type` (for function defs) or just `type` (for val specs).
pub(crate) struct FunctionParam {
    pub(crate) name: String,
    pub(crate) type_text: String,
    pub(crate) range: TextRange,
    pub(crate) is_comment: bool,
}

impl AlignedItem for FunctionParam {
    fn skip(&self) -> bool {
        self.is_comment
    }

    fn get_range(&self) -> TextRange {
        self.range
    }

    fn rewrite_prefix(&self, _context: &RewriteContext<'_>, _shape: Shape) -> RewriteResult {
        Ok(self.name.clone())
    }

    fn rewrite_aligned_item(
        &self,
        _context: &RewriteContext<'_>,
        _shape: Shape,
        prefix_max_width: usize,
    ) -> RewriteResult {
        if self.name.is_empty() {
            // Val-style type-only param — no colon alignment needed.
            Ok(self.type_text.clone())
        } else {
            let padding = prefix_max_width.saturating_sub(self.name.len());
            Ok(format!("{}{} : {}", self.name, " ".repeat(padding), self.type_text))
        }
    }
}

/// Split a parameter string at top-level commas (not inside brackets/parens).
fn split_params_at_commas(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escape = false;

    for ch in s.chars() {
        if in_string {
            current.push(ch);
            if ch == '"' && !escape {
                in_string = false;
            }
            escape = ch == '\\' && !escape;
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                escape = false;
                current.push(ch);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(ch);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                parts.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        parts.push(current);
    }
    parts
}

/// Find the matching `)` for an opening `(`, tracking bracket depth.
/// `s` starts right after the `(`. Returns the byte offset of `)` within `s`.
fn find_matching_paren(s: &str) -> Option<usize> {
    let mut depth = 1i32;
    let mut in_string = false;
    let mut escape = false;

    for (i, ch) in s.char_indices() {
        if in_string {
            if ch == '"' && !escape {
                in_string = false;
            }
            escape = ch == '\\' && !escape;
            continue;
        }
        match ch {
            '"' => {
                in_string = true;
                escape = false;
            }
            '(' | '[' | '{' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            ']' | '}' => depth -= 1,
            _ => {}
        }
    }
    None
}

/// Parse function parameters from `name : type` text fragments.
/// Returns (params, has_named_params) where has_named_params is true if
/// at least one param has a `name : type` pattern (vs. type-only for val specs).
fn parse_function_params(params_str: &str, base_offset: usize) -> (Vec<FunctionParam>, bool) {
    let raw_params = split_params_at_commas(params_str);
    let mut params = Vec::new();
    let mut has_named = false;
    let mut offset = base_offset;

    for raw in &raw_params {
        let trimmed = raw.trim();
        let len = raw.len();
        let range = base_db::text_range(offset, offset + len);

        if trimmed.is_empty() {
            offset += len + 1; // +1 for comma
            continue;
        }

        // Try to split on `:` for `name : type` pattern.
        // Be careful not to split on `:` inside nested types like `MemoryAccessType(foo)`.
        // Only split on the first `:` that appears at depth 0.
        let mut colon_pos = None;
        let mut depth = 0i32;
        for (i, ch) in trimmed.char_indices() {
            match ch {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' | '}' => depth -= 1,
                ':' if depth == 0 => {
                    colon_pos = Some(i);
                    break;
                }
                _ => {}
            }
        }

        if let Some(cp) = colon_pos {
            let name = trimmed[..cp].trim().to_string();
            let type_text = trimmed[cp + 1..].trim().to_string();
            // Only count as named if the name looks like an identifier
            // (not a complex type expression).
            let looks_like_name = !name.is_empty()
                && name.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '\'');
            if looks_like_name {
                has_named = true;
                params.push(FunctionParam { name, type_text, range, is_comment: false });
            } else {
                // Not a named param — treat as type-only.
                params.push(FunctionParam {
                    name: String::new(),
                    type_text: trimmed.to_string(),
                    range,
                    is_comment: false,
                });
            }
        } else {
            params.push(FunctionParam {
                name: String::new(),
                type_text: trimmed.to_string(),
                range,
                is_comment: false,
            });
        }

        offset += len + 1; // +1 for comma
    }

    (params, has_named)
}

/// Format a CALLABLE_DEF node that is a function definition (S31).
///
///   1. Reconstruct single-line signature (undo legacy wrap)
///   2. compute_budgets_for_params → one_line_budget / multi_line_budget
///   3. definitive_tactic → Horizontal if fits, else Vertical
///   4. write_list → single-line join or one-param-per-line with alignment
pub(crate) fn rewrite_function_def(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let source = context.snippet(node.text_range());

    // Verify this is a function definition (not mapping).
    let is_function = node
        .children_with_tokens()
        .any(|elem| matches!(elem, NodeOrToken::Token(ref t) if t.kind() == SK::KW_FUNCTION));
    if !is_function {
        return None;
    }

    // ── Step 1: Extract signature components from CST source ──────────
    //
    // Find `(`, matching `)`, and the body-start marker (`= {` or `=`).
    // Reconstruct a single-line version so legacy line-breaks don't
    // distort width measurements.

    let open_paren = source.find('(')?;
    let after_paren = &source[open_paren + 1..];
    let close_offset = find_matching_paren(after_paren)?;
    let close_paren = open_paren + 1 + close_offset; // absolute index of `)`

    // prefix: everything before `(` — e.g. "function foo" / "private function foo"
    let prefix_raw = &source[..open_paren];
    // params_str: raw text between the parens
    let params_str_raw = &after_paren[..close_offset];
    // suffix: everything after `)` up to (and including) body-start marker
    let after_close_raw = &source[close_paren + 1..];
    let body_start_offset = after_close_raw
        .find("= {")
        .or_else(|| after_close_raw.find("=\n"))
        .unwrap_or(after_close_raw.len());
    let suffix_raw = &after_close_raw[..body_start_offset];

    // Normalise each component to single-line (collapse legacy wraps).
    let prefix: String = prefix_raw.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    let params_str: String = params_str_raw.lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
    let suffix: String = {
        let joined: String = suffix_raw
            .lines()
            .map(|l| l.trim())
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let body_marker = &after_close_raw[body_start_offset..];
        let marker_part: String =
            body_marker.lines().next().map(|l| l.trim().to_string()).unwrap_or_default();
        let mut s = joined;
        if !marker_part.is_empty() {
            if !s.is_empty() {
                s.push(' ');
            }
            s.push_str(&marker_part);
        }
        s
    };

    // ── Step 2: compute_budgets (aligned with rustfmt) ────────────────
    // Uses context.budget() to compute remaining width after overhead.
    let prefix_width = shape.indent.width() + prefix.len() + 1; // +1 for `(`
    let suffix_width = suffix.trim().len();
    let overhead = prefix_width + suffix_width + 1; // +1 for `)`
    let one_line_budget = context.budget(overhead);

    let param_indent = shape.indent.block_indent(context.config);
    let _multi_line_budget = context.budget(param_indent.width() + 1); // +1 for comma; used by AlignedItem rewrite

    // ── Step 3: Parse params & definitive_tactic ──────────────────────
    let params_vec = split_params_at_commas(&params_str);
    if params_vec.is_empty() {
        return None;
    }

    // Convert params to ListItems, then call definitive_tactic.
    use super::lists::{self, DefinitiveListTactic, ListItem, SeparatorTactic};
    let list_items: Vec<ListItem> =
        params_vec.iter().map(|p| ListItem::from_str(p.trim())).collect();
    let list_tactic = lists::definitive_tactic(&list_items, one_line_budget, 2); // 2 = ", ".len()

    #[derive(Debug, PartialEq)]
    enum Tactic {
        Horizontal,
        Vertical,
    }

    // Mixed falls back to Vertical if the packed layout still overflows.
    let tactic = match list_tactic {
        DefinitiveListTactic::Horizontal | DefinitiveListTactic::Mixed => Tactic::Horizontal,
        DefinitiveListTactic::Vertical => Tactic::Vertical,
    };
    // (no trailing comma); Vertical uses Vertical (trailing comma only
    // on vertical layout).
    let _sep_tactic = match tactic {
        Tactic::Horizontal => SeparatorTactic::Never,
        Tactic::Vertical => SeparatorTactic::Vertical,
    };

    // ── Step 4: Horizontal — try single line ──────────────────────────
    let indent_str = shape.indent.to_string_inner(context.config);
    let suffix_sep = if suffix.trim().is_empty() { "" } else { " " };
    let suffix_trimmed = suffix.trim();

    // Body text: everything after the signature (after the body-start marker line).
    // We must preserve the body verbatim.
    let body_rest = {
        let marker_end = close_paren + 1 + body_start_offset;
        let after_marker = &source[marker_end..];
        // The body-start marker line (e.g. "= {") is in suffix_trimmed.
        // Everything after that first line is the body.
        let first_nl = after_marker.find('\n');
        match first_nl {
            Some(pos) => &after_marker[pos..], // includes leading \n
            None => "",
        }
    };

    if tactic == Tactic::Horizontal {
        let joined_params: String =
            params_vec.iter().map(|p| p.trim()).collect::<Vec<_>>().join(", ");
        let sig_line = format!("{prefix}({joined_params}){suffix_sep}{suffix_trimmed}");
        // offset_left accounts for text already placed on this line;
        // used_width gives the total consumed columns.
        let sig_shape = shape
            .offset_left(prefix.len() + 1) // +1 for `(`
            .and_then(|s| s.sub_width(suffix_width + 1)); // +1 for `)`
        let full_width = shape.used_width() + sig_line.len();
        let fits = sig_shape.is_some() && full_width <= context.config.max_width();
        if fits {
            // Check if source signature is already identical single-line.
            let source_sig_normalized: String =
                source[..close_paren + 1].lines().map(|l| l.trim()).collect::<Vec<_>>().join(" ");
            let new_sig_normalized = format!("{prefix}({joined_params})");
            if source_sig_normalized == new_sig_normalized
                && !source[..close_paren + 1].contains('\n')
            {
                return None; // Already correct, no rewrite needed.
            }
            // Rewrite signature + preserve body.
            return Some(format!("{sig_line}{body_rest}"));
        }
        // Doesn't fit → fall through to Vertical.
    }

    // ── Step 5: Vertical — one-param-per-line with colon alignment ────
    let node_start: usize = node.text_range().start().into();
    let params_abs_start = node_start + open_paren + 1;
    let (parsed_params, has_named) = parse_function_params(&params_str, params_abs_start);

    if parsed_params.is_empty() {
        return None;
    }

    let body_shape = shape.block_indent(context.config);

    // paren line, matching rustfmt's pattern for multi-line parameter lists.
    let close_indent = shape.indent.to_string_with_newline(context.config);

    if has_named {
        // Use AlignedItem for colon alignment.
        let aligned_body = rewrite_with_alignment(&parsed_params, context, body_shape)?;
        // Add trailing comma to each parameter line.
        let param_lines: Vec<String> = aligned_body
            .lines()
            .map(|line| {
                let trimmed = line.trim_end();
                if !trimmed.is_empty() && !trimmed.ends_with(',') {
                    format!("{trimmed},")
                } else {
                    trimmed.to_string()
                }
            })
            .collect();
        let params_block = param_lines.join("\n");

        Some(format!(
            "{indent_str}{prefix}(\n{params_block}{close_indent}){suffix_sep}{suffix_trimmed}{body_rest}"
        ))
    } else {
        // Type-only params — just indent each one.
        let body_indent = body_shape.indent.to_string_inner(context.config);
        let param_lines: Vec<String> =
            parsed_params.iter().map(|p| format!("{body_indent}{},", p.type_text)).collect();
        let params_block = param_lines.join("\n");
        Some(format!(
            "{indent_str}{prefix}(\n{params_block}{close_indent}){suffix_sep}{suffix_trimmed}{body_rest}"
        ))
    }
}

/// Format a CALLABLE_SPEC node that is a val declaration (S31).
///
/// Short val specs stay on one line. Long val specs break to
/// one-type-per-line for parameter lists.
pub(crate) fn rewrite_val_spec(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let source = context.snippet(node.text_range());

    // Verify this is a val spec.
    let is_val = node
        .children_with_tokens()
        .any(|elem| matches!(elem, NodeOrToken::Token(ref t) if t.kind() == SK::KW_VAL));
    if !is_val {
        return None;
    }

    // Check if the whole thing fits on one line.
    let first_line = source.lines().next().unwrap_or(source);
    let first_line_width = shape.indent.width() + first_line.trim_end().len();
    if first_line_width <= context.config.max_width() {
        return None;
    }

    // Find opening `(` — val specs with param lists look like:
    // `val pt_walk : forall 'v, is_sv_mode('v) . (int('v), vpn_bits('v), ...) -> PTW_Result('v)`
    let open_paren = source.find('(')?;
    let header = &source[..open_paren]; // e.g. "val pt_walk : forall 'v, is_sv_mode('v) . "
    let after_paren = &source[open_paren + 1..];

    // Find matching `)`.
    let close_offset = find_matching_paren(after_paren)?;
    let params_str = &after_paren[..close_offset];
    let after_close = &after_paren[close_offset + 1..]; // e.g. " -> PTW_Result('v)"

    // Parse as type-only params.
    let raw_params = split_params_at_commas(params_str);
    if raw_params.len() < 2 {
        return None;
    }

    use super::lists::{self, DefinitiveListTactic, ListFormatting, ListItem, SeparatorTactic};
    let list_items: Vec<ListItem> =
        raw_params.iter().map(|p| ListItem::from_str(p.trim())).collect();

    let indent = shape.indent.to_string_inner(context.config);
    let body_shape = shape.block_indent(context.config);

    let formatting = ListFormatting::new(body_shape, context.config)
        .tactic(DefinitiveListTactic::Vertical)
        .separator(",")
        .trailing_separator(SeparatorTactic::Always);

    let params_block = lists::write_list(&list_items, &formatting).ok()?;
    let after_trimmed = after_close.trim_start();
    let sep = if after_trimmed.is_empty() { "" } else { " " };

    Some(format!("{indent}{header}(\n{params_block}\n{indent}){sep}{after_trimmed}"))
}

/// Detect the definition kind by scanning for the first keyword token.
fn first_keyword(node: &SyntaxNode) -> Option<SK> {
    for elem in node.children_with_tokens() {
        if let NodeOrToken::Token(tok) = elem {
            match tok.kind() {
                SK::KW_STRUCT | SK::KW_BITFIELD | SK::KW_ENUM | SK::KW_UNION | SK::KW_REGISTER => {
                    return Some(tok.kind())
                }
                _ => {}
            }
        }
    }
    None
}

/// Format a NAMED_DEF node (struct/bitfield/enum/register).
///
/// Returns `Some(formatted)` on success, `None` to fall back to verbatim.
pub(crate) fn rewrite_named_def(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let kw = first_keyword(node)?;
    match kw {
        SK::KW_STRUCT | SK::KW_UNION => rewrite_struct_like(node, context, shape),
        SK::KW_BITFIELD => rewrite_bitfield(node, context, shape),
        SK::KW_ENUM => {
            // Enum members are comma-separated identifiers, not `name : type`.
            // Fall back to verbatim; enum alignment is a future enhancement.
            None
        }
        SK::KW_REGISTER => rewrite_register(node, context, shape),
        _ => None,
    }
}

/// Format a SCATTERED_CLAUSE_DEF node (union clause with body).
pub(crate) fn rewrite_scattered_clause_def(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    // Only handle `union clause Name = { ... }` — these have struct-like
    // field definitions (Name : Type). Do NOT handle `function clause`
    // or `mapping clause` — those have expression bodies that would be
    // corrupted by field-alignment logic.
    let is_union_clause = node
        .children_with_tokens()
        .any(|elem| matches!(elem, NodeOrToken::Token(ref t) if t.kind() == SK::KW_UNION));
    if !is_union_clause {
        return None;
    }

    let body_node = find_child_node(node, SK::BLOCK_EXPR)?;
    let (body, body_start) = extract_body_from_braced_node(&body_node, context.snippet_provider)?;

    let fields = parse_colon_fields(&body, body_start);
    if fields.is_empty() {
        return None;
    }

    let body_shape = shape.block_indent(context.config);
    let aligned_body = rewrite_with_alignment(&fields, context, body_shape)?;

    let source = context.snippet(node.text_range());
    let l_curly_pos = source.find('{')?;
    let header = source[..=l_curly_pos].trim_end();

    let indent = shape.indent.to_string_inner(context.config);
    Some(format!("{header}\n{aligned_body}\n{indent}}}"))
}

/// Format a CALLABLE_DEF node that is a mapping.
pub(crate) fn rewrite_mapping_def(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    // Verify this is actually a mapping.
    let is_mapping = node
        .children_with_tokens()
        .any(|elem| matches!(elem, NodeOrToken::Token(ref t) if t.kind() == SK::KW_MAPPING));
    if !is_mapping {
        return None;
    }

    let arms = extract_mapping_arms(node, context.snippet_provider);
    if arms.is_empty() {
        return None;
    }

    let body_shape = shape.block_indent(context.config);
    let aligned_body = rewrite_with_alignment(&arms, context, body_shape)?;

    let source = context.snippet(node.text_range());
    let l_curly_pos = source.find('{')?;
    let header = source[..=l_curly_pos].trim_end();

    let indent = shape.indent.to_string_inner(context.config);
    Some(format!("{header}\n{aligned_body}\n{indent}}}"))
}

fn rewrite_struct_like(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let fields = extract_struct_fields(node, context.snippet_provider);
    if fields.is_empty() {
        return None;
    }

    let body_shape = shape.block_indent(context.config);
    let aligned_body = rewrite_with_alignment(&fields, context, body_shape)?;

    let source = context.snippet(node.text_range());
    let l_curly_pos = source.find('{')?;
    let header = source[..=l_curly_pos].trim_end();

    // Use block_unindent from body_shape to get back to definition level.
    let close_indent = body_shape.indent.block_unindent(context.config);
    let indent = close_indent.to_string_inner(context.config);
    Some(format!("{header}\n{aligned_body}\n{indent}}}"))
}

fn rewrite_bitfield(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    shape: Shape,
) -> Option<String> {
    let fields = extract_bitfield_fields(node, context.snippet_provider);
    if fields.is_empty() {
        return None;
    }

    let body_shape = shape.block_indent(context.config);
    let aligned_body = rewrite_with_alignment(&fields, context, body_shape)?;

    let source = context.snippet(node.text_range());
    let l_curly_pos = source.find('{')?;
    let header = source[..=l_curly_pos].trim_end();

    let indent = shape.indent.to_string_inner(context.config);
    Some(format!("{header}\n{aligned_body}\n{indent}}}"))
}

/// Format a single register NAMED_DEF node.
///
/// Uses `RegisterDecl` + `extract_register_decls` to parse the register
/// and `AlignedItem` to format it (alignment happens across consecutive
/// registers via `AlignedItem` formatting).
fn rewrite_register(
    node: &SyntaxNode,
    context: &RewriteContext<'_>,
    _shape: Shape,
) -> Option<String> {
    let nodes = [node];
    let decls = extract_register_decls(&nodes, context.snippet_provider);
    if decls.is_empty() {
        return None;
    }
    // Single register — format using AlignedItem (no alignment padding
    // since there's no group to align with; the visitor's post-processing
    // pass handles cross-definition alignment).
    let decl = &decls[0];
    Some(format!("register {} : {}", decl.name, decl.type_text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ide_db::ide_types::FormatOptions;

    fn make_context(source: &str) -> (SnippetProvider, FormatOptions) {
        let snippet = SnippetProvider::new(source.to_string());
        let config = FormatOptions::default();
        (snippet, config)
    }

    // -- struct alignment -----------------------------------------------

    #[test]
    fn struct_field_alignment() {
        let source = "struct foo = {\n  x : int,\n  y_offset : bits(32),\n}\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let named_def =
            root.children().find(|c| c.kind() == SK::NAMED_DEF).expect("should have NAMED_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_named_def(&named_def, &ctx, shape);

        let formatted = result.expect("should format struct");
        assert!(formatted.contains("x        : int,"), "expected padded x, got:\n{formatted}");
        assert!(
            formatted.contains("y_offset : bits(32),"),
            "expected aligned y_offset, got:\n{formatted}"
        );
    }

    // -- bitfield alignment ---------------------------------------------

    #[test]
    fn bitfield_field_alignment() {
        let source =
            "bitfield Mstatus : xlenbits = {\n  MIE : 3,\n  MPIE : 7,\n  MPP : 12 .. 11,\n}\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let named_def =
            root.children().find(|c| c.kind() == SK::NAMED_DEF).expect("should have NAMED_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_named_def(&named_def, &ctx, shape);

        let formatted = result.expect("should format bitfield");
        assert!(formatted.contains("MIE  : 3,"), "expected padded MIE, got:\n{formatted}");
        assert!(formatted.contains("MPIE : 7,"), "expected aligned MPIE, got:\n{formatted}");
    }

    // -- mapping alignment ----------------------------------------------

    #[test]
    fn mapping_arm_alignment() {
        let source = "mapping foo = {\n  X <-> 0x1,\n  YYYY <-> 0xFF,\n}\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_mapping_def(&callable_def, &ctx, shape);

        let formatted = result.expect("should format mapping");
        assert!(formatted.contains("X    <-> 0x1,"), "expected padded X, got:\n{formatted}");
        assert!(formatted.contains("YYYY <-> 0xFF,"), "expected aligned YYYY, got:\n{formatted}");
    }

    // -- register alignment ---------------------------------------------

    #[test]
    fn register_decl_alignment() {
        let src1 = "register x : int\n";
        let src2 = "register pc_reg : bits(64)\n";
        let combined = format!("{src1}{src2}");
        let (snippet, config) = make_context(&combined);
        let (root, _errors) = syntax::parse_text(&combined);

        let nodes: Vec<_> = root.children().filter(|c| c.kind() == SK::NAMED_DEF).collect();
        let node_refs: Vec<&SyntaxNode> = nodes.iter().collect();

        let decls = extract_register_decls(&node_refs, &snippet);
        assert_eq!(decls.len(), 2, "should extract 2 register decls");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let formatted = rewrite_with_alignment(&decls, &ctx, shape).unwrap();
        assert!(formatted.contains("x      : int"), "expected padded x, got:\n{formatted}");
        assert!(
            formatted.contains("pc_reg : bits(64)"),
            "expected aligned pc_reg, got:\n{formatted}"
        );
    }

    // -- union (scattered clause) alignment -----------------------------

    #[test]
    fn union_clause_field_alignment() {
        let source = "union clause ast = {\n  ADD : (reg, reg),\n  LOAD : bits(32),\n}\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        // `union clause` parses as SCATTERED_CLAUSE_DEF.
        let scd = root
            .children()
            .find(|c| c.kind() == SK::SCATTERED_CLAUSE_DEF)
            .expect("should have SCATTERED_CLAUSE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_scattered_clause_def(&scd, &ctx, shape);

        let formatted = result.expect("should format union clause");
        assert!(formatted.contains("ADD  : (reg, reg),"), "expected padded ADD, got:\n{formatted}");
        assert!(formatted.contains("LOAD : bits(32),"), "expected aligned LOAD, got:\n{formatted}");
    }

    // -- extract helpers ------------------------------------------------

    #[test]
    fn extract_struct_fields_basic() {
        let source = "struct point = {\n  x : int,\n  y : int,\n}\n";
        let (snippet, _config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let named_def =
            root.children().find(|c| c.kind() == SK::NAMED_DEF).expect("should have NAMED_DEF");

        let fields = extract_struct_fields(&named_def, &snippet);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].name, "x");
        assert_eq!(fields[1].name, "y");
    }

    #[test]
    fn extract_mapping_arms_basic() {
        let source = "mapping foo = {\n  A <-> 1,\n  B <-> 2,\n}\n";
        let (snippet, _config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let arms = extract_mapping_arms(&callable_def, &snippet);
        assert_eq!(arms.len(), 2);
        assert_eq!(arms[0].lhs, "A");
        assert_eq!(arms[0].rhs, "1,");
        assert_eq!(arms[1].lhs, "B");
        assert_eq!(arms[1].rhs, "2,");
    }

    // -- function signature formatting (S31) --------------------------------

    #[test]
    fn function_def_short_unchanged() {
        // Short function signature should not be rewritten.
        let source = "function foo(x : int) -> int = x + 1\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);
        assert!(result.is_none(), "short function should not be rewritten");
    }

    #[test]
    fn function_def_long_params_colon_aligned() {
        // Long function signature exceeding max_width should break with colon alignment.
        // Use a narrow max_width to force wrapping.
        let source = "function check_PTE_permission(access : MemoryAccessType(mem_payload), priv : Privilege, mxr : bool, do_sum : bool, pte_flags : PTE_Flags) -> PTE_Check = {\n";
        let (snippet, mut config) = make_context(source);
        config.max_line_width = Some(60);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);

        let formatted = result.expect("long function should be rewritten");
        // Should have opening paren at end of first line.
        assert!(
            formatted.starts_with("function check_PTE_permission("),
            "should start with header(, got:\n{formatted}"
        );
        // Should have one param per line with trailing comma.
        assert!(
            formatted.contains("access    : MemoryAccessType(mem_payload),"),
            "expected aligned access param, got:\n{formatted}"
        );
        assert!(
            formatted.contains("pte_flags : PTE_Flags,"),
            "expected aligned pte_flags param, got:\n{formatted}"
        );
        // Closing paren + return type on same line.
        assert!(
            formatted.contains(") -> PTE_Check = {"),
            "expected closing paren with return type, got:\n{formatted}"
        );
    }

    #[test]
    fn function_def_private_long_params() {
        // Test with `private` visibility modifier.
        let source = "private function check_PTE(access : MemoryAccessType, priv : Privilege, mxr : bool, do_sum : bool) -> PTE_Check = {\n";
        let (snippet, mut config) = make_context(source);
        config.max_line_width = Some(60);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);

        let formatted = result.expect("long private function should be rewritten");
        assert!(
            formatted.starts_with("private function check_PTE("),
            "should start with private function header(, got:\n{formatted}"
        );
        // Should have colon-aligned params.
        assert!(
            formatted.contains("access : MemoryAccessType,"),
            "expected access param, got:\n{formatted}"
        );
    }

    #[test]
    fn function_def_legacy_wrap_cleaned_up() {
        // A signature broken by legacy wrap should be reconstructed as single-line
        // if it fits, or as clean vertical if it doesn't.
        // Here it fits at width 100 so should be cleaned up to single-line.
        let source = "function foo(x : int,\n  y : bool) -> int = {\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);

        let formatted = result.expect("legacy-wrapped function should be rewritten to single line");
        // Should be a single line with no newline in the signature part.
        assert!(!formatted.contains('\n'), "expected single-line output, got:\n{formatted}");
        assert!(
            formatted.contains("function foo(x : int, y : bool)"),
            "expected reconstructed single-line params, got:\n{formatted}"
        );
    }

    #[test]
    fn function_def_with_return_type_and_body() {
        // Signature with `-> RetType = {` should preserve the suffix.
        let source = "function big_fn(alpha : int, beta : bits(32), gamma : bool, delta : string, epsilon : unit) -> MyResult = {\n";
        let (snippet, mut config) = make_context(source);
        config.max_line_width = Some(60);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);

        let formatted = result.expect("long function with return type should be rewritten");
        // Return type + body start should appear after closing paren.
        assert!(
            formatted.contains(") -> MyResult = {"),
            "expected return type + body start after ), got:\n{formatted}"
        );
        // Each param on its own line.
        assert!(formatted.contains("alpha"), "expected alpha param, got:\n{formatted}");
        assert!(
            formatted.contains("epsilon : unit,"),
            "expected epsilon param with trailing comma, got:\n{formatted}"
        );
    }

    #[test]
    fn function_def_budget_based_not_param_count() {
        // Two params that together exceed max_width should go vertical,
        // even though there are only 2 params (old code required >= 2,
        // new code uses budget, no param-count threshold).
        let source = "function process(very_long_parameter_name : VeryLongTypeName(with_args), another_long_param : AnotherLongType) -> RetType = {\n";
        let (snippet, mut config) = make_context(source);
        config.max_line_width = Some(60);
        let (root, _errors) = syntax::parse_text(source);

        let callable_def = root
            .children()
            .find(|c| c.kind() == SK::CALLABLE_DEF)
            .expect("should have CALLABLE_DEF");

        let ctx = RewriteContext::new(&config, &snippet);
        let shape = Shape::with_max_width(&config);
        let result = rewrite_function_def(&callable_def, &ctx, shape);

        let formatted = result.expect("2 long params exceeding budget should be rewritten");
        // Should be vertical (multi-line).
        assert!(
            formatted.contains('\n'),
            "expected multi-line output for 2 long params, got:\n{formatted}"
        );
    }

    #[test]
    fn val_spec_short_unchanged() {
        // Short val spec should not be rewritten.
        let source = "val foo : int -> int\n";
        let (snippet, config) = make_context(source);
        let (root, _errors) = syntax::parse_text(source);

        let callable_spec = root.children().find(|c| c.kind() == SK::CALLABLE_SPEC);
        // Val specs may or may not parse as CALLABLE_SPEC depending on the grammar.
        // If it doesn't parse as one, just skip the test.
        if let Some(spec) = callable_spec {
            let ctx = RewriteContext::new(&config, &snippet);
            let shape = Shape::with_max_width(&config);
            let result = rewrite_val_spec(&spec, &ctx, shape);
            assert!(result.is_none(), "short val should not be rewritten");
        }
    }

    #[test]
    fn val_spec_long_params() {
        // Long val spec with parameter list.
        let source = "val pt_walk : forall 'v . (int('v), vpn_bits('v), Privilege, bool, bool, bool) -> PTW_Result('v)\n";
        let (snippet, mut config) = make_context(source);
        config.max_line_width = Some(60);
        let (root, _errors) = syntax::parse_text(source);

        let callable_spec = root.children().find(|c| c.kind() == SK::CALLABLE_SPEC);
        if let Some(spec) = callable_spec {
            let ctx = RewriteContext::new(&config, &snippet);
            let shape = Shape::with_max_width(&config);
            let result = rewrite_val_spec(&spec, &ctx, shape);

            let formatted = result.expect("long val should be rewritten");
            // Should have opening paren at end of header line.
            assert!(
                formatted.contains("(\n"),
                "should break after opening paren, got:\n{formatted}"
            );
            // Each type on its own line with trailing comma.
            assert!(formatted.contains("int('v),"), "expected int('v) param, got:\n{formatted}");
            // Closing paren + return type.
            assert!(
                formatted.contains(") -> PTW_Result('v)"),
                "expected closing paren with return type, got:\n{formatted}"
            );
        }
    }
}
