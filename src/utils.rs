use lsp_types::{Position, TextDocumentContentChangeEvent};

pub fn byte_to_utf16_offset(line: &str, byte_offset: usize) -> u32 {
    let mut utf16_offset = 0;
    for (i, c) in line.char_indices() {
        if i >= byte_offset {
            return utf16_offset;
        }
        utf16_offset += c.len_utf16() as u32;
    }
    utf16_offset
}

pub fn utf16_offset_to_byte(line: &str, utf16_col: usize) -> usize {
    let mut utf16_pos = 0;
    for (byte_idx, ch) in line.char_indices() {
        if utf16_pos >= utf16_col { return byte_idx; }
        utf16_pos += ch.len_utf16();
    }
    line.len()
}

pub fn position_to_byte_offset(content: &str, pos: Position) -> usize {
    let mut current_line = 0;
    for (i, c) in content.char_indices() {
        if current_line == pos.line as usize {
            let line_rest = &content[i..];
            let next_newline = line_rest.find('\n').unwrap_or(line_rest.len());
            let line_text = &line_rest[..next_newline];
            return i + utf16_offset_to_byte(line_text, pos.character as usize);
        }
        if c == '\n' {
            current_line += 1;
        }
    }
    content.len()
}

pub fn apply_changes(content: &mut String, changes: Vec<TextDocumentContentChangeEvent>) {
    for change in changes {
        if let Some(range) = change.range {
            let start = position_to_byte_offset(content, range.start);
            let end = position_to_byte_offset(content, range.end);
            if start <= end && end <= content.len() {
                content.replace_range(start..end, &change.text);
            }
        } else {
            *content = change.text;
        }
    }
}

pub fn get_word_at(content: &str, pos: Position) -> Option<String> {
    let line = content.lines().nth(pos.line as usize)?;
    let col_byte = utf16_offset_to_byte(line, pos.character as usize);
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_' || c == '#' || c == '$';
    
    let mut start = col_byte;
    while start > 0 {
        let prev_char = line[..start].chars().next_back()?;
        if is_ident_char(prev_char) {
            start -= prev_char.len_utf8();
        } else {
            break;
        }
    }
    
    let mut end = col_byte;
    while end < line.len() {
        let next_char = line[end..].chars().next()?;
        if is_ident_char(next_char) {
            end += next_char.len_utf8();
        } else {
            break;
        }
    }
    
    if start < end {
        Some(line[start..end].to_string())
    } else {
        None
    }
}
