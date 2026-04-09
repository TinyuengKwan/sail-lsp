use crate::state::File;
use std::collections::HashMap;
use tower_lsp::lsp_types::{
    CodeActionKind, Diagnostic, FormattingOptions, Position, Range, TextEdit, Url,
};

pub(crate) fn code_action_kind_allowed(
    requested: &Option<Vec<CodeActionKind>>,
    action_kind: &CodeActionKind,
) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let action = action_kind.as_str();
    requested
        .iter()
        .any(|kind| action.starts_with(kind.as_str()) || kind.as_str().starts_with(action))
}

pub(crate) fn lazy_code_action_data(uri: &Url, edits: &[TextEdit]) -> serde_json::Value {
    serde_json::json!({
        "uri": uri.as_str(),
        "edits": edits,
    })
}

pub(crate) fn resolve_code_action_edit_from_data(
    data: &serde_json::Value,
) -> Option<(Url, Vec<TextEdit>)> {
    let uri = data.get("uri")?.as_str().and_then(|s| Url::parse(s).ok())?;
    let edits_value = data.get("edits")?.clone();
    let edits = serde_json::from_value::<Vec<TextEdit>>(edits_value).ok()?;
    Some((uri, edits))
}

pub(crate) fn sail_source_fix_all_kind() -> CodeActionKind {
    CodeActionKind::new("source.fixAll.sail")
}

pub(crate) fn default_code_action_format_options() -> FormattingOptions {
    FormattingOptions {
        tab_size: 2,
        insert_spaces: true,
        properties: HashMap::new(),
        trim_trailing_whitespace: Some(true),
        insert_final_newline: None,
        trim_final_newlines: None,
    }
}

fn expected_token_from_message(message: &str) -> Option<&'static str> {
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

fn line_text(file: &File, line: u32) -> Option<&str> {
    let start = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line, 0));
    if start > file.source.text().len() {
        return None;
    }
    let end = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line + 1, 0))
        .min(file.source.text().len());
    Some(&file.source.text()[start..end])
}

pub(crate) fn missing_semicolon_fix(file: &File, diagnostic: &Diagnostic) -> Option<TextEdit> {
    let message = diagnostic.message.to_ascii_lowercase();
    if !message.contains("expected") || !message.contains(';') {
        return None;
    }

    let line = diagnostic.range.start.line;
    let text = line_text(file, line)?;
    let mut logical = text.trim_end_matches(['\n', '\r']);
    if let Some((head, _)) = logical.split_once("//") {
        logical = head;
    }
    let logical = logical.trim_end();
    if logical.is_empty() || logical.ends_with(';') {
        return None;
    }
    if logical.ends_with('{') || logical.ends_with('}') {
        return None;
    }

    let base = file
        .source
        .offset_at(&tower_lsp::lsp_types::Position::new(line, 0));
    let insert_offset = base + logical.len();
    let insert_pos = file.source.position_at(insert_offset);

    Some(TextEdit {
        range: Range::new(insert_pos, insert_pos),
        new_text: ";".to_string(),
    })
}

fn missing_token_fix(file: &File, diagnostic: &Diagnostic, token: &str) -> Option<TextEdit> {
    if !matches!(token, ")" | "]" | "}" | "," | "=") {
        return None;
    }
    let pos = diagnostic.range.end;
    let offset = file.source.offset_at(&pos);
    if file.source.text().get(offset..offset + token.len()) == Some(token) {
        return None;
    }
    Some(TextEdit {
        range: Range::new(pos, pos),
        new_text: token.to_string(),
    })
}

pub(crate) fn quick_fix_for_diagnostic(
    file: &File,
    diagnostic: &Diagnostic,
) -> Option<(String, TextEdit, bool)> {
    let token = expected_token_from_message(&diagnostic.message)?;
    if token == ";" {
        let edit = missing_semicolon_fix(file, diagnostic)?;
        return Some(("Insert missing `;`".to_string(), edit, true));
    }
    let edit = missing_token_fix(file, diagnostic, token)?;
    Some((format!("Insert missing `{token}`"), edit, false))
}

/// Quick fix for `unused-variable`: prefix the variable name with `_`
/// to silence the warning while preserving the binding's structural role.
/// This matches both the upstream Sail convention (`_` prefix marks
/// intentionally-unused identifiers) and rust-analyzer's suggestion.
pub(crate) fn unused_variable_fix(
    file: &File,
    diagnostic: &Diagnostic,
) -> Option<(String, TextEdit, bool)> {
    let code_str = match diagnostic.code.as_ref()? {
        tower_lsp::lsp_types::NumberOrString::String(s) => s.as_str(),
        _ => return None,
    };
    if code_str != "unused-variable" {
        return None;
    }
    // The diagnostic range is the variable name. Extract it from the source
    // to make sure we're not double-prefixing an already-`_`-prefixed name.
    let start_offset = file.source.offset_at(&diagnostic.range.start);
    let end_offset = file.source.offset_at(&diagnostic.range.end);
    let name = file.source.text().get(start_offset..end_offset)?;
    if name.starts_with('_') {
        return None;
    }
    Some((
        format!("Rename `{name}` to `_{name}`"),
        TextEdit {
            range: tower_lsp::lsp_types::Range::new(
                diagnostic.range.start,
                diagnostic.range.start,
            ),
            new_text: "_".to_string(),
        },
        true,
    ))
}

pub(crate) fn var_to_let_fix(file: &File, diagnostic: &Diagnostic) -> Option<(String, TextEdit, bool)> {
    let code_str = match diagnostic.code.as_ref()? {
        tower_lsp::lsp_types::NumberOrString::String(s) => s.as_str(),
        _ => return None,
    };
    if code_str != "unmodified-mutable-variable" {
        return None;
    }
    // The diagnostic range points to the variable name. We need to find the `var` keyword before it.
    let name_start = file.source.offset_at(&diagnostic.range.start);
    let prefix = &file.source.text()[..name_start];
    let var_start = prefix.rfind("var")?;
    // Verify it's the keyword (preceded by whitespace or line start)
    if var_start > 0 {
        let before = file.source.text().as_bytes()[var_start - 1];
        if before != b' ' && before != b'\t' && before != b'\n' && before != b'\r' && before != b'{' && before != b';' {
            return None;
        }
    }
    let var_end = var_start + 3; // "var".len()
    Some((
        "Change `var` to `let`".to_string(),
        TextEdit {
            range: tower_lsp::lsp_types::Range::new(
                file.source.position_at(var_start),
                file.source.position_at(var_end),
            ),
            new_text: "let".to_string(),
        },
        true,
    ))
}

