//! `generate_documentation_template` assist — generate a doc comment template for a function.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_documentation_template(
    acc: &mut Assists,
    ctx: &AssistContext<'_>,
) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find "function" keyword near cursor
    let func_start = find_function_keyword(text, offset)?;

    // Parse: function <name>(<params>) -> <ret_ty> = ...
    let after_kw = func_start + "function".len();
    let rest = text[after_kw..].trim_start();
    let name_offset = after_kw + (text[after_kw..].len() - rest.len());

    // Extract function name
    let name_end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    let func_name = &rest[..name_end];

    // Find params between parens
    let paren_open_rel = rest[name_end..].find('(')?;
    let paren_open = name_offset + name_end + paren_open_rel;
    let paren_close = find_matching_paren(text, paren_open)?;

    let params_str = &text[paren_open + 1..paren_close];

    // Parse parameters: "x : int, y : bits(32)"
    let params = parse_params(params_str);

    // Find return type: -> <type>
    let after_paren = &text[paren_close + 1..];
    let ret_ty = if let Some(arrow) = after_paren.find("->") {
        let after_arrow = after_paren[arrow + 2..].trim_start();
        // Return type ends at '=' or newline
        let end = after_arrow.find(|c: char| c == '=' || c == '\n').unwrap_or(after_arrow.len());
        Some(after_arrow[..end].trim().to_string())
    } else {
        None
    };

    // Get indentation of the function line
    let line_start = text[..func_start].rfind('\n').map_or(0, |p| p + 1);
    let indent = &text[line_start..func_start];

    // Build doc comment
    let mut doc = String::new();
    doc.push_str(indent);
    doc.push_str("/** ");
    doc.push_str(func_name);
    doc.push('\n');

    for (name, _ty) in &params {
        doc.push_str(indent);
        doc.push_str(" * @param ");
        doc.push_str(name);
        doc.push('\n');
    }

    if let Some(ref ty) = ret_ty {
        if !ty.is_empty() {
            doc.push_str(indent);
            doc.push_str(" * @returns ");
            doc.push_str(ty);
            doc.push('\n');
        }
    }

    doc.push_str(indent);
    doc.push_str(" */\n");

    acc.add_with_edits(
        AssistId("generate_documentation_template", AssistKind::RefactorRewrite),
        "Generate documentation template",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(func_start, func_start), new_text: doc }],
    );
    Some(())
}

fn find_function_keyword(text: &str, offset: usize) -> Option<usize> {
    let start = offset.saturating_sub(200);
    let end = text.len().min(offset + 200);
    let window = &text[start..end];
    let kw = "function";
    let mut best = None;
    let mut search_from = 0;
    loop {
        match window[search_from..].find(kw) {
            Some(pos) => {
                let abs = start + search_from + pos;
                let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
                let after_ok = abs + kw.len() >= text.len()
                    || !text.as_bytes()[abs + kw.len()].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    best = Some(abs);
                }
                search_from += pos + 1;
            }
            None => break,
        }
    }
    best
}

fn find_matching_paren(text: &str, open: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut i = open + 1;
    let bytes = text.as_bytes();
    while i < text.len() && depth > 0 {
        match bytes[i] {
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_params(params_str: &str) -> Vec<(String, String)> {
    if params_str.trim().is_empty() {
        return Vec::new();
    }

    let mut result = Vec::new();
    // Split on commas, but respect nested parens
    let mut depth = 0i32;
    let mut current = String::new();
    for ch in params_str.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                if let Some(p) = parse_single_param(&current) {
                    result.push(p);
                }
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if let Some(p) = parse_single_param(&current) {
        result.push(p);
    }
    result
}

fn parse_single_param(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // "name : type" or just "name"
    if let Some(colon) = s.find(':') {
        let name = s[..colon].trim().to_string();
        let ty = s[colon + 1..].trim().to_string();
        Some((name, ty))
    } else {
        Some((s.to_string(), String::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(
            generate_documentation_template,
            "function foo(x : int, y : bits(32)) -> int = x\n",
            0,
        );
        let _ = labels;
    }
}
