pub(crate) mod support;

use self::support::{binding_type_hint, infer_call_arg_types_at_position};
use crate::state::File;
use crate::symbols::{
    builtin_docs, extract_comments,
    find_call_at_position, find_callable_signature, instantiate_signature, token_is_close_bracket,
    token_is_open_bracket,
};
use sail_parser::{
    find_enum_name_for_member, find_named_members, find_top_level_item_span, Decl, DeclKind,
    DeclRole, NamedDefKind, Scope, Token,
};
use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position, Range, SymbolKind, Url,
};

pub(crate) use support::infer_expr_type_text;

pub(crate) fn hover_for_symbol<'a, I>(
    files: I,
    current_uri: &Url,
    current_file: &File,
    position: Position,
    hover_range: Range,
    symbol_key: &str,
) -> Option<Hover>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    if symbol_key.starts_with('\'') {
        return Some(markdown_hover(fenced_sail(symbol_key), hover_range));
    }

    let files = files.into_iter().collect::<Vec<_>>();
    let current_offset = current_file.source.offset_at(&position);

    // RA-style: Builtin docs
    if let Some(doc) = builtin_docs(symbol_key) {
        let mut markdown = vec![format!("**builtin** **{symbol_key}**")];
        markdown.push("___".to_string());
        markdown.push(doc.to_string());
        return Some(markdown_hover(markdown.join("\n\n"), hover_range));
    }

    let decl_ref = resolve_decl(
        files.iter().copied(),
        current_uri,
        symbol_key,
        Some(current_offset),
    );

    let mut markdown = Vec::new();

    if let Some(decl_ref) = decl_ref.as_ref() {
        let label = decl_kind_label(decl_ref.decl.kind);
        let name = &decl_ref.decl.name;

        // Item: Counts in Header (Plagiarism from RA)
        let refs = files.iter().map(|(_, f)| f.ref_counts.get(name).copied().unwrap_or(0)).sum::<usize>();
        let mut header = format!("**{label}** **{name}**");
        header.push_str(&format!(
            "  *({refs} {})*",
            if refs == 1 { "ref" } else { "refs" }
        ));

        if symbol_kind_for_decl(decl_ref.decl.kind) == SymbolKind::FUNCTION {
            let impls = files.iter().map(|(_, f)| f.impl_counts.get(name).copied().unwrap_or(0)).sum::<usize>();
            header.push_str(&format!(
                " • *({impls} {})*",
                if impls == 1 { "impl" } else { "impls" }
            ));
        }
        markdown.push(header);

        let mut headline =
            render_signature(decl_ref, files.iter().copied(), current_uri, symbol_key);

        // Item 2: Generic Substitution / Instantiation
        if symbol_kind_for_decl(decl_ref.decl.kind) == SymbolKind::FUNCTION {
            let offset = current_file.source.offset_at(&position);
            let is_at_definition = decl_ref.decl.role == DeclRole::Definition
                && decl_ref.decl.span.start <= offset
                && offset <= decl_ref.decl.span.end;

            if !is_at_definition {
                if let Some((callee, _)) = find_call_at_position(current_file, position) {
                    if callee == *name {
                        if let Some(sig) =
                            find_callable_signature(files.iter().copied(), current_uri, name)
                        {
                            if let Some(arg_types) = infer_call_arg_types_at_position(
                                &files,
                                current_uri,
                                current_file,
                                position,
                                name,
                            ) {
                                let instantiated = instantiate_signature(&sig, &arg_types);
                                if instantiated != headline {
                                    markdown.push("___".to_string());
                                    markdown.push(format!("*instantiated as:*"));
                                    headline = instantiated;
                                }
                            }
                        }
                    }
                }
            }
        }

        markdown.push("___".to_string());
        markdown.push(fenced_sail(&headline));

        // Overload inspection
        if decl_ref.decl.kind == DeclKind::Overload {
            let members = overload_members(decl_ref.file, &decl_ref.decl);
            if !members.is_empty() {
                markdown.push("___".to_string());
                markdown.push(format!("**members:**"));
                for member in members {
                    if let Some(sig) =
                        find_callable_signature(files.iter().copied(), current_uri, &member)
                    {
                        markdown.push(fenced_sail(&sig.label));
                    } else {
                        markdown.push(format!("- `{member}`"));
                    }
                }
            }
        }

        // Effect tracking: show side effects of functions
        if matches!(
            decl_ref.decl.kind,
            DeclKind::Function | DeclKind::Value | DeclKind::Mapping
        ) {
            if let Some(ast) = decl_ref.file.core_ast() {
                let effects = infer_effects_for_def(ast, &decl_ref.decl.name);
                if !effects.is_empty() {
                    markdown.push("___".to_string());
                    markdown.push(format!("**effects:** {}", effects.join(", ")));
                } else {
                    markdown.push("___".to_string());
                    markdown.push("**effects:** *pure*".to_string());
                }
            }
        }

        if let Some(comments) =
            extract_comments(decl_ref.file.source.text(), decl_ref.decl.span.start)
        {
            markdown.push("___".to_string());
            markdown.push(comments);
        }

        // Constant folding: show computed value for let/var bindings
        if matches!(decl_ref.decl.kind, DeclKind::Let | DeclKind::Var) {
            if let Some(ast) = decl_ref.file.core_ast() {
                if let Some(value_span) = find_binding_value_span(ast, &decl_ref.decl.name, decl_ref.decl.span) {
                    let value_text = decl_ref.file.source.text().get(value_span.start..value_span.end);
                    if let Some(value_text) = value_text {
                        if let Some(folded) = crate::actions::try_fold_constant(value_text) {
                            markdown.push("___".to_string());
                            markdown.push(format!("**value:** `{folded}`"));
                        }
                    }
                }
            }
        }

        // Show path like RA (using simple relative path or filename)
        let path = decl_ref.uri.path();
        let filename = path.split('/').last().unwrap_or(path);

        // Build navigation link (RA-style)
        let pos = decl_ref.file.source.position_at(decl_ref.decl.span.start);
        let link = format!("{}#L{},{}", decl_ref.uri, pos.line + 1, pos.character + 1);

        markdown.push("___".to_string());
        markdown.push(format!("[Go to Definition]({}) • *in {}*", link, filename));
    } else {
        markdown.push(fenced_sail(symbol_key));
    }

    let markup = markdown.join("\n\n");
    Some(markdown_hover(markup, hover_range))
}