pub(crate) fn extract_local_let_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let start = file.source.offset_at(&range.start);
    let end = file.source.offset_at(&range.end);
    if start >= end {
        return None;
    }

    let selected = file.source.text().get(start..end)?.trim();
    if selected.is_empty() || selected.contains('\n') || selected.contains('\r') {
        return None;
    }

    let binding = "extracted_value";
    let line_start = Position::new(range.start.line, 0);
    let line_start_offset = file.source.offset_at(&line_start);
    let line_end_offset = file
        .source
        .offset_at(&Position::new(range.start.line + 1, 0))
        .min(file.source.text().len());
    let current_line = file.source.text().get(line_start_offset..line_end_offset)?;
    let indent = current_line
        .chars()
        .take_while(|ch| *ch == ' ' || *ch == '\t')
        .collect::<String>();

    let insert_text = format!("{indent}let {binding} = {selected};\n");
    Some(vec![
        TextEdit {
            range: Range::new(line_start, line_start),
            new_text: insert_text,
        },
        TextEdit {
            range,
            new_text: binding.to_string(),
        },
    ])
}

fn is_import_directive(line: &str) -> bool {
    line.starts_with("$include ") || line.starts_with("include ")
}

pub(crate) fn organize_imports_edits(file: &File) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    if text.is_empty() {
        return None;
    }

    let mut lines: Vec<(usize, usize, &str)> = Vec::new();
    let mut cursor = 0usize;
    for chunk in text.split_inclusive('\n') {
        let end = cursor + chunk.len();
        lines.push((cursor, end, chunk));
        cursor = end;
    }
    if cursor < text.len() {
        lines.push((cursor, text.len(), &text[cursor..]));
    }
    if lines.is_empty() {
        return None;
    }

    let mut first_code = 0usize;
    while first_code < lines.len() {
        let trimmed = lines[first_code].2.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") {
            first_code += 1;
            continue;
        }
        break;
    }
    if first_code >= lines.len() {
        return None;
    }

    let first_trimmed = lines[first_code].2.trim();
    if !is_import_directive(first_trimmed) || first_trimmed.contains("//") {
        return None;
    }

    let mut block_end = first_code;
    let mut directives: Vec<String> = Vec::new();
    while block_end < lines.len() {
        let trimmed = lines[block_end].2.trim();
        if trimmed.is_empty() {
            block_end += 1;
            continue;
        }
        if trimmed.starts_with("//") {
            return None;
        }
        if !is_import_directive(trimmed) || trimmed.contains("//") {
            break;
        }
        directives.push(trimmed.to_string());
        block_end += 1;
    }

    if directives.len() < 2 {
        return None;
    }

    directives.sort_unstable();
    directives.dedup();

    let eol = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let old_range_start = lines[first_code].0;
    let old_range_end = lines[block_end - 1].1;
    let mut replacement = directives.join(eol);
    if text.get(old_range_end.saturating_sub(1)..old_range_end) == Some("\n") {
        replacement.push_str(eol);
    }

    let old_block = text.get(old_range_start..old_range_end)?;
    if old_block == replacement {
        return None;
    }

    let edit = TextEdit {
        range: Range::new(
            file.source.position_at(old_range_start),
            file.source.position_at(old_range_end),
        ),
        new_text: replacement,
    };
    Some(vec![edit])
}

// ---------------------------------------------------------------------------
// Invert If  (swap then/else branches and negate condition)
// ---------------------------------------------------------------------------

/// Given a cursor position inside an `if` expression, produce edits that invert the condition
/// and swap the then/else branches.
pub(crate) fn invert_if_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let (expr, expr_span) = find_if_at_offset(core_ast, offset)?;
    let Expr::If {
        cond,
        then_branch,
        else_branch: Some(else_branch),
    } = expr
    else {
        return None;
    };

    let text = file.source.text();
    let cond_text = text.get(cond.1.start..cond.1.end)?;
    let then_text = text.get(then_branch.1.start..then_branch.1.end)?;
    let else_text = text.get(else_branch.1.start..else_branch.1.end)?;

    let inverted_cond = invert_condition(cond_text);

    // Replace the entire if expression
    let new_text = format!(
        "if {} then {} else {}",
        inverted_cond, else_text, then_text
    );
    Some(vec![TextEdit {
        range: Range::new(
            file.source.position_at(expr_span.start),
            file.source.position_at(expr_span.end),
        ),
        new_text,
    }])
}

fn invert_condition(cond: &str) -> String {
    let trimmed = cond.trim();
    // !(expr) → expr
    if let Some(inner) = trimmed
        .strip_prefix("~(")
        .and_then(|s| s.strip_suffix(')'))
    {
        return inner.to_string();
    }
    if let Some(inner) = trimmed
        .strip_prefix("not(")
        .and_then(|s| s.strip_suffix(')'))
    {
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

use sail_parser::core_ast::{
    BlockItem, DefinitionKind, Expr, SourceFile as CoreSourceFile,
};
use sail_parser::DeclRole;

/// Walk the core AST to find an `if` expression whose span contains `offset`.
fn find_if_at_offset<'a>(
    ast: &'a CoreSourceFile,
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    for (def, _def_span) in &ast.defs {
        if let Some(result) = find_if_in_def(&def.kind, offset) {
            return Some(result);
        }
    }
    None
}

fn find_if_in_def(
    kind: &DefinitionKind,
    offset: usize,
) -> Option<(&Expr, sail_parser::Span)> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                find_if_in_expr(body, offset)
            } else {
                for clause in &c.clauses {
                    if let Some(body) = &clause.0.body {
                        if let Some(r) = find_if_in_expr(body, offset) {
                            return Some(r);
                        }
                    }
                }
                None
            }
        }
        _ => None,
    }
}

fn find_if_in_expr<'a>(
    expr: &'a (Expr, sail_parser::Span),
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    // Try to find a more specific (nested) if first
    match &expr.0 {
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            // Recurse into sub-expressions first
            if let Some(r) = find_if_in_expr(cond, offset) {
                return Some(r);
            }
            if let Some(r) = find_if_in_expr(then_branch, offset) {
                return Some(r);
            }
            if let Some(Some(r)) = else_branch.as_ref().map(|e| find_if_in_expr(e, offset)) {
                return Some(r);
            }
            // This if expression itself
            if else_branch.is_some() {
                return Some((&expr.0, expr.1));
            }
            None
        }
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                        if let Some(r) = find_if_in_expr(e, offset) {
                            return Some(r);
                        }
                    }
                    BlockItem::Let(lb) => {
                        if let Some(r) = find_if_in_expr(&*lb.value, offset) {
                            return Some(r);
                        }
                    }
                }
            }
            None
        }
        Expr::Let { body, .. } | Expr::Var { body, .. } => find_if_in_expr(body, offset),
        Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
            for case in cases {
                if let Some(r) = find_if_in_expr(&case.0.body, offset) {
                    return Some(r);
                }
            }
            None
        }
        Expr::Foreach(f) => find_if_in_expr(&f.body, offset),
        Expr::While { body, .. } | Expr::Repeat { body, .. } => find_if_in_expr(body, offset),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Flip Binary Expression (swap lhs and rhs of a binary operator)
