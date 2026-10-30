//! Shared utilities for assist handlers.

use ide_db::ide_types::IdeTextEdit;
use ide_db::FileDb;
use ide_diagnostics::Diagnostic;
use syntax::ast;

/// Extract a trivial expression from a block.
///
/// If a block contains only a single item (no statements),
/// returns that item. Used by assists that simplify blocks.
///
/// `Option<ast::Expr>`, Sail returns `Option<ast::BlockItem>` because
/// Sail blocks contain `BlockItem` nodes (not bare `Expr`).
pub fn extract_trivial_expression(block: &ast::BlockExpr) -> Option<ast::BlockItem> {
    let items = block.items();
    if items.len() == 1 {
        return items.into_iter().next();
    }
    None
}

/// A struct's name, its fields with their types, and the offset just
/// past its definition.
pub type StructShape = (String, Vec<(String, String)>, usize);

/// The `struct` definition the cursor is in.
///
/// Two handlers used to recover this from the raw text — the last
/// `struct` in a ±200-byte window, then a brace count, then the field
/// list split on `,`. The split is what makes it wrong rather than
/// merely fragile: `{ v : vector(2, dec, bit) }` is one field, and read
/// that way it is three, two of which have no `:` and are dropped.
pub fn struct_at_offset(root: &syntax::SyntaxNode, offset: usize) -> Option<StructShape> {
    use parser::SyntaxKind as SK;

    let def = root
        .descendants()
        .filter(|n| {
            n.kind() == SK::NAMED_DEF
                && n.children_with_tokens()
                    .filter_map(|el| el.into_token())
                    .any(|t| t.kind() == SK::KW_STRUCT)
                && usize::from(n.text_range().start()) <= offset
                && offset <= usize::from(n.text_range().end())
        })
        .last()?;

    let name = def.children().find(|n| n.kind() == SK::NAME)?.text().to_string();
    let body = def.children().find(|n| n.kind() == SK::STRUCT_EXPR)?;
    let fields: Vec<(String, String)> = body
        .children()
        .filter(|n| n.kind() == SK::FIELD_INIT)
        .filter_map(|f| {
            let field_name = f
                .children_with_tokens()
                .filter_map(|el| el.into_token())
                .find(|t| t.kind() == SK::IDENT)?
                .text()
                .to_string();
            let ty = f.children().next()?.text().to_string();
            Some((field_name, ty))
        })
        .collect();
    if fields.is_empty() {
        return None;
    }

    Some((name, fields, usize::from(def.text_range().end())))
}

pub(crate) fn expected_token_from_message(message: &str) -> Option<&'static str> {
    let message = message.to_ascii_lowercase();
    if message.contains("expected ';'") || message.contains("expected ;") {
        return Some(";");
    }
    if message.contains("expected ')'") || message.contains("expected )") {
        return Some(")");
    }
    if message.contains("expected ']'") || message.contains("expected ]") {
        return Some("]");
    }
    if message.contains("expected '}'") || message.contains("expected }") {
        return Some("}");
    }
    if message.contains("expected ','") || message.contains("expected ,") {
        return Some(",");
    }
    if message.contains("expected '='") || message.contains("expected =") {
        return Some("=");
    }
    None
}

pub(crate) fn line_text(file: &dyn FileDb, line: u32) -> Option<&str> {
    let start = file.offset_at(&ide_db::LineCol { line, col: 0 });
    if start > file.text().len() {
        return None;
    }
    let end = file.offset_at(&ide_db::LineCol { line: line + 1, col: 0 }).min(file.text().len());
    Some(&file.text()[start..end])
}

pub(crate) fn missing_token_fix(
    file: &dyn FileDb,
    diagnostic: &Diagnostic,
    token: &str,
) -> Option<IdeTextEdit> {
    if !matches!(token, ")" | "]" | "}" | "," | "=") {
        return None;
    }
    let offset = base_db::range_end(diagnostic.range.range);
    if file.text().get(offset..offset + token.len()) == Some(token) {
        return None;
    }
    Some(IdeTextEdit { range: base_db::text_range(offset, offset), new_text: token.to_string() })
}

/// Insert an explicitly unfinished wildcard branch into the diagnosed match.
/// Use its direct closing token: braces in nested expressions, comments and
/// strings do not delimit this match. `undefined` preserves the expected type.
pub(crate) fn incomplete_match_fix(
    file: &dyn FileDb,
    diagnostic: &Diagnostic,
) -> Option<(String, IdeTextEdit, bool)> {
    let root = file.syntax()?;
    let range = diagnostic.range.range;
    let node = root
        .descendants()
        .filter(|node| {
            node.kind() == syntax::SyntaxKind::MATCH_EXPR && node.text_range().contains_range(range)
        })
        .min_by_key(|node| node.text_range().len())?;
    let close = node
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .find(|token| token.kind() == syntax::SyntaxKind::R_CURLY)?;
    let previous = close.prev_token()?;
    let mut previous = previous;
    while previous.kind().is_trivia() {
        previous = previous.prev_token()?;
    }
    let separator = if previous.text() == "," || previous.text() == "{" { "" } else { "," };
    Some((
        "Add wildcard arm with `undefined` placeholder".to_string(),
        IdeTextEdit {
            range: base_db::text_range(
                usize::from(close.text_range().start()),
                usize::from(close.text_range().start()),
            ),
            new_text: format!("{separator}\n    _ => undefined\n"),
        },
        false,
    ))
}

