//! `generate_new` assist — generate a constructor function for a struct.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_new(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find "struct" keyword near cursor
    let struct_start = find_keyword_near(text, offset, "struct")?;

    // Parse: struct <Name> = { <field> : <type>, ... }
    let after_kw = struct_start + "struct".len();
    let rest = text[after_kw..].trim_start();
    let name_start = after_kw + (text[after_kw..].len() - rest.len());

    // Extract struct name
    let name_end_rel = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    let struct_name = &rest[..name_end_rel];
    if struct_name.is_empty() {
        return None;
    }

    // Find '=' then '{'
    let after_name = &rest[name_end_rel..];
    let eq_pos = after_name.find('=')?;
    let after_eq = after_name[eq_pos + 1..].trim_start();
    if !after_eq.starts_with('{') {
        return None;
    }

    let brace_open_abs = name_start + name_end_rel + eq_pos + 1
        + (after_name[eq_pos + 1..].len() - after_eq.len());
    let brace_close = find_matching_brace(text, brace_open_abs)?;

    let fields_str = &text[brace_open_abs + 1..brace_close];
    let fields = parse_fields(fields_str);
    if fields.is_empty() {
        return None;
    }

    // Build constructor function
    let params: Vec<String> = fields.iter().map(|(n, t)| format!("{} : {}", n, t)).collect();
    let assignments: Vec<String> = fields.iter().map(|(n, _)| format!("{} = {}", n, n)).collect();

    let func = format!(
        "\nfunction mk_{}({}) -> {} = struct {{ {} }}",
        struct_name,
        params.join(", "),
        struct_name,
        assignments.join(", "),
    );

    // Insert after the struct definition (after closing brace)
    // Find end of line after close brace
    let insert_pos = text[brace_close + 1..]
        .find('\n')
        .map_or(brace_close + 1, |p| brace_close + 1 + p + 1);

    acc.add_with_edits(
        AssistId("generate_new", AssistKind::Generate),
        "Generate constructor",
        ctx.range,
        vec![TextEdit {
            range: base_db::text_range(insert_pos, insert_pos),
            new_text: func,
        }],
    );
    Some(())
}

fn find_keyword_near(text: &str, offset: usize, kw: &str) -> Option<usize> {
    let start = offset.saturating_sub(200);
    let end = text.len().min(offset + 200);
    let window = &text[start..end];
    let mut best = None;
    let mut search_from = 0;
    let kw_len = kw.len();
    loop {
        match window[search_from..].find(kw) {
            Some(pos) => {
                let abs = start + search_from + pos;
                let before_ok =
                    abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
                let after_ok = abs + kw_len >= text.len()
                    || !text.as_bytes()[abs + kw_len].is_ascii_alphanumeric();
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

fn find_matching_brace(text: &str, open: usize) -> Option<usize> {
    let mut depth = 1i32;
    let mut i = open + 1;
    let bytes = text.as_bytes();
    while i < text.len() && depth > 0 {
        match bytes[i] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        if depth == 0 {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn parse_fields(fields_str: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    for item in fields_str.split(',') {
        let item = item.trim();
        if item.is_empty() {
            continue;
        }
        if let Some(colon) = item.find(':') {
            let name = item[..colon].trim().to_string();
            let ty = item[colon + 1..].trim().to_string();
            if !name.is_empty() && !ty.is_empty() {
                result.push((name, ty));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(
            generate_new,
            "struct Point = { x : int, y : int }\n",
            0,
        );
        let _ = labels;
    }
}
