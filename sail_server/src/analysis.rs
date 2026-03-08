use crate::file::File;
use tower_lsp::lsp_types::{Location, Range, SymbolKind, Url};

#[derive(Clone)]
pub(crate) struct SymbolDecl {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    pub(crate) detail: &'static str,
    pub(crate) offset: usize,
}

#[derive(Clone)]
pub(crate) struct CallableSignature {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) params: Vec<String>,
    pub(crate) return_type: Option<String>,
}

pub(crate) fn token_symbol_key(token: &sail_parser::Token) -> Option<String> {
    match token {
        sail_parser::Token::Id(name) => Some(name.clone()),
        sail_parser::Token::TyVal(name) => Some(format!("'{}", name)),
        _ => None,
    }
}

pub(crate) fn extract_symbol_decls(file: &File) -> Vec<SymbolDecl> {
    let Some(tokens) = file.tokens.as_ref() else {
        return Vec::new();
    };

    sail_parser::parse_tokens(tokens)
        .decls
        .into_iter()
        .filter_map(|decl| {
            if decl.scope == sail_parser::Scope::Local
                && !matches!(decl.kind, sail_parser::DeclKind::EnumMember)
            {
                return None;
            }
            let (kind, detail) = match decl.kind {
                sail_parser::DeclKind::Function => (SymbolKind::FUNCTION, "function"),
                sail_parser::DeclKind::Value => (SymbolKind::FUNCTION, "value"),
                sail_parser::DeclKind::Mapping => (SymbolKind::FUNCTION, "mapping"),
                sail_parser::DeclKind::Overload => (SymbolKind::FUNCTION, "overload"),
                sail_parser::DeclKind::Register => (SymbolKind::VARIABLE, "register"),
                sail_parser::DeclKind::Type
                | sail_parser::DeclKind::Struct
                | sail_parser::DeclKind::Union
                | sail_parser::DeclKind::Bitfield
                | sail_parser::DeclKind::Newtype => (SymbolKind::STRUCT, "type"),
                sail_parser::DeclKind::Enum => (SymbolKind::ENUM, "enum"),
                sail_parser::DeclKind::EnumMember => (SymbolKind::ENUM_MEMBER, "enum member"),
                sail_parser::DeclKind::Let | sail_parser::DeclKind::Var => {
                    (SymbolKind::VARIABLE, "binding")
                }
            };

            Some(SymbolDecl {
                name: decl.name,
                kind,
                detail,
                offset: decl.span.start,
            })
        })
        .collect()
}

pub(crate) fn location_from_offset(uri: &Url, file: &File, offset: usize) -> Location {
    let position = file.source.position_at(offset);
    Location::new(uri.clone(), Range::new(position, position))
}

pub(crate) fn is_definition_at(file: &File, name: &str, offset: usize) -> bool {
    file.definitions.get(name).copied() == Some(offset)
}

pub(crate) fn token_is_open_bracket(token: &sail_parser::Token) -> bool {
    matches!(
        token,
        sail_parser::Token::LeftBracket
            | sail_parser::Token::LeftSquareBracket
            | sail_parser::Token::LeftCurlyBracket
            | sail_parser::Token::LeftCurlyBar
            | sail_parser::Token::LeftSquareBar
    )
}

pub(crate) fn token_is_close_bracket(token: &sail_parser::Token) -> bool {
    matches!(
        token,
        sail_parser::Token::RightBracket
            | sail_parser::Token::RightSquareBracket
            | sail_parser::Token::RightCurlyBracket
            | sail_parser::Token::RightCurlyBar
            | sail_parser::Token::RightSquareBar
    )
}

fn token_starts_declaration(token: &sail_parser::Token) -> bool {
    matches!(
        token,
        sail_parser::Token::KwFunction
            | sail_parser::Token::KwVal
            | sail_parser::Token::KwMapping
            | sail_parser::Token::KwOverload
            | sail_parser::Token::KwRegister
            | sail_parser::Token::KwType
            | sail_parser::Token::KwStruct
            | sail_parser::Token::KwUnion
            | sail_parser::Token::KwBitfield
            | sail_parser::Token::KwNewtype
            | sail_parser::Token::KwEnum
            | sail_parser::Token::KwLet
            | sail_parser::Token::KwVar
    )
}