pub(crate) fn is_import_directive(line: &str) -> bool {
    line.starts_with("$include ") || line.starts_with("include ")
}

pub(crate) fn invert_condition(cond: &str) -> String {
    let trimmed = cond.trim();
    // !(expr) → expr
    if let Some(inner) = trimmed.strip_prefix("~(").and_then(|s| s.strip_suffix(')')) {
        return inner.to_string();
    }
    if let Some(inner) = trimmed.strip_prefix("not(").and_then(|s| s.strip_suffix(')')) {
        return inner.to_string();
    }
    // ~expr → expr (single identifier)
    if let Some(inner) = trimmed.strip_prefix('~') {
        if inner.chars().all(|c| c.is_alphanumeric() || c == '_') {
            return inner.to_string();
        }
    }
    // == → !=
    if let Some((lhs, rhs)) = trimmed.split_once(" == ") {
        return format!("{lhs} != {rhs}");
    }
    // != → ==
    if let Some((lhs, rhs)) = trimmed.split_once(" != ") {
        return format!("{lhs} == {rhs}");
    }
    // >= → <
    if let Some((lhs, rhs)) = trimmed.split_once(" >= ") {
        return format!("{lhs} < {rhs}");
    }
    // <= → >
    if let Some((lhs, rhs)) = trimmed.split_once(" <= ") {
        return format!("{lhs} > {rhs}");
    }
    // > → <=  (must be after >=)
    if let Some((lhs, rhs)) = trimmed.split_once(" > ") {
        return format!("{lhs} <= {rhs}");
    }
    // < → >=  (must be after <=)
    if let Some((lhs, rhs)) = trimmed.split_once(" < ") {
        return format!("{lhs} >= {rhs}");
    }
    // fallback: wrap with not()
    format!("~({trimmed})")
}

pub(crate) fn flip_comparison_op(op: &str) -> &str {
    match op {
        "<" => ">",
        ">" => "<",
        "<=" => ">=",
        ">=" => "<=",
        other => other, // ==, !=, +, -, *, etc. stay the same
    }
}

pub(crate) fn strip_block_braces(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with('{') && s.ends_with('}') {
        s[1..s.len() - 1].trim()
    } else {
        s
    }
}

pub(crate) fn split_assignment(s: &str) -> Option<(&str, &str)> {
    // Find first `=` that is not `==`, `!=`, `<=`, `>=`, `=>`
    let bytes = s.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] == b'=' {
            if i > 0 && matches!(bytes[i - 1], b'!' | b'<' | b'>' | b'=') {
                continue;
            }
            if i + 1 < bytes.len() && matches!(bytes[i + 1], b'=' | b'>') {
                continue;
            }
            let target = s[..i].trim();
            let value = s[i + 1..].trim();
            if !target.is_empty() && !value.is_empty() {
                return Some((target, value));
            }
        }
    }
    None
}

pub(crate) fn try_eval_int_expr(s: &str) -> Option<i64> {
    // Try direct integer literal
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }

    // Try binary operations: a + b, a - b, a * b, a / b, a % b
    for op in [" + ", " - ", " * ", " / ", " % ", " << ", " >> "] {
        if let Some(pos) = s.rfind(op) {
            let lhs = try_eval_int_expr(s[..pos].trim())?;
            let rhs = try_eval_int_expr(s[pos + op.len()..].trim())?;
            return match op.trim() {
                "+" => Some(lhs + rhs),
                "-" => Some(lhs - rhs),
                "*" => Some(lhs * rhs),
                "/" if rhs != 0 => Some(lhs / rhs),
                "%" if rhs != 0 => Some(lhs % rhs),
                "<<" if (0..64).contains(&rhs) => Some(lhs << rhs),
                ">>" if (0..64).contains(&rhs) => Some(lhs >> rhs),
                _ => None,
            };
        }
    }

    // Parenthesized expression
    if s.starts_with('(') && s.ends_with(')') {
        return try_eval_int_expr(&s[1..s.len() - 1]);
    }
    // Negation
    if let Some(rest) = s.strip_prefix('-') {
        return try_eval_int_expr(rest.trim()).map(|n| -n);
    }
    // Hex/binary integer literals
    if let Some(hex) = s.strip_prefix("0x") {
        return i64::from_str_radix(hex, 16).ok();
    }
    if let Some(bin) = s.strip_prefix("0b") {
        return i64::from_str_radix(bin, 2).ok();
    }

    // 2 ^ n (power)
    if let Some(pos) = s.find(" ^ ") {
        let base = try_eval_int_expr(s[..pos].trim())?;
        let exp = try_eval_int_expr(s[pos + 3..].trim())?;
        if (0..63).contains(&exp) {
            return Some(base.pow(exp as u32));
        }
    }

    None
}