fn overload_members(file: &File, decl: &Decl) -> Vec<String> {
    if let Some(ast) = file.core_ast() {
        if let Some(members) = find_named_members(ast, NamedDefKind::Overload, decl.span) {
            return members.iter().map(|member| member.0.clone()).collect();
        }
    }

    let Some(tokens) = file.tokens.as_deref() else {
        return Vec::new();
    };
    let Some(idx) = tokens
        .iter()
        .position(|(_, span)| span.start == decl.span.start)
    else {
        return Vec::new();
    };

    let mut members = Vec::new();
    let mut j = idx;
    while j < tokens.len() && tokens[j].0 != Token::Equal {
        j += 1;
    }
    j += 1; // skip '='

    let mut depth = 0;
    while j < tokens.len() {
        let (token, _) = &tokens[j];
        match token {
            Token::LeftCurlyBracket => depth += 1,
            Token::RightCurlyBracket => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Token::Id(name) if depth == 1 => members.push(name.clone()),
            Token::Comma if depth == 1 => {}
            _ if depth == 0 && token_starts_declaration(token) => break,
            _ => {}
        }
        j += 1;
    }
    members
}

fn markdown_hover(markdown: String, range: Range) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: Some(range),
    }
}

fn fenced_sail(text: &str) -> String {
    format!("```sail\n{text}\n```")
}