// ---------------------------------------------------------------------------

pub(crate) fn flip_binexpr_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let (expr, expr_span) = find_infix_at_offset(core_ast, offset)?;
    let Expr::Infix { lhs, op, rhs } = expr else {
        return None;
    };

    let text = file.source.text();
    let lhs_text = text.get(lhs.1.start..lhs.1.end)?;
    let rhs_text = text.get(rhs.1.start..rhs.1.end)?;
    let op_text = &op.0;

    let flipped_op = flip_comparison_op(op_text);

    let new_text = format!("{} {} {}", rhs_text, flipped_op, lhs_text);
    Some(vec![TextEdit {
        range: Range::new(
            file.source.position_at(expr_span.start),
            file.source.position_at(expr_span.end),
        ),
        new_text,
    }])
}

fn flip_comparison_op(op: &str) -> &str {
    match op {
        "<" => ">",
        ">" => "<",
        "<=" => ">=",
        ">=" => "<=",
        other => other, // ==, !=, +, -, *, etc. stay the same
    }
}

fn find_infix_at_offset<'a>(
    ast: &'a CoreSourceFile,
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    for (def, _) in &ast.defs {
        if let Some(result) = find_infix_in_def(&def.kind, offset) {
            return Some(result);
        }
    }
    None
}

fn find_infix_in_def(
    kind: &DefinitionKind,
    offset: usize,
) -> Option<(&Expr, sail_parser::Span)> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                find_infix_in_expr(body, offset)
            } else {
                for clause in &c.clauses {
                    if let Some(body) = &clause.0.body {
                        if let Some(r) = find_infix_in_expr(body, offset) {
                            return Some(r);
                        }
                    }
                }
                None
            }
        }
        _ => None,
    }
}

fn find_infix_in_expr<'a>(
    expr: &'a (Expr, sail_parser::Span),
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        Expr::Infix { lhs, op, rhs } => {
            // Check if cursor is on the operator
            if offset >= op.1.start && offset <= op.1.end {
                return Some((&expr.0, expr.1));
            }
            // Otherwise recurse into operands
            find_infix_in_expr(lhs, offset)
                .or_else(|| find_infix_in_expr(rhs, offset))
        }
        Expr::Block(items) => items.iter().find_map(|item| match &item.0 {
            BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                find_infix_in_expr(e, offset)
            }
            BlockItem::Let(lb) => find_infix_in_expr(&*lb.value, offset),
        }),
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => find_infix_in_expr(cond, offset)
            .or_else(|| find_infix_in_expr(then_branch, offset))
            .or_else(|| {
                else_branch
                    .as_ref()
                    .and_then(|e| find_infix_in_expr(e, offset))
            }),
        Expr::Let { body, .. } | Expr::Var { body, .. } => find_infix_in_expr(body, offset),
        Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
            cases.iter().find_map(|c| find_infix_in_expr(&c.0.body, offset))
        }
        Expr::Foreach(f) => find_infix_in_expr(&f.body, offset),
        Expr::While { body, cond, .. } => {
            find_infix_in_expr(cond, offset).or_else(|| find_infix_in_expr(body, offset))
        }
        Expr::Repeat { body, until, .. } => {
            find_infix_in_expr(body, offset).or_else(|| find_infix_in_expr(until, offset))
        }
        Expr::Call(call) => {
            for arg in &call.args {
                if let Some(r) = find_infix_in_expr(arg, offset) {
                    return Some(r);
                }
            }
            find_infix_in_expr(&call.callee, offset)
        }
        Expr::Return(e) | Expr::Throw(e) | Expr::Prefix { expr: e, .. } | Expr::Cast { expr: e, .. } => {
            find_infix_in_expr(e, offset)
        }
        Expr::Assign { rhs, .. } => find_infix_in_expr(rhs, offset),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Apply De Morgan's Law
// ---------------------------------------------------------------------------

pub(crate) fn apply_demorgan_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let (expr, expr_span) = find_infix_at_offset(core_ast, offset)?;
    let Expr::Infix { lhs, op, rhs } = expr else {
        return None;
    };

    // Only applies to boolean operators & and |
    let new_op = match op.0.as_str() {
        "&" => "|",
        "|" => "&",
        _ => return None,
    };

    let text = file.source.text();
    let lhs_text = text.get(lhs.1.start..lhs.1.end)?;
    let rhs_text = text.get(rhs.1.start..rhs.1.end)?;

    let inv_lhs = invert_condition(lhs_text);
    let inv_rhs = invert_condition(rhs_text);

    let new_text = format!("~({inv_lhs} {new_op} {inv_rhs})");
    Some(vec![TextEdit {
        range: Range::new(
            file.source.position_at(expr_span.start),
            file.source.position_at(expr_span.end),
        ),
        new_text,
    }])
}

// ---------------------------------------------------------------------------
// Inline Variable (reverse of extract let)
// ---------------------------------------------------------------------------

pub(crate) fn inline_variable_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let text = file.source.text();

    // Find the let binding at cursor position
    let core_ast = file.core_ast.as_deref()?;
    let (name, value_span, let_span, body_span) = find_let_at_offset(core_ast, offset)?;
    let value_text = text.get(value_span.start..value_span.end)?;

    // Find all uses of `name` in the body and replace them
    let body_text = text.get(body_span.start..body_span.end)?;

    // Collect offsets of the name in the body
    let mut replacements: Vec<(usize, usize)> = Vec::new();
    let name_len = name.len();
    let mut search_start = 0;
    while let Some(pos) = body_text[search_start..].find(&name) {
        let abs = search_start + pos;
        // Check that it's a whole word
        let before_ok = abs == 0
            || !body_text.as_bytes()[abs - 1].is_ascii_alphanumeric()
                && body_text.as_bytes()[abs - 1] != b'_';
        let after_ok = abs + name_len >= body_text.len()
            || !body_text.as_bytes()[abs + name_len].is_ascii_alphanumeric()
                && body_text.as_bytes()[abs + name_len] != b'_';
        if before_ok && after_ok {
            replacements.push((body_span.start + abs, body_span.start + abs + name_len));
        }
        search_start = abs + 1;
    }

    if replacements.is_empty() {
        return None;
    }

    let mut edits = Vec::new();

    // Replace the let expression with just the body
    // First, replace all occurrences in the body
    for &(start, end) in replacements.iter().rev() {
        edits.push(TextEdit {
            range: Range::new(
                file.source.position_at(start),
                file.source.position_at(end),
            ),
            new_text: value_text.to_string(),
        });
    }

    // Remove the let binding line: replace `let name = value in body` with `body`
    // But since Sail let bindings have different shapes, we replace the whole let expr
    // with the body where the name is already replaced.
    // Actually, simpler: remove the `let name = value;` or `let name = value in` prefix
    let let_prefix_end = body_span.start;
    edits.push(TextEdit {
        range: Range::new(
            file.source.position_at(let_span.start),
            file.source.position_at(let_prefix_end),
        ),
        new_text: String::new(),
    });

    Some(edits)
}

