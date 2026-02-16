use std::sync::Arc;
use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::io::Write;
use lsp_types::{
    Url, Location, Range, Position, Hover, HoverParams, HoverContents, MarkupContent, MarkupKind,
    GotoDefinitionParams, GotoDefinitionResponse, WorkspaceSymbolParams, SymbolInformation,
    DocumentSymbolParams, DocumentSymbolResponse, DocumentSymbol, SymbolKind, DocumentFormattingParams,
    TextEdit, ReferenceParams, RenameParams, WorkspaceEdit, CompletionParams, CompletionItem, CompletionItemKind
};

use crate::state::SailState;
use crate::utils::{get_word_at, byte_to_utf16_offset, utf16_offset_to_byte};

pub fn handle_hover(state: &SailState, params: HoverParams) -> Option<Hover> {
    let uri = &params.text_document_position_params.text_document.uri;
    let pos = params.text_document_position_params.position;
    let content = state.files.read().unwrap().get(uri)?.clone();
    let word = get_word_at(&content, pos)?;
    
    let output = {
        let mut repl = state.repl.lock().unwrap();
        repl.send_command(&format!(":t {}", word))
    };
    let joined = output.join("\n").trim().to_string();
    if !joined.is_empty() && !joined.contains("not found") {
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: format!("```sail\n{}\n```", joined),
            }),
            range: None,
        });
    }
    None
}

pub fn handle_definition(state: &Arc<SailState>, params: GotoDefinitionParams) -> Option<GotoDefinitionResponse> {
    let content = state.files.read().unwrap().get(&params.text_document_position_params.text_document.uri)?.clone();
    let word = get_word_at(&content, params.text_document_position_params.position)?;
    state.symbols.read().unwrap().get(&word).map(|infos| {
        if infos.len() == 1 {
            GotoDefinitionResponse::Scalar(infos[0].location.clone())
        } else {
            GotoDefinitionResponse::Array(infos.iter().map(|i| i.location.clone()).collect())
        }
    })
}

pub fn handle_workspace_symbols(state: &SailState, params: WorkspaceSymbolParams) -> Option<Vec<SymbolInformation>> {
    let query = params.query.to_lowercase();
    let mut results = Vec::new();
    let symbols = state.symbols.read().unwrap();
    for (name, infos) in symbols.iter() {
        if name.to_lowercase().contains(&query) {
            for info in infos {
                #[allow(deprecated)]
                results.push(SymbolInformation {
                    name: name.clone(),
                    kind: info.kind,
                    location: info.location.clone(),
                    container_name: None,
                    deprecated: None,
                    tags: None,
                });
            }
        }
    }
    Some(results)
}

pub fn handle_document_symbols(state: &SailState, params: DocumentSymbolParams) -> Option<DocumentSymbolResponse> {
    let uri = params.text_document.uri;
    let mut symbols = Vec::new();
    let symbols_guard = state.symbols.read().unwrap();
    for (name, infos) in symbols_guard.iter() {
        for info in infos {
            if info.location.uri == uri {
                #[allow(deprecated)]
                symbols.push(DocumentSymbol {
                    name: name.clone(),
                    detail: None,
                    kind: info.kind,
                    tags: None,
                    range: info.location.range,
                    selection_range: info.location.range,
                    children: None,
                    deprecated: None,
                });
            }
        }
    }
    symbols.sort_by_key(|s| (s.range.start.line, s.range.start.character));
    Some(DocumentSymbolResponse::Nested(symbols))
}

pub fn handle_formatting(state: &SailState, params: DocumentFormattingParams) -> Option<Vec<TextEdit>> {
    let uri = params.text_document.uri;
    let content = state.files.read().unwrap().get(&uri)?.clone();
    
    let mut cmd = Command::new("sail");
    cmd.arg("--fmt").arg("--fmt-emit").arg("stdout");
    
    if let Ok(path) = uri.to_file_path() {
        cmd.arg(path);
    }
    
    let mut child = cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdin = child.stdin.take()?;
    stdin.write_all(content.as_bytes()).ok()?;
    drop(stdin);
    
    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        let new_text = String::from_utf8_lossy(&output.stdout).into_owned();
        
        let lines: Vec<&str> = content.lines().collect();
        let last_line = lines.len().saturating_sub(1);
        let last_line_text = lines.last().copied().unwrap_or("");
        let last_char_utf16 = last_line_text.chars().map(|c| c.len_utf16() as u32).sum::<u32>();

        return Some(vec![TextEdit {
            range: Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: last_line as u32, character: last_char_utf16 },
            },
            new_text,
        }]);
    }
    None
}