fn decl_kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Function => "function",
        DeclKind::Value => "value",
        DeclKind::Mapping => "mapping",
        DeclKind::Overload => "overload",
        DeclKind::Register => "register",
        DeclKind::Parameter => "parameter",
        DeclKind::Type => "type",
        DeclKind::Struct => "struct",
        DeclKind::Union => "union",
        DeclKind::Bitfield => "bitfield",
        DeclKind::Enum => "enum",
        DeclKind::EnumMember => "enum member",
        DeclKind::Newtype => "newtype",
        DeclKind::Let => "let binding",
        DeclKind::Var => "var binding",
    }
}

fn symbol_kind_for_decl(kind: DeclKind) -> SymbolKind {
    match kind {
        DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload => {
            SymbolKind::FUNCTION
        }
        DeclKind::Register | DeclKind::Parameter | DeclKind::Let | DeclKind::Var => {
            SymbolKind::VARIABLE
        }
        DeclKind::Enum => SymbolKind::ENUM,
        DeclKind::EnumMember => SymbolKind::ENUM_MEMBER,
        DeclKind::Type
        | DeclKind::Struct
        | DeclKind::Union
        | DeclKind::Bitfield
        | DeclKind::Newtype => SymbolKind::STRUCT,
    }
}

fn token_starts_declaration(token: &Token) -> bool {
    matches!(
        token,
        Token::KwFunction
            | Token::KwVal
            | Token::KwMapping
            | Token::KwOverload
            | Token::KwRegister
            | Token::KwType
            | Token::KwStruct
            | Token::KwUnion
            | Token::KwBitfield
            | Token::KwNewtype
            | Token::KwEnum
            | Token::KwLet
            | Token::KwVar
            | Token::KwScattered
            | Token::Directive { .. }
            | Token::StructuredDirectiveStart(_)
    )
}

fn decl_headline(file: &File, decl: &Decl) -> String {
    if decl.scope == Scope::TopLevel {
        if let Some(ast) = file.core_ast() {
            if let Some(span) = find_top_level_item_span(ast, decl.span) {
                return file.source.text()[span.start..span.end].trim().to_string();
            }
        }
    }

    let Some(tokens) = file.tokens.as_deref() else {
        return format!("{} {}", decl_kind_label(decl.kind), decl.name);
    };
    let Some(idx) = tokens
        .iter()
        .position(|(_, span)| span.start == decl.span.start)
    else {
        return format!("{} {}", decl_kind_label(decl.kind), decl.name);
    };
    let text = file.source.text();
    let mut start_idx = idx;
    while start_idx > 0 {
        let token = &tokens[start_idx - 1].0;
        if token_starts_declaration(token) {
            start_idx -= 1;
            break;
        }
        start_idx -= 1;
    }
    let mut end_idx = idx;
    let mut depth = 0_usize;
    while end_idx + 1 < tokens.len() {
        let token = &tokens[end_idx + 1].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth = depth.saturating_sub(1);
        } else if depth == 0 && token_starts_declaration(token) {
            break;
        }
        end_idx += 1;
    }
    text[tokens[start_idx].1.start..tokens[end_idx].1.end]
        .trim()
        .to_string()
}

fn render_signature<'a, I>(
    decl_ref: &DeclRef<'a>,
    files: I,
    current_uri: &Url,
    symbol_key: &str,
) -> String
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let files = files.into_iter().collect::<Vec<_>>();
    if matches!(
        decl_ref.decl.kind,
        DeclKind::Parameter | DeclKind::Let | DeclKind::Var
    ) {
        if let Some(ty) = binding_type_hint(&files, current_uri, decl_ref.file, &decl_ref.decl) {
            let kw = match decl_ref.decl.kind {
                DeclKind::Parameter => "parameter",
                DeclKind::Var => "var",
                _ => "let",
            };
            return format!("{kw} {} : {ty}", decl_ref.decl.name);
        }
    }

    if decl_ref.decl.kind == DeclKind::EnumMember {
        if let Some(EnumInfo::Member { enum_name, .. }) =
            enum_info_for_symbol(decl_ref.file, symbol_key)
        {
            return format!("{enum_name}::{symbol_key}");
        }
    }

    decl_headline(decl_ref.file, &decl_ref.decl)
}