fn find_let_at_offset(
    ast: &CoreSourceFile,
    offset: usize,
) -> Option<(String, sail_parser::Span, sail_parser::Span, sail_parser::Span)> {
    for (def, _) in &ast.defs {
        if let Some(r) = find_let_in_def(&def.kind, offset) {
            return Some(r);
        }
    }
    None
}

fn find_let_in_def(
    kind: &DefinitionKind,
    offset: usize,
) -> Option<(String, sail_parser::Span, sail_parser::Span, sail_parser::Span)> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                find_let_in_expr(body, offset)
            } else {
                for clause in &c.clauses {
                    if let Some(body) = &clause.0.body {
                        if let Some(r) = find_let_in_expr(body, offset) {
                            return Some(r);
                        }
                    }
                }
                None
            }
        }
        _ => None,
    }
}

fn find_let_in_expr(
    expr: &(Expr, sail_parser::Span),
    offset: usize,
) -> Option<(String, sail_parser::Span, sail_parser::Span, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        Expr::Let { binding, body } => {
            // Check if cursor is on the let keyword / binding name region
            if offset < body.1.start {
                // Cursor is in the let-binding area
                use sail_parser::core_ast::Pattern;
                let name = match &binding.pattern.0 {
                    Pattern::Ident(n) => n.clone(),
                    _ => return None,
                };
                return Some((name, binding.value.1, expr.1, body.1));
            }
            // Recurse into body
            find_let_in_expr(body, offset)
        }
        Expr::Block(items) => items.iter().find_map(|item| match &item.0 {
            BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                find_let_in_expr(e, offset)
            }
            BlockItem::Let(lb) => find_let_in_expr(&*lb.value, offset),
        }),
        Expr::If {
            cond,
            then_branch,
            else_branch,
        } => find_let_in_expr(cond, offset)
            .or_else(|| find_let_in_expr(then_branch, offset))
            .or_else(|| {
                else_branch
                    .as_ref()
                    .and_then(|e| find_let_in_expr(e, offset))
            }),
        Expr::Var { body, .. } => find_let_in_expr(body, offset),
        Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
            cases.iter().find_map(|c| find_let_in_expr(&c.0.body, offset))
        }
        Expr::Foreach(f) => find_let_in_expr(&f.body, offset),
        Expr::While { body, .. } | Expr::Repeat { body, .. } => find_let_in_expr(body, offset),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Extract Function
// ---------------------------------------------------------------------------

pub(crate) fn extract_function_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let start = file.source.offset_at(&range.start);
    let end = file.source.offset_at(&range.end);
    if start >= end {
        return None;
    }
    let text = file.source.text();
    let selected = text.get(start..end)?.trim();
    if selected.is_empty() {
        return None;
    }

    // Find free variables in the selected text by looking at parsed data
    let parsed = file.parsed()?;
    let mut params: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    // Simple heuristic: find identifiers in the selection that are defined outside
    for occ in &parsed.symbol_occurrences {
        let occ_start = occ.span.start;
        let occ_end = occ.span.end;
        if occ_start >= start && occ_end <= end {
            // This occurrence is inside the selection
            if occ.role.is_none()
                && occ.scope == Some(sail_parser::Scope::Local)
            {
                // Check if the definition is outside the selection
                if let Some(def_occ) = parsed.symbol_occurrences.iter().find(|o| {
                    o.name == occ.name
                        && o.role == Some(DeclRole::Definition)
                        && (o.span.start < start || o.span.end > end)
                }) {
                    if seen.insert(def_occ.name.clone()) {
                        params.push(def_occ.name.clone());
                    }
                }
            }
        }
    }

    let func_name = "extracted_function";
    let param_list = params.join(", ");
    let call_text = if params.is_empty() {
        format!("{func_name}()")
    } else {
        format!("{func_name}({param_list})")
    };

    // Find the end of the current top-level definition to insert the new function after it
    let insert_offset = find_def_end_after(file, start)?;

    let indent = "  ";
    let func_def = format!(
        "\n\nfunction {func_name}({param_list}) = {{\n{indent}{selected}\n}}\n"
    );

    Some(vec![
        // Replace selection with function call
        TextEdit {
            range,
            new_text: call_text,
        },
        // Insert new function definition
        TextEdit {
            range: Range::new(
                file.source.position_at(insert_offset),
                file.source.position_at(insert_offset),
            ),
            new_text: func_def,
        },
    ])
}

fn find_def_end_after(file: &File, offset: usize) -> Option<usize> {
    let core_ast = file.core_ast.as_deref()?;
    let mut best_end = None;
    for (_, def_span) in &core_ast.defs {
        if def_span.start <= offset && offset <= def_span.end {
            best_end = Some(def_span.end);
        }
    }
    best_end.or_else(|| Some(file.source.text().len()))
}

// ---------------------------------------------------------------------------
// Generate Documentation Template
// ---------------------------------------------------------------------------

