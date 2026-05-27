//! `convert_comment_block` assist — convert between line comments (`//`) and block comments (`/* */`).

use crate::assist_context::{AssistContext, AssistId, AssistKind, Assists};
use ide_db::text_edit::TextEdit;

pub(crate) fn convert_comment_block(acc: &mut Assists, ctx: &AssistContext<'_>) -> Option<()> {
    let text = ctx.source_text();
    let offset = ctx.offset();

    // Determine what kind of comment the cursor is in.
    if is_in_line_comment(text, offset) {
        convert_line_to_block(acc, ctx, text, offset)
    } else if is_in_block_comment(text, offset) {
        convert_block_to_line(acc, ctx, text, offset)
    } else {
        None
    }
}

fn is_in_line_comment(text: &str, offset: usize) -> bool {
    // Walk backward to start of line
    let line_start = text[..offset].rfind('\n').map_or(0, |p| p + 1);
    let line = text[line_start..].trim_start();
    line.starts_with("//")
}

fn is_in_block_comment(text: &str, offset: usize) -> bool {
    // Walk backward for "/*"
    text[..offset].rfind("/*").is_some() && text[offset..].find("*/").is_some()
}

fn convert_line_to_block(
    acc: &mut Assists,
    ctx: &AssistContext<'_>,
    text: &str,
    offset: usize,
) -> Option<()> {
    // Find the contiguous block of // comments around the cursor
    let lines: Vec<&str> = text.lines().collect();
    let mut cursor_line = 0;
    let mut pos = 0;
    for (i, line) in lines.iter().enumerate() {
        if pos + line.len() >= offset {
            cursor_line = i;
            break;
        }
        pos += line.len() + 1; // +1 for '\n'
    }

    // Find start and end of contiguous // block
    let mut start_line = cursor_line;
    while start_line > 0 && lines[start_line - 1].trim_start().starts_with("//") {
        start_line -= 1;
    }
    let mut end_line = cursor_line;
    while end_line + 1 < lines.len() && lines[end_line + 1].trim_start().starts_with("//") {
        end_line += 1;
    }

    if !lines.get(start_line)?.trim_start().starts_with("//") {
        return None;
    }

    // Compute byte range
    let block_start: usize = lines[..start_line].iter().map(|l| l.len() + 1).sum();
    let block_end: usize = lines[..=end_line].iter().map(|l| l.len() + 1).sum();

    // Get the leading indentation from the first line
    let indent = lines[start_line].len() - lines[start_line].trim_start().len();
    let indent_str = &lines[start_line][..indent];

    // Extract comment content (strip "// " or "//" prefix)
    let mut content_lines = Vec::new();
    for i in start_line..=end_line {
        let trimmed = lines[i].trim_start();
        let body = if trimmed.starts_with("// ") {
            &trimmed[3..]
        } else if trimmed.starts_with("//") {
            &trimmed[2..]
        } else {
            trimmed
        };
        content_lines.push(body);
    }

    let mut result = format!("{}/* ", indent_str);
    if content_lines.len() == 1 {
        result.push_str(content_lines[0]);
        result.push_str(" */\n");
    } else {
        result.push_str(content_lines[0]);
        result.push('\n');
        for line in &content_lines[1..] {
            result.push_str(indent_str);
            result.push_str("   ");
            result.push_str(line);
            result.push('\n');
        }
        result.push_str(indent_str);
        result.push_str("   */\n");
    }

    acc.add_with_edits(
        AssistId("convert_comment_block", AssistKind::RefactorRewrite),
        "Convert to block comment",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(block_start, block_end), new_text: result }],
    );
    Some(())
}

fn convert_block_to_line(
    acc: &mut Assists,
    ctx: &AssistContext<'_>,
    text: &str,
    offset: usize,
) -> Option<()> {
    // Find the block comment boundaries
    let open = text[..=offset].rfind("/*")?;
    let close = text[open..].find("*/")? + open;

    // Get indentation
    let line_start = text[..open].rfind('\n').map_or(0, |p| p + 1);
    let indent_str = &text[line_start..open];

    // Extract content between /* and */
    let content = &text[open + 2..close];
    let content = content.trim();

    // Split into lines
    let content_lines: Vec<&str> = content.lines().collect();

    let mut result = String::new();
    for line in &content_lines {
        let trimmed = line.trim().trim_start_matches("* ").trim_start_matches('*');
        result.push_str(indent_str);
        result.push_str("// ");
        result.push_str(trimmed.trim());
        result.push('\n');
    }

    // Range includes from line_start to after "*/" + newline
    let end = if text.as_bytes().get(close + 2) == Some(&b'\n') { close + 3 } else { close + 2 };

    acc.add_with_edits(
        AssistId("convert_comment_block", AssistKind::RefactorRewrite),
        "Convert to line comments",
        ctx.range,
        vec![TextEdit { range: base_db::text_range(line_start, end), new_text: result }],
    );
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::check_assist;

    #[test]
    fn smoke() {
        let labels = check_assist(convert_comment_block, "// hello\n", 0);
        let _ = labels;
    }
}