struct DeclRef<'a> {
    uri: &'a Url,
    file: &'a File,
    decl: Decl,
}

fn resolve_decl<'a, I>(
    files: I,
    current_uri: &Url,
    symbol_key: &str,
    current_offset: Option<usize>,
) -> Option<DeclRef<'a>>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let files = files.into_iter().collect::<Vec<_>>();

    if let Some(offset) = current_offset {
        for (uri, file) in files.iter().copied() {
            if uri != current_uri {
                continue;
            }
            let Some(parsed) = file.parsed() else {
                continue;
            };
            if let Some(decl) = parsed
                .decls
                .iter()
                .filter(|decl| {
                    decl.name == symbol_key
                        && decl.scope == Scope::Local
                        && decl.span.start <= offset
                })
                .max_by_key(|decl| decl.span.start)
            {
                return Some(DeclRef {
                    uri,
                    file,
                    decl: decl.clone(),
                });
            }
        }
    }

    let mut best: Option<(usize, DeclRef<'a>)> = None;
    for (uri, file) in files {
        let Some(parsed) = file.parsed() else {
            continue;
        };
        for decl in parsed.decls.iter().filter(|decl| {
            decl.name == symbol_key
                && (decl.scope == Scope::TopLevel || decl.kind == DeclKind::EnumMember)
        }) {
            let mut score = uri_prefix_score(current_uri, uri) * 16;
            if uri == current_uri {
                score += 8;
            }
            if decl.role == DeclRole::Definition {
                score += 4;
            }
            score += match symbol_kind_for_decl(decl.kind) {
                SymbolKind::ENUM_MEMBER => 2,
                SymbolKind::FUNCTION => 1,
                _ => 0,
            };
            match &best {
                Some((best_score, _)) if *best_score > score => {}
                _ => {
                    best = Some((
                        score,
                        DeclRef {
                            uri,
                            file,
                            decl: decl.clone(),
                        },
                    ))
                }
            }
        }
    }
    best.map(|(_, decl_ref)| decl_ref)
}

fn uri_prefix_score(lhs: &Url, rhs: &Url) -> usize {
    match (lhs.path_segments(), rhs.path_segments()) {
        (Some(a), Some(b)) => a.zip(b).take_while(|(x, y)| x == y).count(),
        _ => 0,
    }
}

enum EnumInfo {
    Member { enum_name: String },
}