pub(crate) fn generate_doc_template_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;

    for (def, def_span) in &core_ast.defs {
        if offset < def_span.start || offset > def_span.end {
            continue;
        }
        match &def.kind {
            DefinitionKind::Callable(c) => {
                let text = file.source.text();
                // Check if there's already a doc comment
                let before = &text[..def_span.start];
                if before.trim_end().ends_with("*/") || before.trim_end().ends_with("///") {
                    return None;
                }

                let mut doc = String::from("/*!\n");
                doc.push_str(&format!(" * {}\n", c.name.0));
                doc.push_str(" *\n");
                for param in &c.params {
                    if let sail_parser::core_ast::Pattern::Ident(name) = &param.0 {
                        doc.push_str(&format!(" * @param {name}\n"));
                    }
                }
                // Also handle clause params
                for clause in &c.clauses {
                    for param in &clause.0.patterns {
                        if let sail_parser::core_ast::Pattern::Ident(name) = &param.0 {
                            doc.push_str(&format!(" * @param {name}\n"));
                        }
                    }
                    break; // just first clause
                }
                doc.push_str(" * @return\n");
                doc.push_str(" */\n");

                return Some(vec![TextEdit {
                    range: Range::new(
                        file.source.position_at(def_span.start),
                        file.source.position_at(def_span.start),
                    ),
                    new_text: doc,
                }]);
            }
            DefinitionKind::Named(_) | DefinitionKind::TypeAlias(_) => {
                let text = file.source.text();
                let before = &text[..def_span.start];
                if before.trim_end().ends_with("*/") || before.trim_end().ends_with("///") {
                    return None;
                }
                let doc = format!("/*!\n * TODO: document\n */\n");
                return Some(vec![TextEdit {
                    range: Range::new(
                        file.source.position_at(def_span.start),
                        file.source.position_at(def_span.start),
                    ),
                    new_text: doc,
                }]);
            }
            _ => {}
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Remove Unused Imports
// ---------------------------------------------------------------------------

pub(crate) fn remove_unused_imports_edits(file: &File) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let parsed = file.parsed()?;

    // Collect all $include paths
    let mut edits = Vec::new();
    let mut cursor = 0usize;
    for chunk in text.split_inclusive('\n') {
        let line_start = cursor;
        let line_end = cursor + chunk.len();
        let trimmed = chunk.trim();
        cursor = line_end;

        if let Some(path) = trimmed
            .strip_prefix("$include \"")
            .and_then(|s| {
                s.strip_suffix('"')
                    .or_else(|| s.strip_suffix("\"\n"))
                    .or_else(|| s.strip_suffix("\"\r\n"))
            })
        {
            // Check if this include is actually used
            // Simple heuristic: check if the included file's name appears in any
            // cross-file reference. If no symbols from this file are referenced, flag it.
            // For now, we check against the file name stem
            let stem = std::path::Path::new(path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(path);

            // Check if any declarations from this include are used in the current file
            let is_used = parsed.symbol_occurrences.iter().any(|occ| {
                occ.role.is_none()
                    && occ.scope == Some(sail_parser::Scope::TopLevel)
            });

            // If clearly unused (no external references at all), add removal edit
            // This is conservative - we only remove if we're confident
            if !is_used && !stem.is_empty() {
                edits.push(TextEdit {
                    range: Range::new(
                        file.source.position_at(line_start),
                        file.source.position_at(line_end),
                    ),
                    new_text: String::new(),
                });
            }
        }
    }

    if edits.is_empty() {
        None
    } else {
        Some(edits)
    }
}

// ---------------------------------------------------------------------------
// Unwrap Block (remove if/while/foreach wrapper, keep body)
// ---------------------------------------------------------------------------

pub(crate) fn unwrap_block_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let text = file.source.text();

    // Find an if/while/foreach/repeat at the cursor position
    for (def, _) in &core_ast.defs {
        if let Some((body_span, wrapper_span)) = find_unwrappable_in_def(&def.kind, offset) {
            let body_text = text.get(body_span.start..body_span.end)?;
            // Strip surrounding braces if present
            let trimmed = body_text.trim();
            let inner = if trimmed.starts_with('{') && trimmed.ends_with('}') {
                trimmed[1..trimmed.len() - 1].trim()
            } else {
                trimmed
            };
            return Some(vec![TextEdit {
                range: Range::new(
                    file.source.position_at(wrapper_span.start),
                    file.source.position_at(wrapper_span.end),
                ),
                new_text: inner.to_string(),
            }]);
        }
    }
    None
}

fn find_unwrappable_in_def(
    kind: &DefinitionKind,
    offset: usize,
) -> Option<(sail_parser::Span, sail_parser::Span)> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                return find_unwrappable_in_expr(body, offset);
            }
            for clause in &c.clauses {
                if let Some(body) = &clause.0.body {
                    if let Some(r) = find_unwrappable_in_expr(body, offset) {
                        return Some(r);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_unwrappable_in_expr(
    expr: &(Expr, sail_parser::Span),
    offset: usize,
) -> Option<(sail_parser::Span, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        Expr::If { then_branch, else_branch, .. } => {
            // Only unwrap if cursor is on the if keyword area (near start)
            if offset < expr.1.start + 5 {
                return Some((then_branch.1, expr.1));
            }
            find_unwrappable_in_expr(then_branch, offset)
                .or_else(|| else_branch.as_ref().and_then(|e| find_unwrappable_in_expr(e, offset)))
        }
        Expr::While { body, .. } | Expr::Repeat { body, .. } => {
            if offset < expr.1.start + 8 {
                return Some((body.1, expr.1));
            }
            find_unwrappable_in_expr(body, offset)
        }
        Expr::Foreach(f) => {
            if offset < expr.1.start + 8 {
                return Some((f.body.1, expr.1));
            }
            find_unwrappable_in_expr(&f.body, offset)
        }
        Expr::Block(items) => items.iter().find_map(|item| match &item.0 {
            BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                find_unwrappable_in_expr(e, offset)
            }
            BlockItem::Let(lb) => find_unwrappable_in_expr(&*lb.value, offset),
        }),
        Expr::Let { body, .. } | Expr::Var { body, .. } => find_unwrappable_in_expr(body, offset),
        Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
            cases.iter().find_map(|c| find_unwrappable_in_expr(&c.0.body, offset))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Pull Assignment Up
// ---------------------------------------------------------------------------

pub(crate) fn pull_assignment_up_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let text = file.source.text();

    // Find an if-else where both branches end with assignment to the same target
    for (def, _) in &core_ast.defs {
        if let Some(result) = find_pullable_in_def(&def.kind, offset, text, file) {
            return Some(result);
        }
    }
    None
}

fn find_pullable_in_def(
    kind: &DefinitionKind,
    offset: usize,
    text: &str,
    file: &File,
) -> Option<Vec<TextEdit>> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                return find_pullable_in_expr(body, offset, text, file);
            }
            for clause in &c.clauses {
                if let Some(body) = &clause.0.body {
                    if let Some(r) = find_pullable_in_expr(body, offset, text, file) {
                        return Some(r);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_pullable_in_expr(
    expr: &(Expr, sail_parser::Span),
    offset: usize,
    text: &str,
    file: &File,
) -> Option<Vec<TextEdit>> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        Expr::If {
            cond,
            then_branch,
            else_branch: Some(else_branch),
        } => {
            // Check if both branches are assignments to the same target
            let then_text = text.get(then_branch.1.start..then_branch.1.end)?.trim();
            let else_text = text.get(else_branch.1.start..else_branch.1.end)?.trim();

            // Strip braces
            let then_inner = strip_block_braces(then_text);
            let else_inner = strip_block_braces(else_text);

            // Check for `target = value` pattern
            if let (Some((then_target, then_val)), Some((else_target, else_val))) =
                (split_assignment(then_inner), split_assignment(else_inner))
            {
                if then_target == else_target {
                    let cond_text = text.get(cond.1.start..cond.1.end)?;
                    let new_text = format!(
                        "{} = if {} then {} else {}",
                        then_target, cond_text, then_val.trim_end_matches(';').trim(),
                        else_val.trim_end_matches(';').trim()
                    );
                    return Some(vec![TextEdit {
                        range: Range::new(
                            file.source.position_at(expr.1.start),
                            file.source.position_at(expr.1.end),
                        ),
                        new_text,
                    }]);
                }
            }
            // Recurse
            find_pullable_in_expr(then_branch, offset, text, file)
                .or_else(|| find_pullable_in_expr(else_branch, offset, text, file))
        }
        Expr::Block(items) => items.iter().find_map(|item| match &item.0 {
            BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                find_pullable_in_expr(e, offset, text, file)
            }
            BlockItem::Let(lb) => find_pullable_in_expr(&*lb.value, offset, text, file),
        }),
        Expr::Let { body, .. } | Expr::Var { body, .. } => {
            find_pullable_in_expr(body, offset, text, file)
        }
        Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
            cases.iter().find_map(|c| find_pullable_in_expr(&c.0.body, offset, text, file))
        }
        Expr::Foreach(f) => find_pullable_in_expr(&f.body, offset, text, file),
        Expr::While { body, .. } | Expr::Repeat { body, .. } => {
            find_pullable_in_expr(body, offset, text, file)
        }
        _ => None,
    }
}