fn token_span_text(text: &str, begin: usize, end: usize) -> String {
    text[begin..end].trim().to_string()
}

pub(crate) fn collect_callable_signatures(file: &File) -> Vec<CallableSignature> {
    let Some(tokens) = file.tokens.as_ref() else {
        return Vec::new();
    };
    let text = file.source.text();
    let parsed = sail_parser::parse_tokens(tokens);
    let mut out = Vec::new();
    let mut token_index_by_start = std::collections::HashMap::<usize, usize>::new();
    for (idx, (_, span)) in tokens.iter().enumerate() {
        token_index_by_start.insert(span.start, idx);
    }

    for decl in parsed.decls {
        if decl.scope != sail_parser::Scope::TopLevel
            || !matches!(
                decl.kind,
                sail_parser::DeclKind::Function
                    | sail_parser::DeclKind::Value
                    | sail_parser::DeclKind::Mapping
            )
        {
            continue;
        }
        let Some(i) = token_index_by_start.get(&decl.span.start).copied() else {
            continue;
        };
        if i == 0 {
            continue;
        }
        let kw_idx = i - 1;

        match decl.kind {
            sail_parser::DeclKind::Value | sail_parser::DeclKind::Mapping => {
                let mut colon_idx = None;
                let mut j = i + 1;
                while j < tokens.len() {
                    if token_starts_declaration(&tokens[j].0) {
                        break;
                    }
                    if tokens[j].0 == sail_parser::Token::Colon {
                        colon_idx = Some(j);
                        break;
                    }
                    j += 1;
                }
                let Some(colon_idx) = colon_idx else {
                    continue;
                };

                let mut segments = Vec::new();
                let mut segment_start = colon_idx + 1;
                let mut depth = 0_i32;
                let mut end_idx = tokens.len().saturating_sub(1);
                let mut k = colon_idx + 1;
                while k < tokens.len() {
                    let token = &tokens[k].0;
                    if token_is_open_bracket(token) {
                        depth += 1;
                    } else if token_is_close_bracket(token) {
                        depth -= 1;
                    }

                    if *token == sail_parser::Token::RightArrow && depth == 0 {
                        let segment =
                            token_span_text(text, tokens[segment_start].1.start, tokens[k].1.start);
                        if !segment.is_empty() {
                            segments.push(segment);
                        }
                        segment_start = k + 1;
                    } else if depth == 0
                        && (token_starts_declaration(token)
                            || *token == sail_parser::Token::Semicolon)
                    {
                        end_idx = k.saturating_sub(1);
                        break;
                    } else {
                        end_idx = k;
                    }
                    k += 1;
                }
                if segment_start <= end_idx && end_idx < tokens.len() {
                    let segment =
                        token_span_text(text, tokens[segment_start].1.start, tokens[end_idx].1.end);
                    if !segment.is_empty() {
                        segments.push(segment);
                    }
                }

                let label = token_span_text(text, tokens[kw_idx].1.start, tokens[end_idx].1.end);
                let params = if segments.len() > 1 {
                    segments[..segments.len() - 1]
                        .iter()
                        .enumerate()
                        .map(|(idx, ty)| format!("arg{}: {}", idx + 1, ty))
                        .collect::<Vec<_>>()
                } else {
                    Vec::new()
                };
                let return_type = segments.last().cloned();
                out.push(CallableSignature {
                    name: decl.name,
                    label,
                    params,
                    return_type,
                });
            }
            sail_parser::DeclKind::Function => {
                let mut params = Vec::new();
                if i + 1 < tokens.len() && tokens[i + 1].0 == sail_parser::Token::LeftBracket {
                    let mut depth = 0_i32;
                    let mut expect_param_name = true;
                    let mut j = i + 1;
                    while j < tokens.len() {
                        let token = &tokens[j].0;
                        match token {
                            sail_parser::Token::LeftBracket => depth += 1,
                            sail_parser::Token::RightBracket => {
                                depth -= 1;
                                if depth <= 0 {
                                    break;
                                }
                            }
                            sail_parser::Token::Comma if depth == 1 => {
                                expect_param_name = true;
                            }
                            sail_parser::Token::Id(param_name)
                                if depth == 1 && expect_param_name =>
                            {
                                params.push(param_name.clone());
                                expect_param_name = false;
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                }

                let mut end_idx = i;
                let mut depth = 0_i32;
                let mut j = i + 1;
                while j < tokens.len() {
                    let token = &tokens[j].0;
                    if token_is_open_bracket(token) {
                        depth += 1;
                    } else if token_is_close_bracket(token) {
                        depth -= 1;
                    }
                    if depth == 0
                        && (*token == sail_parser::Token::Equal
                            || *token == sail_parser::Token::Semicolon)
                    {
                        break;
                    }
                    end_idx = j;
                    j += 1;
                }
                let label = token_span_text(text, tokens[kw_idx].1.start, tokens[end_idx].1.end);
                let return_type = label
                    .split_once("->")
                    .map(|(_, tail)| tail.trim().to_string())
                    .filter(|ty| !ty.is_empty());

                out.push(CallableSignature {
                    name: decl.name,
                    label,
                    params,
                    return_type,
                });
            }
            _ => {}
        }
    }

    out
}

pub(crate) fn find_callable_signature<'a, I>(
    files: I,
    uri: &Url,
    name: &str,
) -> Option<CallableSignature>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut best: Option<(usize, CallableSignature)> = None;
    for (candidate_uri, candidate_file) in files {
        for sig in collect_callable_signatures(candidate_file) {
            if sig.name != name {
                continue;
            }
            let score = match (uri.path_segments(), candidate_uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            };
            match &best {
                Some((best_score, _)) if *best_score > score => {}
                _ => best = Some((score, sig)),
            }
        }
    }
    best.map(|(_, sig)| sig)
}

pub(crate) fn find_call_at_position(
    file: &File,
    position: tower_lsp::lsp_types::Position,
) -> Option<(String, usize)> {
    let tokens = file.tokens.as_ref()?;
    let offset = file.source.offset_at(&position);
    let parsed = sail_parser::parse_tokens(tokens);
    let mut candidate = None::<sail_parser::CallSite>;
    for call in parsed.call_sites {
        if call.open_span.start > offset {
            continue;
        }
        if let Some(close) = call.close_span {
            if close.end < offset {
                continue;
            }
        }
        match &candidate {
            Some(current) if current.open_span.start > call.open_span.start => {}
            _ => candidate = Some(call),
        }
    }
    let call = candidate?;
    let arg_index = call
        .arg_separator_spans
        .iter()
        .filter(|span| span.start < offset)
        .count();
    Some((call.callee, arg_index))
}

fn snippet_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

pub(crate) fn function_snippet(name: &str, params: &[String]) -> String {
    if params.is_empty() {
        return format!("{name}()");
    }
    let body = params
        .iter()
        .enumerate()
        .map(|(idx, param)| {
            let placeholder = param
                .split(':')
                .next()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .unwrap_or("arg");
            format!("${{{}:{}}}", idx + 1, snippet_escape(placeholder))
        })
        .collect::<Vec<_>>()
        .join(", ");
    format!("{name}({body})")
}

pub(crate) fn inlay_param_name(param: &str) -> &str {
    param
        .split(':')
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(param)
}

pub(crate) fn infer_binding_type(token: &sail_parser::Token) -> Option<&'static str> {
    match token {
        sail_parser::Token::Num(_) => Some("int"),
        sail_parser::Token::Real(_) => Some("real"),
        sail_parser::Token::String(_) => Some("string"),
        sail_parser::Token::KwTrue | sail_parser::Token::KwFalse => Some("bool"),
        sail_parser::Token::Bin(_) | sail_parser::Token::Hex(_) => Some("bits"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::File;
    use tower_lsp::lsp_types::Position;

    #[test]
    fn parses_multiline_val_signature() {
        let file = File::new("val f : int ->\n  bool -> string\n".to_string());
        let signatures = collect_callable_signatures(&file);
        let sig = signatures.iter().find(|sig| sig.name == "f").unwrap();
        assert_eq!(sig.params.len(), 2);
        assert_eq!(sig.return_type.as_deref(), Some("string"));
    }

    #[test]
    fn ignores_parentheses_inside_string_for_call_detection() {
        let source = r#"let _ = foo("(", 3)"#.to_string();
        let file = File::new(source);
        let call = find_call_at_position(&file, Position::new(0, 17)).unwrap();
        assert_eq!(call.0, "foo");
        assert_eq!(call.1, 1);
    }
}