fn enum_info_for_symbol(file: &File, symbol: &str) -> Option<EnumInfo> {
    if let Some(ast) = file.core_ast() {
        if let Some(enum_name) = find_enum_name_for_member(ast, symbol) {
            return Some(EnumInfo::Member {
                enum_name: enum_name.to_string(),
            });
        }
    }

    let tokens = file.tokens.as_deref()?;
    let mut i = 0usize;

    while i + 1 < tokens.len() {
        if tokens[i].0 != Token::KwEnum {
            i += 1;
            continue;
        }
        let Token::Id(enum_name) = &tokens[i + 1].0 else {
            i += 1;
            continue;
        };

        let mut j = i + 2;
        while j < tokens.len() && tokens[j].0 != Token::LeftCurlyBracket {
            if token_starts_declaration(&tokens[j].0) {
                break;
            }
            j += 1;
        }
        if j >= tokens.len() || tokens[j].0 != Token::LeftCurlyBracket {
            i += 1;
            continue;
        }

        let mut members = Vec::<String>::new();
        let mut depth = 1_i32;
        j += 1;
        while j < tokens.len() && depth > 0 {
            match &tokens[j].0 {
                Token::LeftCurlyBracket => depth += 1,
                Token::RightCurlyBracket => depth -= 1,
                Token::Id(name) if depth == 1 => members.push(name.clone()),
                _ => {}
            }
            j += 1;
        }

        if members.iter().any(|member| member == symbol) {
            return Some(EnumInfo::Member {
                enum_name: enum_name.clone(),
            });
        }

        i = j;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::{Range, Url};

    fn hover_markdown(hover: Hover) -> String {
        match hover.contents {
            HoverContents::Markup(content) => content.value,
            HoverContents::Array(_) | HoverContents::Scalar(_) => String::new(),
        }
    }

    #[test]
    fn shows_function_signature_and_location() {
        let source = "val add : int -> int\nfunction add(x) = x\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.find("add(x)").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(
                pos,
                file.source.position_at(source.find("add(x)").unwrap() + 3),
            ),
            "add",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**function** **add**"));
        assert!(markdown.contains("```sail\nfunction add(x) = x\n```"));
        assert!(markdown.contains("*in main.sail*"));
    }

    #[test]
    fn shows_local_binding_type_hint() {
        let source = "function foo() = {\n  let x : bits(32) = 1;\n  x\n}\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.rfind("x").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(pos, file.source.position_at(source.rfind("x").unwrap() + 1)),
            "x",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**let binding** **x**"));
        assert!(markdown.contains("```sail\nlet x : bits(32)\n```"));
    }

    #[test]
    fn shows_inferred_local_binding_type_hint() {
        let source = "function foo() = {\n  let x = 1;\n  x\n}\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.rfind("x").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(pos, file.source.position_at(source.rfind("x").unwrap() + 1)),
            "x",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**let binding** **x**"));
        assert!(markdown.contains("```sail\nlet x : int\n```"));
    }

    #[test]
    fn shows_parameter_type_hint() {
        let source = "function foo(x : bits(32)) = x\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.rfind("x").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(pos, file.source.position_at(source.rfind("x").unwrap() + 1)),
            "x",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**parameter** **x**"));
        assert!(markdown.contains("```sail\nparameter x : bits(32)\n```"));
    }

    #[test]
    fn shows_enum_member_context() {
        let source = "enum color = { Red, Green, Blue }\nlet x = Red\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.rfind("Red").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(
                pos,
                file.source.position_at(source.rfind("Red").unwrap() + 3),
            ),
            "Red",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**enum member** **Red**"));
        assert!(markdown.contains("```sail\ncolor::Red\n```"));
    }

    #[test]
    fn shows_overload_members() {
        let source = r#"
val add : int -> int
function add(x) = x
val sub : int -> int
function sub(x) = x
overload op = {add, sub}
"#
        .to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let start = source.find("op =").unwrap();
        let pos = file.source.position_at(start);

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(pos, file.source.position_at(start + 2)),
            "op",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("**overload** **op**"));
        assert!(markdown.contains("**members:**"));
        assert!(markdown.contains("```sail\nval add : int -> int\n```"));
        assert!(markdown.contains("```sail\nval sub : int -> int\n```"));
    }

    #[test]
    fn shows_type_variable_hover() {
        let file = File::new("val f : bits('n) -> bits('n)\n".to_string());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = Position::new(0, 0);
        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(pos, Position::new(0, 2)),
            "'n",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert_eq!(markdown.trim(), "```sail\n'n\n```");
    }

    #[test]
    fn shows_comments_in_hover() {
        let source =
            "// This is a comment\n// for the add function\nfunction add(x) = x\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.find("add(x)").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(
                pos,
                file.source.position_at(source.find("add(x)").unwrap() + 3),
            ),
            "add",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("This is a comment\nfor the add function"));
    }

    #[test]
    fn returns_precise_hover_range_for_identifier() {
        let source = "val add : int -> int\nfunction add(x) = x\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let name_offset = source.rfind("add").unwrap();
        let pos = file.source.position_at(name_offset + 1);
        let hover_range = Range::new(
            file.source.position_at(name_offset),
            file.source.position_at(name_offset + 3),
        );

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            hover_range,
            "add",
        )
        .expect("hover");

        assert_eq!(hover.range, Some(hover_range));
    }

    #[test]
    fn shows_instantiated_signature_in_hover() {
        let source =
            "val f : bits('n) -> bits('n)\nfunction f(x) = x\nlet _ = f(0xDEADBEEF)\n".to_string();
        let file = File::new(source.clone());
        let uri = Url::parse("file:///tmp/main.sail").unwrap();
        let pos = file.source.position_at(source.rfind("f(0x").unwrap());

        let hover = hover_for_symbol(
            std::iter::once((&uri, &file)),
            &uri,
            &file,
            pos,
            Range::new(
                pos,
                file.source.position_at(source.rfind("f(0x").unwrap() + 1),
            ),
            "f",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("instantiated as"));
        assert!(markdown.contains("bits(32) -> bits(32)"));
    }
}

