use crate::state::File;
use sail_parser::Span;
use std::collections::HashMap;
use tower_lsp::lsp_types::{Location, Range, SymbolKind, Url};

#[derive(Clone)]
pub(crate) struct SymbolDecl {
    pub(crate) name: String,
    pub(crate) kind: SymbolKind,
    pub(crate) detail: &'static str,
    pub(crate) offset: usize,
}

#[derive(Clone)]
pub(crate) struct Parameter {
    pub(crate) name: String,
    pub(crate) is_implicit: bool,
}

#[derive(Clone)]
pub(crate) struct CallableSignature {
    pub(crate) name: String,
    pub(crate) label: String,
    pub(crate) params: Vec<Parameter>,
    pub(crate) return_type: Option<String>,
}

pub(crate) fn token_symbol_key(token: &sail_parser::Token) -> Option<String> {
    match token {
        sail_parser::Token::Id(name) => Some(name.clone()),
        sail_parser::Token::TyVal(name) => Some(format!("'{}", name)),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn add_definitions(
    tokens: &[(sail_parser::Token, Span)],
    definitions: &mut HashMap<String, usize>,
) {
    let parsed = sail_parser::parse_core_source(tokens)
        .into_output()
        .map(|ast| sail_parser::ParsedFile::from_core_ast(&ast))
        .unwrap_or_else(|| sail_parser::parse_tokens(tokens));
    add_parsed_definitions(&parsed, definitions);
}

pub(crate) fn add_parsed_definitions(
    parsed: &sail_parser::ParsedFile,
    definitions: &mut HashMap<String, usize>,
) {
    for decl in &parsed.decls {
        if decl.role != sail_parser::DeclRole::Definition {
            continue;
        }
        let include = match decl.kind {
            sail_parser::DeclKind::Let
            | sail_parser::DeclKind::Var
            | sail_parser::DeclKind::Parameter => decl.scope == sail_parser::Scope::TopLevel,
            _ => true,
        };
        if !include {
            continue;
        }

        definitions
            .entry(decl.name.clone())
            .or_insert(decl.span.start);
        if decl.kind == sail_parser::DeclKind::Bitfield {
            definitions
                .entry(format!("Mk_{}", decl.name))
                .or_insert(decl.span.start);
        }
    }
}

pub(crate) fn extract_symbol_decls(file: &File) -> Vec<SymbolDecl> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };

    parsed
        .decls
        .iter()
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
                sail_parser::DeclKind::Parameter => (SymbolKind::VARIABLE, "parameter"),
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
                name: decl.name.clone(),
                kind,
                detail,
                offset: decl.span.start,
            })
        })
        .collect()
}

pub(crate) fn range_from_span(file: &File, span: Span) -> Range {
    Range::new(
        file.source.position_at(span.start),
        file.source.position_at(span.end),
    )
}

pub(crate) fn location_from_span(uri: &Url, file: &File, span: Span) -> Location {
    Location::new(uri.clone(), range_from_span(file, span))
}

pub(crate) fn builtin_docs(name: &str) -> Option<&'static str> {
    match name {
        "bits" => Some("`bits('n)` is a bitvector of length `'n`. It is one of the most fundamental types in Sail."),
        "int" => Some("`int` is an arbitrary-precision integer."),
        "nat" => Some("`nat` is a non-negative arbitrary-precision integer."),
        "bool" => Some("`bool` is a boolean type with values `true` and `false`."),
        "unit" => Some("`unit` is the type with a single value `()`, similar to `void` in C or `()` in Rust."),
        "string" => Some("`string` is a sequence of characters."),
        "real" => Some("`real` is a real number type."),
        "forall" => Some("`forall` is used to introduce type variables in a declaration."),
        "scattered" => Some("`scattered` allows defining a function or union across multiple files or locations."),
        "overload" => Some("`overload` defines a common name for multiple functions or operators."),
        "register" => Some("`register` declares a piece of processor state."),
        "mapping" => Some("`mapping` defines a bidirectional translation between types (e.g., for assembly formatting)."),
        _ => None,
    }
}

