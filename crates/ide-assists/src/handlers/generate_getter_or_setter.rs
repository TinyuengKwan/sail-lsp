//! `generate_getter_or_setter` assist — generate getter/setter functions for struct fields.

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn generate_getter_or_setter(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Find "struct" keyword near cursor
    let struct_start = find_keyword_near(text, offset, "struct")?;

    // Parse: struct <Name> = { <field> : <type>, ... }
    let after_kw = struct_start + "struct".len();
    let rest = text[after_kw..].trim_start();
    let name_start = after_kw + (text[after_kw..].len() - rest.len());

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

    let brace_open_abs =
        name_start + name_end_rel + eq_pos + 1 + (after_name[eq_pos + 1..].len() - after_eq.len());
    let brace_close = find_matching_brace(text, brace_open_abs)?;

    let fields_str = &text[brace_open_abs + 1..brace_close];
    let fields = parse_fields(fields_str);
    if fields.is_empty() {
        return None;
    }

    // Build getter and setter functions for each field
    let mut funcs = String::new();
    for (field_name, field_ty) in &fields {
        // Getter
        funcs.push_str(&format!(
            "\nfunction get_{}(s : {}) -> {} = s.{}",
            field_name, struct_name, field_ty, field_name,
        ));
        // Setter
        funcs.push_str(&format!(
            "\nfunction set_{}(s : {}, v : {}) -> {} = {{ s with {} = v }}",
            field_name, struct_name, field_ty, struct_name, field_name,
        ));
    }

    // Insert after the struct definition
    let insert_pos =
        text[brace_close + 1..].find('\n').map_or(brace_close + 1, |p| brace_close + 1 + p + 1);

    acc.add_with_edits(
        AssistId("generate_getter_or_setter", AssistKind::Generate),
        "Generate getters and setters",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(insert_pos, insert_pos), new_text: funcs }],
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
                let before_ok = abs == 0 || !text.as_bytes()[abs - 1].is_ascii_alphanumeric();
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
        let labels = check_assist(generate_getter_or_setter, "struct Foo = { bar : int }\n", 0);
        let _ = labels;
    }
}