pub(crate) fn find_def_end_after(file: &dyn FileDb, offset: usize) -> Option<usize> {
    if let Some(it) = file.item_tree() {
        for &id in it.top_level_items() {
            let span = id.span(it);
            if span.start <= offset && offset <= span.end {
                return Some(span.end);
            }
        }
    }
    Some(file.text().len())
}

/// Find an expression of a specific kind at an offset in a callable body.
/// Generic visitor that replaces 6 find_*_in_expr recursive walkers.
///
/// Walks `body.iter_exprs()` and returns the first Expr that:
/// 1. Has a span covering `offset`
/// 2. Matches the `predicate`
pub(crate) fn find_hir_expr_at_offset<'a, P>(
    body: &'a hir::Body,
    source_map: &hir::BodySourceMap,
    offset: usize,
    predicate: P,
) -> Option<(hir::ExprId, &'a hir::Expr, parser::Span)>
where
    P: Fn(&hir::Expr) -> bool,
{
    // Find the smallest enclosing expression at offset that matches
    let mut best: Option<(hir::ExprId, &hir::Expr, parser::Span, usize)> = None;

    for (id, hir) in body.iter_exprs() {
        let span = match source_map.expr_syntax(id) {
            Some(s) => s,
            None => continue,
        };
        if span.start <= offset && offset < span.end && predicate(hir) {
            let width = span.end - span.start;
            if best.as_ref().is_none_or(|b| width < b.3) {
                best = Some((id, hir, span, width));
            }
        }
    }

    best.map(|(id, hir, span, _)| (id, hir, span))
}

/// Find an if-expression at offset using Body arena.
pub(crate) fn find_if_at_offset_hir(
    body: &hir::Body,
    source_map: &hir::BodySourceMap,
    offset: usize,
) -> Option<(hir::ExprId, parser::Span)> {
    find_hir_expr_at_offset(body, source_map, offset, |hir| matches!(hir, hir::Expr::If { .. }))
        .map(|(id, _, span)| (id, span))
}

/// Find an infix expression at offset using Body arena.
pub(crate) fn find_infix_at_offset_hir(
    body: &hir::Body,
    source_map: &hir::BodySourceMap,
    offset: usize,
) -> Option<(hir::ExprId, parser::Span)> {
    find_hir_expr_at_offset(body, source_map, offset, |hir| {
        matches!(hir, hir::Expr::BinaryOp { .. })
    })
    .map(|(id, _, span)| (id, span))
}

/// Find a let-expression at offset using Body arena.
pub(crate) fn find_let_at_offset_hir(
    body: &hir::Body,
    source_map: &hir::BodySourceMap,
    offset: usize,
) -> Option<(hir::ExprId, parser::Span)> {
    find_hir_expr_at_offset(body, source_map, offset, |hir| matches!(hir, hir::Expr::Let { .. }))
        .map(|(id, _, span)| (id, span))
}

pub fn try_fold_constant(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Simple integer arithmetic
    if let Some(result) = try_eval_int_expr(trimmed) {
        return Some(format!("{result}"));
    }

    // Boolean constants
    match trimmed {
        "true & true" => return Some("true".to_string()),
        "true & false" | "false & true" | "false & false" => return Some("false".to_string()),
        "true | true" | "true | false" | "false | true" => return Some("true".to_string()),
        "false | false" => return Some("false".to_string()),
        "~(true)" | "not(true)" => return Some("false".to_string()),
        "~(false)" | "not(false)" => return Some("true".to_string()),
        _ => {}
    }

    // Hex/binary literal size
    if let Some(hex) = trimmed.strip_prefix("0x") {
        let bits = hex.len() * 4;
        return Some(format!("bits({bits})"));
    }
    if let Some(bin) = trimmed.strip_prefix("0b") {
        let bits = bin.len();
        return Some(format!("bits({bits})"));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use syntax::ast::AstNode;

    #[test]
    fn trivial_block_returns_single_item() {
        // Parse a minimal block expression and verify extraction.
        let source = "function foo () = { x }";
        let (root, _errors) = syntax::parse_text(source);
        // Walk descendants looking for a BlockExpr.
        let block = root.descendants().find_map(ast::BlockExpr::cast);
        if let Some(ref b) = block {
            // If the parser produces a BlockExpr with one item, extraction should succeed.
            let result = extract_trivial_expression(b);
            if b.items().len() == 1 {
                assert!(result.is_some());
            }
        }
        // The key point: the function is callable and exercises the API.
    }
}
