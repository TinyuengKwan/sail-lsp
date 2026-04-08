use crate::state::File;
use sail_parser::Span;
use std::collections::HashMap;
use tower_lsp::lsp_types::{DocumentSymbol, Location, Range, SymbolKind, Url};

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
    let Some(parsed) = sail_parser::parse_core_source(tokens)
        .into_output()
        .map(|ast| sail_parser::ParsedFile::from_core_ast(&ast))
    else {
        return;
    };
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

/// Build a hierarchical DocumentSymbol tree. Enum members become children of
/// their parent enum; all other top-level decls are roots.
#[allow(deprecated)] // DocumentSymbol.deprecated is deprecated in the LSP type
pub(crate) fn document_symbol_tree(file: &File) -> Vec<DocumentSymbol> {
    let Some(parsed) = file.parsed() else {
        return Vec::new();
    };

    // First pass: collect top-level items and their full spans.
    let item_spans: Vec<(usize, usize)> = if let Some(ast) = file.core_ast() {
        ast.defs
            .iter()
            .map(|(_, span)| (span.start, span.end))
            .collect()
    } else {
        Vec::new()
    };

    let mut roots: Vec<DocumentSymbol> = Vec::new();
    // Track enum positions for parenting members.
    let mut enum_indices: HashMap<String, usize> = HashMap::new();

    for decl in &parsed.decls {
        if decl.scope == sail_parser::Scope::Local
            && !matches!(decl.kind, sail_parser::DeclKind::EnumMember)
        {
            continue;
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

        let selection_range = Range::new(
            file.source.position_at(decl.span.start),
            file.source.position_at(decl.span.start + decl.name.len()),
        );

        // Find the full range from the core_ast item span (if available).
        let full_range = item_spans
            .iter()
            .find(|(s, e)| *s <= decl.span.start && decl.span.end <= *e)
            .map(|(s, e)| {
                Range::new(file.source.position_at(*s), file.source.position_at(*e))
            })
            .unwrap_or(selection_range);

        let symbol = DocumentSymbol {
            name: decl.name.clone(),
            detail: Some(detail.to_string()),
            kind,
            tags: None,
            deprecated: None,
            range: full_range,
            selection_range,
            children: Some(Vec::new()),
        };

        if decl.kind == sail_parser::DeclKind::EnumMember {
            // Try to parent under the most recent enum.
            let parented = enum_indices
                .values()
                .copied()
                .max()
                .and_then(|idx| roots.get_mut(idx))
                .and_then(|parent| parent.children.as_mut());
            if let Some(children) = parented {
                children.push(symbol);
                continue;
            }
        }

        if decl.kind == sail_parser::DeclKind::Enum {
            enum_indices.insert(decl.name.clone(), roots.len());
        }
        roots.push(symbol);
    }

    roots
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
        "slice" => Some("`slice(xs, start, len)` returns a window of length `len` starting at `start` from a vector or bitvector."),
        "vector_access#" => Some("Internal parser rewrite for `xs[i]`. Reads one element or bitfield field from a vector-like value."),
        "vector_subrange#" => Some("Internal parser rewrite for `xs[hi .. lo]`. Extracts a subrange from a vector or bitvector."),
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

        // Check for block comment ending with */
        if line_end >= 2 && &text[line_end - 2..line_end] == "*/" {
            // Walk backwards to find the matching /*
            let block_end = line_end;
            let mut search = line_end - 2;
            let mut found_start = None;
            while search >= 2 {
                if &text[search - 2..search] == "/*" {
                    found_start = Some(search - 2);
                    break;
                }
                search -= 1;
                if search == 0 {
                    break;
                }
            }
            if search == 0 && text.starts_with("/*") {
                found_start = Some(0);
            }

            if let Some(start) = found_start {
                let block = &text[start..block_end];
                // Strip the comment markers
                let inner = if block.starts_with("/*!") {
                    // Doc block comment: strip /*! and */
                    &block[3..block.len() - 2]
                } else {
                    // Regular block comment: strip /* and */
                    &block[2..block.len() - 2]
                };

                // Process each line of the block comment
                let mut block_lines: Vec<String> = Vec::new();
                for comment_line in inner.lines() {
                    let trimmed = comment_line.trim();
                    // Strip leading * that is common in block comment style
                    let trimmed = trimmed.strip_prefix('*').map(|s| s.trim()).unwrap_or(trimmed);
                    block_lines.push(trimmed.to_string());
                }

                // Remove leading/trailing empty lines
                while block_lines.first().is_some_and(|l| l.is_empty()) {
                    block_lines.remove(0);
                }
                while block_lines.last().is_some_and(|l| l.is_empty()) {
                    block_lines.pop();
                }

                // Prepend block comment lines before any line comments already collected
                if !block_lines.is_empty() {
                    block_lines.extend(lines.into_iter().rev());
                    lines = block_lines;
                    lines.reverse();
                }

                current_offset = start;
                // Continue looking for more comments above
                // Move to the beginning of the line containing the block start
                while current_offset > 0 && text.as_bytes()[current_offset - 1] != b'\n' {
                    current_offset -= 1;
                }
                continue;
            }
        }

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

fn span_text<'a>(text: &'a str, span: sail_parser::Span) -> &'a str {
    text[span.start..span.end].trim()
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

        let label = span_text(text, head.label_span).to_string();
        let params = head
            .params
            .iter()
            .enumerate()
            .map(|(idx, param)| {
                let ty_text = param.ty_span.map(|span| span_text(text, span).to_string());
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
        let return_type = head.return_type_span.map(|span| span_text(text, span).to_string());
        out.push(CallableSignature {
            name: head.name.clone(),
            label,
            params,
            return_type,
        });
    }

    out
}

/// Build a name→signature index for a single file (called once at parse time).
pub(crate) fn build_signature_index(file: &File) -> HashMap<String, CallableSignature> {
    let sigs = collect_callable_signatures(file);
    let mut index = HashMap::with_capacity(sigs.len());
    for sig in sigs {
        // Prefer val specs (richer info) over plain function defs.
        let dominated = index.get(&sig.name).is_some_and(|existing: &CallableSignature| {
            existing.label.starts_with("val") && !sig.label.starts_with("val")
        });
        if !dominated {
            index.insert(sig.name.clone(), sig);
        }
    }
    index
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
        let Some(sig) = candidate_file.signature_index.get(name) else {
            continue;
        };
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
            _ => best = Some((score, sig.clone())),
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