// ---------------------------------------------------------------------------
// Effect Tracking
// ---------------------------------------------------------------------------

fn find_binding_value_span(
    ast: &sail_parser::core_ast::SourceFile,
    name: &str,
    decl_span: sail_parser::Span,
) -> Option<sail_parser::Span> {
    use sail_parser::core_ast::{BlockItem, DefinitionKind, Expr, Pattern};

    fn search_expr(
        expr: &(Expr, sail_parser::Span),
        name: &str,
        decl_span: sail_parser::Span,
    ) -> Option<sail_parser::Span> {
        match &expr.0 {
            Expr::Let { binding, body } => {
                if matches!(&binding.pattern.0, Pattern::Ident(n) if n == name)
                    && binding.pattern.1.start == decl_span.start
                {
                    return Some(binding.value.1);
                }
                search_expr(body, name, decl_span)
            }
            Expr::Block(items) => {
                for item in items {
                    match &item.0 {
                        BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                            if let Some(r) = search_expr(e, name, decl_span) {
                                return Some(r);
                            }
                        }
                        BlockItem::Let(lb) => {
                            if matches!(&lb.pattern.0, Pattern::Ident(n) if n == name)
                                && lb.pattern.1.start == decl_span.start
                            {
                                return Some(lb.value.1);
                            }
                            if let Some(r) = search_expr(&*lb.value, name, decl_span) {
                                return Some(r);
                            }
                        }
                    }
                }
                None
            }
            Expr::If { then_branch, else_branch, .. } => {
                search_expr(then_branch, name, decl_span)
                    .or_else(|| else_branch.as_ref().and_then(|e| search_expr(e, name, decl_span)))
            }
            Expr::Var { body, .. } => search_expr(body, name, decl_span),
            Expr::Match { cases, .. } | Expr::Try { cases, .. } => {
                cases.iter().find_map(|(c, _)| search_expr(&c.body, name, decl_span))
            }
            Expr::Foreach(f) => search_expr(&f.body, name, decl_span),
            Expr::While { body, .. } | Expr::Repeat { body, .. } => search_expr(body, name, decl_span),
            _ => None,
        }
    }

    for (def, _) in &ast.defs {
        match &def.kind {
            DefinitionKind::Callable(c) => {
                if let Some(body) = &c.body {
                    if let Some(r) = search_expr(body, name, decl_span) {
                        return Some(r);
                    }
                }
                for (clause, _) in &c.clauses {
                    if let Some(body) = &clause.body {
                        if let Some(r) = search_expr(body, name, decl_span) {
                            return Some(r);
                        }
                    }
                }
            }
            DefinitionKind::Named(nd) => {
                if nd.name.0 == name && nd.name.1.start == decl_span.start {
                    if let Some(v) = &nd.value {
                        return Some(v.1);
                    }
                }
            }
            _ => {}
        }
    }
    None
}