pub(crate) fn extract_comments(text: &str, offset: usize) -> Option<String> {
    let mut lines = Vec::new();
    let mut current_offset = offset;

    // Move to the beginning of the line containing the offset
    while current_offset > 0 && text.as_bytes()[current_offset - 1] != b'\n' {
        current_offset -= 1;
    }

    while current_offset > 0 {
        // Skip whitespace and move to the end of the previous line
        while current_offset > 0 && text.as_bytes()[current_offset - 1].is_ascii_whitespace() {
            current_offset -= 1;
        }

        let line_end = current_offset;
        while current_offset > 0 && text.as_bytes()[current_offset - 1] != b'\n' {
            current_offset -= 1;
        }

        let line = text[current_offset..line_end].trim();
        if line.starts_with("//") {
            let comment_text = line.trim_start_matches('/').trim();
            lines.push(comment_text.to_string());
        } else if line.is_empty() {
            // Keep looking through empty lines
            continue;
        } else {
            // Not a comment line, stop searching
            break;
        }
    }

    if lines.is_empty() {
        None
    } else {
        lines.reverse();
        Some(lines.join("\n"))
    }
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

fn token_span_text(text: &str, begin: usize, end: usize) -> String {
    text[begin..end].trim().to_string()
}

fn span_text(text: &str, span: sail_parser::Span) -> String {
    token_span_text(text, span.start, span.end)
}

pub(crate) fn collect_callable_signatures(file: &File) -> Vec<CallableSignature> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };
    let text = file.source.text();
    let mut out = Vec::new();
    for head in &parsed.callable_heads {
        if !matches!(
            head.kind,
            sail_parser::DeclKind::Function
                | sail_parser::DeclKind::Value
                | sail_parser::DeclKind::Mapping
        ) {
            continue;
        }

        let label = span_text(text, head.label_span);
        let params = head
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let ty_text = param.ty_span.map(|span| span_text(text, span));
                let name = match (param.name.as_deref(), ty_text.as_deref()) {
                    (Some(name), Some(ty)) => format!("{name} : {ty}"),
                    (Some(name), None) => name.to_string(),
                    (None, Some(ty)) => format!("arg{}: {}", idx + 1, ty),
                    (None, None) => format!("arg{}", idx + 1),
                };
                Parameter {
                    name,
                    is_implicit: span_text(text, param.span).contains("implicit"),
                }
            })
            .collect::<Vec<_>>();
        let return_type = head.return_type_span.map(|span| span_text(text, span));
        out.push(CallableSignature {
            name: head.name.clone(),
            label,
            params,
            return_type,
        });
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
            let mut score = match (uri.path_segments(), candidate_uri.path_segments()) {
                (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
                _ => 0,
            } * 10;
            if sig.label.starts_with("val") {
                score += 5;
            }
            if sig.label.contains("->") {
                score += 2;
            }
            if sig.label.contains("forall") {
                score += 1;
            }
            match &best {
                Some((best_score, _)) if *best_score > score => {}
                _ => best = Some((score, sig)),
            }
        }
    }
    best.map(|(_, sig)| sig)
}

pub(crate) fn instantiate_signature(
    sig: &CallableSignature,
    arg_types: &[Option<String>],
) -> String {
    let mut subst = std::collections::HashMap::new();

    for (param, arg_ty) in sig.params.iter().zip(arg_types.iter()) {
        let param_name = &param.name;
        let Some(arg_ty) = arg_ty else { continue };

        // Generic unification for bits('n) and bits(32)
        // We look for 'tick followed by name
        if let Some(n_start) = param_name.find("'") {
            let var_name = &param_name[n_start..];
            let v_end = var_name[1..]
                .find(|c: char| !c.is_alphanumeric() && c != '_')
                .map(|i| i + 1)
                .unwrap_or(var_name.len());
            let var = &var_name[..v_end];

            if !var.is_empty() {
                if arg_ty.starts_with("bits(") {
                    let actual = &arg_ty[5..arg_ty.len() - 1];
                    subst.insert(var.to_string(), actual.to_string());
                } else if arg_ty == "int" || arg_ty == "bool" {
                    subst.insert(var.to_string(), arg_ty.clone());
                }
            }
        }
    }

    let mut label = sig.label.clone();
    for (var, val) in subst {
        label = label.replace(&var, &val);
    }
    label
}

fn snippet_escape(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('$', "\\$")
        .replace('}', "\\}")
}

pub(crate) fn function_snippet(name: &str, params: &[Parameter]) -> String {
    if params.is_empty() {
        return format!("{name}()");
    }
    let body = params
        .iter()
        .enumerate()
        .map(|(idx, param)| {
            let placeholder = param
                .name
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

#[cfg(test)]
pub(crate) fn infer_binding_type(token: &sail_parser::Token) -> Option<&'static str> {
    match token {
        sail_parser::Token::Num(_) => Some("int"),
        sail_parser::Token::Real(_) => Some("real"),
        sail_parser::Token::String(_) => Some("string"),
        sail_parser::Token::KwTrue | sail_parser::Token::KwFalse => Some("bool"),
        sail_parser::Token::Bin(s) => {
            let len = s.trim_start_matches("0b").len();
            match len {
                0..=8 => Some("bits(8)"),
                9..=16 => Some("bits(16)"),
                17..=32 => Some("bits(32)"),
                33..=64 => Some("bits(64)"),
                _ => Some("bits"),
            }
        }
        sail_parser::Token::Hex(s) => {
            let len = s.trim_start_matches("0x").len();
            match len {
                0..=2 => Some("bits(8)"),
                3..=4 => Some("bits(16)"),
                5..=8 => Some("bits(32)"),
                9..=16 => Some("bits(64)"),
                _ => Some("bits"),
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::File;
    use crate::symbols::find_call_at_position;
    use chumsky::Parser;
    use std::collections::HashMap;
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

    #[test]
    fn captures_only_top_level_bindings() {
        let source = r#"
function foo() = {
  let x = 1;
  var y = 2;
  x + y
}
let z = 3
"#;
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let mut definitions = HashMap::new();
        add_definitions(&tokens, &mut definitions);

        assert!(definitions.contains_key("foo"));
        assert!(definitions.contains_key("z"));
        assert!(!definitions.contains_key("x"));
        assert!(!definitions.contains_key("y"));
    }

    #[test]
    fn excludes_value_declarations_from_definitions() {
        let source = "val foo : int -> int\nfunction foo(x) = x";
        let tokens = sail_parser::lexer().parse(source).into_result().unwrap();
        let mut definitions = HashMap::new();
        add_definitions(&tokens, &mut definitions);
        assert!(definitions.contains_key("foo"));
    }
}