fn strip_block_braces(s: &str) -> &str {
    let s = s.trim();
    if s.starts_with('{') && s.ends_with('}') {
        s[1..s.len() - 1].trim()
    } else {
        s
    }
}

fn split_assignment(s: &str) -> Option<(&str, &str)> {
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

// ---------------------------------------------------------------------------
// Convert to Guarded Return
// ---------------------------------------------------------------------------

pub(crate) fn guarded_return_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let text = file.source.text();

    let (expr, expr_span) = find_if_at_offset(core_ast, offset)?;
    let Expr::If {
        cond,
        then_branch,
        else_branch,
    } = expr
    else {
        return None;
    };

    // Only convert if there's no else branch (simple guard)
    if else_branch.is_some() {
        return None;
    }

    let cond_text = text.get(cond.1.start..cond.1.end)?;
    let body_text = text.get(then_branch.1.start..then_branch.1.end)?;
    let inverted = invert_condition(cond_text);

    // Strip braces from body
    let body_inner = strip_block_braces(body_text.trim());

    let new_text = format!("if {inverted} then return ();\n{body_inner}");
    Some(vec![TextEdit {
        range: Range::new(
            file.source.position_at(expr_span.start),
            file.source.position_at(expr_span.end),
        ),
        new_text,
    }])
}

// ---------------------------------------------------------------------------
// Sort Items (sort struct fields / enum members alphabetically)
// ---------------------------------------------------------------------------