fn infer_effects_for_def(
    ast: &sail_parser::core_ast::SourceFile,
    name: &str,
) -> Vec<String> {
    use sail_parser::core_ast::{
        DefinitionKind,
    };

    let mut effects = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (def, _) in &ast.defs {
        if let DefinitionKind::Callable(c) = &def.kind {
            if c.name.0 != name {
                continue;
            }
            // Check the body for side effects
            if let Some(body) = &c.body {
                collect_effects(&body.0, &mut effects, &mut seen);
            }
            for (clause, _) in &c.clauses {
                if let Some(body) = &clause.body {
                    collect_effects(&body.0, &mut effects, &mut seen);
                }
            }
        }
    }

    effects.sort();
    effects.dedup();
    effects
}

fn collect_effects(
    expr: &sail_parser::core_ast::Expr,
    effects: &mut Vec<String>,
    seen: &mut std::collections::HashSet<String>,
) {
    use sail_parser::core_ast::{BlockItem, Expr};

    match expr {
        Expr::Return(_) => {
            if seen.insert("return".to_string()) {
                effects.push("return".to_string());
            }
        }
        Expr::Throw(_) => {
            if seen.insert("throw".to_string()) {
                effects.push("throw".to_string());
            }
        }
        Expr::Exit(_) => {
            if seen.insert("exit".to_string()) {
                effects.push("exit".to_string());
            }
        }
        Expr::Assert { .. } => {
            if seen.insert("assert".to_string()) {
                effects.push("assert".to_string());
            }
        }
        Expr::Assign { .. } => {
            if seen.insert("mutation".to_string()) {
                effects.push("mutation".to_string());
            }
        }
        Expr::Ref(_) => {
            if seen.insert("register".to_string()) {
                effects.push("register".to_string());
            }
        }
        // Recurse into sub-expressions
        Expr::Block(items) => {
            for item in items {
                match &item.0 {
                    BlockItem::Expr(e) | BlockItem::Var { value: e, .. } => {
                        collect_effects(&e.0, effects, seen);
                    }
                    BlockItem::Let(lb) => collect_effects(&lb.value.0, effects, seen),
                }
            }
        }
        Expr::If { cond, then_branch, else_branch, .. } => {
            collect_effects(&cond.0, effects, seen);
            collect_effects(&then_branch.0, effects, seen);
            if let Some(e) = else_branch {
                collect_effects(&e.0, effects, seen);
            }
        }
        Expr::Match { scrutinee, cases, .. } | Expr::Try { scrutinee, cases, .. } => {
            collect_effects(&scrutinee.0, effects, seen);
            for (case, _) in cases {
                collect_effects(&case.body.0, effects, seen);
            }
        }
        Expr::Let { binding, body } => {
            collect_effects(&binding.value.0, effects, seen);
            collect_effects(&body.0, effects, seen);
        }
        Expr::Var { value, body, .. } => {
            collect_effects(&value.0, effects, seen);
            collect_effects(&body.0, effects, seen);
        }
        Expr::Foreach(f) => collect_effects(&f.body.0, effects, seen),
        Expr::While { cond, body, .. } => {
            collect_effects(&cond.0, effects, seen);
            collect_effects(&body.0, effects, seen);
        }
        Expr::Repeat { body, until, .. } => {
            collect_effects(&body.0, effects, seen);
            collect_effects(&until.0, effects, seen);
        }
        Expr::Call(call) => {
            collect_effects(&call.callee.0, effects, seen);
            for arg in &call.args {
                collect_effects(&arg.0, effects, seen);
            }
        }
        Expr::Infix { lhs, rhs, .. } => {
            collect_effects(&lhs.0, effects, seen);
            collect_effects(&rhs.0, effects, seen);
        }
        Expr::Prefix { expr: e, .. } | Expr::Cast { expr: e, .. } | Expr::Field { expr: e, .. } => {
            collect_effects(&e.0, effects, seen);
        }
        _ => {}
    }
}