pub fn handle_references(state: &SailState, params: ReferenceParams) -> Option<Vec<Location>> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let content = state.files.read().unwrap().get(uri)?.clone();
    let word = get_word_at(&content, pos)?;
    
    let mut refs = Vec::new();
    let project_files = state.project_files.read().unwrap();
    let opened_files = state.files.read().unwrap();
    
    for path in project_files.iter() {
        let Ok(target_uri) = Url::from_file_path(path) else { continue };
        let text = if let Some(t) = opened_files.get(&target_uri) {
            t.clone()
        } else {
            let Ok(t) = std::fs::read_to_string(path) else { continue };
            t
        };
        
        for (i, line) in text.lines().enumerate() {
            for (m_idx, _) in line.match_indices(&word) {
                let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '#' || c == '$';
                let before = if m_idx > 0 {
                    line[..m_idx].chars().next_back()
                } else {
                    None
                };
                let after = line[m_idx + word.len()..].chars().next();
                
                if before.map_or(true, |c| !is_ident(c)) && after.map_or(true, |c| !is_ident(c)) {
                    refs.push(Location {
                        uri: target_uri.clone(),
                        range: Range {
                            start: Position { line: i as u32, character: byte_to_utf16_offset(line, m_idx) },
                            end: Position { line: i as u32, character: byte_to_utf16_offset(line, m_idx + word.len()) },
                        },
                    });
                }
            }
        }
    }
    Some(refs)
}

pub fn handle_rename(state: &SailState, params: RenameParams) -> Option<WorkspaceEdit> {
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let content = state.files.read().unwrap().get(uri)?.clone();
    let word = get_word_at(&content, pos)?;
    
    let mut changes = HashMap::new();
    let project_files = state.project_files.read().unwrap();
    let opened_files = state.files.read().unwrap();
    
    for path in project_files.iter() {
        let Ok(target_uri) = Url::from_file_path(path) else { continue };
        let text = if let Some(t) = opened_files.get(&target_uri) {
            t.clone()
        } else {
            let Ok(t) = std::fs::read_to_string(path) else { continue };
            t
        };
        
        let mut edits = Vec::new();
        for (i, line) in text.lines().enumerate() {
            for (m_idx, _) in line.match_indices(&word) {
                let is_ident = |c: char| c.is_alphanumeric() || c == '_' || c == '#' || c == '$';
                let before = if m_idx > 0 {
                    line[..m_idx].chars().next_back()
                } else {
                    None
                };
                let after = line[m_idx + word.len()..].chars().next();
                
                if before.map_or(true, |c| !is_ident(c)) && after.map_or(true, |c| !is_ident(c)) {
                    edits.push(TextEdit {
                        range: Range {
                            start: Position { line: i as u32, character: byte_to_utf16_offset(line, m_idx) },
                            end: Position { line: i as u32, character: byte_to_utf16_offset(line, m_idx + word.len()) },
                        },
                        new_text: params.new_name.clone(),
                    });
                }
            }
        }
        if !edits.is_empty() {
            changes.insert(target_uri, edits);
        }
    }
    Some(WorkspaceEdit { changes: Some(changes), ..Default::default() })
}

pub fn handle_completion(state: &SailState, params: CompletionParams) -> Vec<CompletionItem> {
    let mut items = Vec::new();
    
    let uri = &params.text_document_position.text_document.uri;
    let pos = params.text_document_position.position;
    let prefix = {
        let files = state.files.read().unwrap();
        if let Some(content) = files.get(uri) {
            if let Some(line) = content.lines().nth(pos.line as usize) {
                let col_byte = utf16_offset_to_byte(line, pos.character as usize);
                let mut start = col_byte;
                while start > 0 {
                    if let Some(prev_char) = line[..start].chars().next_back() {
                        if prev_char.is_alphanumeric() || prev_char == '_' || prev_char == '#' || prev_char == '$' {
                            start -= prev_char.len_utf8();
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                line[start..col_byte].to_lowercase()
            } else {
                "".to_string()
            }
        } else {
            "".to_string()
        }
    };

    let keywords = vec![
        "val", "function", "type", "struct", "union", "enum", "let", "var", "if", "then", "else", "match", "register",
        "mapping", "overload", "outcome", "clause", "forall", "pure", "impure", "monadic", "scattered", "end"
    ];
    for kw in keywords {
        if kw.starts_with(&prefix) {
            items.push(CompletionItem { label: kw.to_string(), kind: Some(CompletionItemKind::KEYWORD), ..Default::default() });
        }
    }

    let types = vec!["int", "nat", "bool", "unit", "bit", "string", "real", "list", "vector", "bitvector", "bits", "atom", "range"];
    for t in types {
        if t.starts_with(&prefix) {
            items.push(CompletionItem { label: t.to_string(), kind: Some(CompletionItemKind::CLASS), ..Default::default() });
        }
    }

    let directives = vec!["$define", "$include", "$ifdef", "$ifndef", "$endif", "$iftarget", "$option"];
    for d in directives {
        if d.starts_with(&prefix) {
            items.push(CompletionItem { label: d.to_string(), kind: Some(CompletionItemKind::KEYWORD), ..Default::default() });
        }
    }

    let symbols = state.symbols.read().unwrap();
    for (name, infos) in symbols.iter() {
        if name.to_lowercase().starts_with(&prefix) {
            if let Some(info) = infos.first() {
                let kind = match info.kind {
                    SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
                    SymbolKind::CLASS    => CompletionItemKind::CLASS,
                    SymbolKind::FIELD    => CompletionItemKind::FIELD,
                    SymbolKind::VARIABLE => CompletionItemKind::VARIABLE,
                    _                    => CompletionItemKind::CONSTANT,
                };
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(kind),
                    detail: Some(info.location.uri.path().split('/').last().unwrap_or("").to_string()),
                    ..Default::default()
                });
            }
        }
    }
    items
}