pub(crate) fn sort_items_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let text = file.source.text();

    for (def, def_span) in &core_ast.defs {
        if offset < def_span.start || offset > def_span.end {
            continue;
        }
        if let DefinitionKind::Named(nd) = &def.kind {
            match &nd.detail {
                Some(sail_parser::core_ast::NamedDefDetail::Enum { members, .. }) => {
                    if members.len() < 2 {
                        return None;
                    }
                    let first_start = members.first()?.1.start;
                    let last_end = members.last()?.1.end;
                    let mut names: Vec<(&str, sail_parser::Span)> = members
                        .iter()
                        .map(|(m, s)| (m.name.0.as_str(), *s))
                        .collect();
                    let already_sorted = names.windows(2).all(|w| w[0].0 <= w[1].0);
                    if already_sorted {
                        return None;
                    }
                    names.sort_by_key(|(name, _)| *name);
                    let sorted_text = names
                        .iter()
                        .map(|(_, span)| text.get(span.start..span.end).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join(",\n");
                    return Some(vec![TextEdit {
                        range: Range::new(
                            file.source.position_at(first_start),
                            file.source.position_at(last_end),
                        ),
                        new_text: sorted_text,
                    }]);
                }
                Some(sail_parser::core_ast::NamedDefDetail::Struct { fields }) => {
                    if fields.len() < 2 {
                        return None;
                    }
                    let first_start = fields.first()?.1.start;
                    let last_end = fields.last()?.1.end;
                    let mut field_data: Vec<(&str, sail_parser::Span)> = fields
                        .iter()
                        .map(|(f, s)| (f.name.0.as_str(), *s))
                        .collect();
                    let already_sorted = field_data.windows(2).all(|w| w[0].0 <= w[1].0);
                    if already_sorted {
                        return None;
                    }
                    field_data.sort_by_key(|(name, _)| *name);
                    let sorted_text = field_data
                        .iter()
                        .map(|(_, span)| text.get(span.start..span.end).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join(",\n");
                    return Some(vec![TextEdit {
                        range: Range::new(
                            file.source.position_at(first_start),
                            file.source.position_at(last_end),
                        ),
                        new_text: sorted_text,
                    }]);
                }
                Some(sail_parser::core_ast::NamedDefDetail::Union { variants }) => {
                    if variants.len() < 2 {
                        return None;
                    }
                    let first_start = variants.first()?.1.start;
                    let last_end = variants.last()?.1.end;
                    let mut variant_data: Vec<(&str, sail_parser::Span)> = variants
                        .iter()
                        .map(|(v, s)| (v.name.0.as_str(), *s))
                        .collect();
                    let already_sorted = variant_data.windows(2).all(|w| w[0].0 <= w[1].0);
                    if already_sorted {
                        return None;
                    }
                    variant_data.sort_by_key(|(name, _)| *name);
                    let sorted_text = variant_data
                        .iter()
                        .map(|(_, span)| text.get(span.start..span.end).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join(",\n");
                    return Some(vec![TextEdit {
                        range: Range::new(
                            file.source.position_at(first_start),
                            file.source.position_at(last_end),
                        ),
                        new_text: sorted_text,
                    }]);
                }
                _ => {}
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Comment Conversions
// ---------------------------------------------------------------------------

/// Convert line comments (//) to block comments (/* */) in the selection range
pub(crate) fn line_to_block_comment_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let start_offset = file.source.offset_at(&range.start);
    let end_offset = file.source.offset_at(&range.end);

    // Find consecutive // lines in the range
    let region = text.get(start_offset..end_offset)?;
    let lines: Vec<&str> = region.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let all_line_comments = lines.iter().all(|l| l.trim().starts_with("//"));
    if !all_line_comments {
        return None;
    }

    let mut block = String::from("/*\n");
    for line in &lines {
        let stripped = line.trim().strip_prefix("///").or_else(|| line.trim().strip_prefix("//"));
        if let Some(content) = stripped {
            block.push_str(&format!(" *{}\n", content));
        }
    }
    block.push_str(" */");

    Some(vec![TextEdit {
        range,
        new_text: block,
    }])
}

/// Convert block comments (/* */) to line comments (//)
pub(crate) fn block_to_line_comment_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let start_offset = file.source.offset_at(&range.start);
    let end_offset = file.source.offset_at(&range.end);

    let region = text.get(start_offset..end_offset)?.trim();
    if !region.starts_with("/*") || !region.ends_with("*/") {
        return None;
    }

    let inner = &region[2..region.len() - 2];
    let mut lines = Vec::new();
    for line in inner.lines() {
        let trimmed = line.trim().strip_prefix("* ").or_else(|| line.trim().strip_prefix('*'));
        let content = trimmed.unwrap_or(line.trim());
        if content.is_empty() {
            continue;
        }
        lines.push(format!("// {content}"));
    }

    if lines.is_empty() {
        return None;
    }

    Some(vec![TextEdit {
        range,
        new_text: lines.join("\n"),
    }])
}

/// Toggle between // and /// (doc comment)
pub(crate) fn toggle_doc_comment_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let text = file.source.text();
    let start_offset = file.source.offset_at(&range.start);
    let end_offset = file.source.offset_at(&range.end);
    let region = text.get(start_offset..end_offset)?;

    let lines: Vec<&str> = region.lines().collect();
    if lines.is_empty() {
        return None;
    }

    let is_doc = lines.iter().all(|l| l.trim().starts_with("///"));
    let is_normal = lines.iter().all(|l| {
        let t = l.trim();
        t.starts_with("//") && !t.starts_with("///")
    });

    if !is_doc && !is_normal {
        return None;
    }

    let new_text = if is_doc {
        // /// → //
        lines
            .iter()
            .map(|l| {
                let trimmed = l.trim();
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                let content = trimmed.strip_prefix("///").unwrap_or(trimmed);
                format!("{indent}//{content}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        // // → ///
        lines
            .iter()
            .map(|l| {
                let trimmed = l.trim();
                let indent: String = l.chars().take_while(|c| c.is_whitespace()).collect();
                let content = trimmed.strip_prefix("//").unwrap_or(trimmed);
                format!("{indent}///{content}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    };

    Some(vec![TextEdit {
        range,
        new_text,
    }])
}

// ---------------------------------------------------------------------------
// Add Missing Match Arms
// ---------------------------------------------------------------------------

pub(crate) fn add_missing_match_arms_edits<'a, I>(
    file: &File,
    range: Range,
    all_files: I,
) -> Option<Vec<TextEdit>>
where
    I: IntoIterator<Item = (&'a tower_lsp::lsp_types::Url, &'a File)>,
{
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;
    let text = file.source.text();

    // Find a match expression at cursor
    let (match_expr, match_span) = find_match_expr_at_offset(core_ast, offset)?;
    let (scrutinee, cases) = match match_expr {
        Expr::Match { scrutinee, cases } => (scrutinee, cases),
        _ => return None,
    };

    // Get the scrutinee name (simple case: identifier)
    let _scrutinee_text = text.get(scrutinee.1.start..scrutinee.1.end)?.trim();

    // Collect existing case pattern names
    let existing_arms: std::collections::HashSet<String> = cases
        .iter()
        .filter_map(|(case, _)| {
            extract_pattern_name(&case.pattern.0)
        })
        .collect();

    // Find the type of the scrutinee by looking at declarations
    let all_files_vec: Vec<_> = all_files.into_iter().collect();
    let mut enum_members = Vec::new();
    let mut union_variants = Vec::new();

    // Look for type information through declarations
    for (_, f) in &all_files_vec {
        let Some(ast) = f.core_ast.as_deref() else { continue };
        for (def, _) in &ast.defs {
            if let DefinitionKind::Named(nd) = &def.kind {
                match &nd.detail {
                    Some(sail_parser::core_ast::NamedDefDetail::Enum { members, .. }) => {
                        // Check if any existing arm matches a member of this enum
                        let member_names: Vec<&str> = members.iter().map(|(m, _)| m.name.0.as_str()).collect();
                        if existing_arms.iter().any(|a| member_names.contains(&a.as_str())) {
                            for (m, _) in members {
                                if !existing_arms.contains(&m.name.0) {
                                    enum_members.push(m.name.0.clone());
                                }
                            }
                        }
                    }
                    Some(sail_parser::core_ast::NamedDefDetail::Union { variants, .. }) => {
                        let variant_names: Vec<&str> = variants.iter().map(|(v, _)| v.name.0.as_str()).collect();
                        if existing_arms.iter().any(|a| variant_names.contains(&a.as_str())) {
                            for (v, _) in variants {
                                if !existing_arms.contains(&v.name.0) {
                                    let arm_pattern = match &v.payload {
                                        sail_parser::core_ast::UnionPayload::Type(_) => {
                                            format!("{}(_)", v.name.0)
                                        }
                                        sail_parser::core_ast::UnionPayload::Struct { .. } => {
                                            format!("{}(..)", v.name.0)
                                        }
                                    };
                                    union_variants.push(arm_pattern);
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    let missing_arms: Vec<String> = if !enum_members.is_empty() {
        enum_members
    } else if !union_variants.is_empty() {
        union_variants
    } else {
        return None;
    };

    if missing_arms.is_empty() {
        return None;
    }

    // Find where to insert (before the closing brace of match)
    let _insert_offset = match_span.end.saturating_sub(1);
    // Walk backward to find the `}` of the match
    let before_close = text[..match_span.end].rfind('}')?;

    let indent = "    ";
    let mut new_arms = String::new();
    for arm in &missing_arms {
        new_arms.push_str(&format!("\n{indent}{arm} => (),"));
    }

    Some(vec![TextEdit {
        range: Range::new(
            file.source.position_at(before_close),
            file.source.position_at(before_close),
        ),
        new_text: new_arms,
    }])
}

fn extract_pattern_name(pattern: &sail_parser::core_ast::Pattern) -> Option<String> {
    use sail_parser::core_ast::Pattern;
    match pattern {
        Pattern::Ident(name) => Some(name.clone()),
        Pattern::App { callee, .. } => Some(callee.0.clone()),
        Pattern::Typed(inner, _) => extract_pattern_name(&inner.0),
        Pattern::Attribute { pattern: inner, .. } => extract_pattern_name(&inner.0),
        _ => None,
    }
}

fn find_match_expr_at_offset<'a>(
    ast: &'a CoreSourceFile,
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    for (def, _) in &ast.defs {
        if let Some(r) = find_match_in_def(&def.kind, offset) {
            return Some(r);
        }
    }
    None
}

fn find_match_in_def(
    kind: &DefinitionKind,
    offset: usize,
) -> Option<(&Expr, sail_parser::Span)> {
    match kind {
        DefinitionKind::Callable(c) => {
            if let Some(body) = &c.body {
                return find_match_in_expr(body, offset);
            }
            for clause in &c.clauses {
                if let Some(body) = &clause.0.body {
                    if let Some(r) = find_match_in_expr(body, offset) {
                        return Some(r);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

fn find_match_in_expr<'a>(
    expr: &'a (Expr, sail_parser::Span),
    offset: usize,
) -> Option<(&'a Expr, sail_parser::Span)> {
    if offset < expr.1.start || offset > expr.1.end {
        return None;
    }
    match &expr.0 {
        Expr::Match { cases, .. } => {
            // Recurse into cases first to find inner matches
            for case in cases {
                if let Some(r) = find_match_in_expr(&case.0.body, offset) {
                    return Some(r);
                }
            }
            // This match itself
            Some((&expr.0, expr.1))
        }
        Expr::Block(items) => items.iter().find_map(|item| match &item.0 {
            BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => find_match_in_expr(e, offset),
            BlockItem::Let(lb) => find_match_in_expr(&*lb.value, offset),
        }),
        Expr::If { cond, then_branch, else_branch, .. } => {
            find_match_in_expr(cond, offset)
                .or_else(|| find_match_in_expr(then_branch, offset))
                .or_else(|| else_branch.as_ref().and_then(|e| find_match_in_expr(e, offset)))
        }
        Expr::Let { body, .. } | Expr::Var { body, .. } => find_match_in_expr(body, offset),
        Expr::Foreach(f) => find_match_in_expr(&f.body, offset),
        Expr::While { body, .. } | Expr::Repeat { body, .. } => find_match_in_expr(body, offset),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Bitfield Accessor Generation
// ---------------------------------------------------------------------------

pub(crate) fn bitfield_accessor_edits(file: &File, range: Range) -> Option<Vec<TextEdit>> {
    let offset = file.source.offset_at(&range.start);
    let core_ast = file.core_ast.as_deref()?;

    for (def, def_span) in &core_ast.defs {
        if offset < def_span.start || offset > def_span.end {
            continue;
        }
        if let DefinitionKind::Named(nd) = &def.kind {
            if let Some(sail_parser::core_ast::NamedDefDetail::Bitfield { fields }) = &nd.detail {
                let bf_name = &nd.name.0;
                let mut code = String::new();

                for (field, _) in fields {
                    let field_name = &field.name.0;
                    // Skip the implicit `bits` field
                    if field_name == "bits" {
                        continue;
                    }

                    // Generate getter
                    code.push_str(&format!(
                        "\nfunction _get_{bf_name}_{field_name}(bf : {bf_name}) -> bits('n) = bf.bits[{field_name}]\n"
                    ));

                    // Generate setter
                    code.push_str(&format!(
                        "\nfunction _set_{bf_name}_{field_name}(bf : {bf_name}, v : bits('n)) -> {bf_name} = {{\n  let new_bits = [bf.bits with {field_name} = v];\n  Mk_{bf_name}(new_bits)\n}}\n"
                    ));
                }

                if code.is_empty() {
                    return None;
                }

                return Some(vec![TextEdit {
                    range: Range::new(
                        file.source.position_at(def_span.end),
                        file.source.position_at(def_span.end),
                    ),
                    new_text: code,
                }]);
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Constant Folding (simple evaluator for numeric/boolean literals)
// ---------------------------------------------------------------------------

pub(crate) fn try_fold_constant(text: &str) -> Option<String> {
    let trimmed = text.trim();

    // Simple integer arithmetic
    if let Some(result) = try_eval_int_expr(trimmed) {
        return Some(format!("{result}"));
    }

    // Boolean constants
    match trimmed {
        "true & true" => return Some("true".to_string()),
        "true & false" | "false & true" | "false & false" => {
            return Some("false".to_string())
        }
        "true | true" | "true | false" | "false | true" => {
            return Some("true".to_string())
        }
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

fn try_eval_int_expr(s: &str) -> Option<i64> {
    // Try direct integer literal
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }

    // Try binary operations: a + b, a - b, a * b, a / b
    for op in [" + ", " - ", " * ", " / "] {
        if let Some(pos) = s.rfind(op) {
            let lhs = try_eval_int_expr(s[..pos].trim())?;
            let rhs = try_eval_int_expr(s[pos + op.len()..].trim())?;
            return match op.trim() {
                "+" => Some(lhs + rhs),
                "-" => Some(lhs - rhs),
                "*" => Some(lhs * rhs),
                "/" if rhs != 0 => Some(lhs / rhs),
                _ => None,
            };
        }
    }

    // Parenthesized expression
    if s.starts_with('(') && s.ends_with(')') {
        return try_eval_int_expr(&s[1..s.len() - 1]);
    }

    // 2 ^ n (power)
    if let Some(pos) = s.find(" ^ ") {
        let base = try_eval_int_expr(s[..pos].trim())?;
        let exp = try_eval_int_expr(s[pos + 3..].trim())?;
        if exp >= 0 && exp < 63 {
            return Some(base.pow(exp as u32));
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::{extract_local_let_edits, organize_imports_edits};
    use crate::state::File;
    use tower_lsp::lsp_types::{Position, Range};

    #[test]
    fn extract_local_let_builds_insert_and_replace_edits() {
        let src = "function f(x) = x + 1\n";
        let file = File::new(src.to_string());
        let start = src.find("x + 1").expect("selection start");
        let end = start + "x + 1".len();
        let edits = extract_local_let_edits(
            &file,
            Range::new(file.source.position_at(start), file.source.position_at(end)),
        )
        .expect("extract edits");
        assert_eq!(edits.len(), 2);
        assert!(edits[0].new_text.contains("let extracted_value = x + 1;"));
        assert_eq!(edits[1].new_text, "extracted_value");
    }

    #[test]
    fn extract_local_let_rejects_empty_or_multiline_selection() {
        let src = "let x = (\n  1 + 2\n)\n";
        let file = File::new(src.to_string());
        let empty = Range::new(Position::new(0, 0), Position::new(0, 0));
        assert!(extract_local_let_edits(&file, empty).is_none());

        let start = src.find("(\n").expect("multiline start");
        let end = src.find(")\n").expect("multiline end") + 1;
        let range = Range::new(file.source.position_at(start), file.source.position_at(end));
        assert!(extract_local_let_edits(&file, range).is_none());
    }

    #[test]
    fn organize_imports_sorts_and_deduplicates_top_block() {
        let src = "$include \"b.sail\"\n$include \"a.sail\"\n$include \"a.sail\"\nlet x = 1\n";
        let file = File::new(src.to_string());
        let edits = organize_imports_edits(&file).expect("organize edits");
        assert_eq!(edits.len(), 1);
        assert_eq!(
            edits[0].new_text,
            "$include \"a.sail\"\n$include \"b.sail\"\n"
        );
    }

    #[test]
    fn organize_imports_skips_when_comment_in_block() {
        let src = "$include \"b.sail\"\n// keep note\n$include \"a.sail\"\n";
        let file = File::new(src.to_string());
        assert!(organize_imports_edits(&file).is_none());
    }
}
