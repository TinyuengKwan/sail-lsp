use crate::analysis::{collect_callable_signatures, infer_binding_type, CallableSignature};
use crate::file::File;
use sail_parser::{Decl, DeclKind, DeclRole, Scope, Span, Token};
use tower_lsp::lsp_types::{
    Hover, HoverContents, MarkupContent, MarkupKind, Position, SymbolKind, Url,
};

pub(crate) fn hover_for_symbol<'a, I>(
    files: I,
    current_uri: &Url,
    current_file: &File,
    position: Position,
    symbol_key: &str,
) -> Option<Hover>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    if symbol_key.starts_with('\'') {
        return Some(markdown_hover(fenced_sail(symbol_key)));
    }

    let files = files.into_iter().collect::<Vec<_>>();
    let current_offset = current_file.source.offset_at(&position);

    let decl_ref = resolve_decl(
        files.iter().copied(),
        current_uri,
        symbol_key,
        Some(current_offset),
    );

    let snippets = if let Some(decl_ref) = decl_ref.as_ref() {
        if is_callable_decl(decl_ref.decl.kind) {
            let callables = collect_callable_hover_snippets(files.iter().copied(), current_uri, symbol_key);
            if callables.is_empty() {
                vec![render_signature(decl_ref, files.iter().copied(), current_uri, symbol_key)]
            } else {
                callables
            }
        } else {
            vec![render_signature(decl_ref, files.iter().copied(), current_uri, symbol_key)]
        }
    } else {
        vec![symbol_key.to_string()]
    };

    let markup = snippets
        .into_iter()
        .map(|snippet| fenced_sail(&snippet))
        .collect::<Vec<_>>()
        .join("\n\n---\n");
    Some(markdown_hover(markup))
}

fn markdown_hover(markdown: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value: markdown,
        }),
        range: None,
    }
}

fn fenced_sail(text: &str) -> String {
    format!("```sail\n{text}\n```")
}

fn is_callable_decl(kind: DeclKind) -> bool {
    matches!(
        kind,
        DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload
    )
}

fn decl_kind_label(kind: DeclKind) -> &'static str {
    match kind {
        DeclKind::Function => "function",
        DeclKind::Value => "value",
        DeclKind::Mapping => "mapping",
        DeclKind::Overload => "overload",
        DeclKind::Register => "register",
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
        DeclKind::Register | DeclKind::Let | DeclKind::Var => SymbolKind::VARIABLE,
        DeclKind::Enum => SymbolKind::ENUM,
        DeclKind::EnumMember => SymbolKind::ENUM_MEMBER,
        DeclKind::Type | DeclKind::Struct | DeclKind::Union | DeclKind::Bitfield | DeclKind::Newtype => SymbolKind::STRUCT,
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
    )
}

fn decl_headline(file: &File, decl: &Decl) -> String {
    if is_callable_decl(decl.kind) {
        if let Some(sig) = collect_callable_signatures(file)
            .into_iter()
            .find(|sig| sig.name == decl.name)
        {
            return sig.label;
        }
    }

    let Some(tokens) = file.tokens.as_ref() else {
        return format!("{} {}", decl_kind_label(decl.kind), decl.name);
    };
    let Some(idx) = tokens.iter().position(|(_, span)| span.start == decl.span.start) else {
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
    while end_idx + 1 < tokens.len() {
        let token = &tokens[end_idx + 1].0;
        if *token == Token::Semicolon || *token == Token::Equal || token_starts_declaration(token) {
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
    if let Some(sig) = best_callable_signature(files, current_uri, symbol_key)
        .filter(|_| is_callable_decl(decl_ref.decl.kind))
    {
        return sig.label;
    }

    if matches!(decl_ref.decl.kind, DeclKind::Let | DeclKind::Var) {
        if let Some(ty) = binding_type_hint(decl_ref.file, &decl_ref.decl) {
            let kw = match decl_ref.decl.kind {
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
            let Some(tokens) = file.tokens.as_ref() else {
                continue;
            };
            let parsed = sail_parser::parse_tokens(tokens);
            if let Some(decl) = parsed
                .decls
                .into_iter()
                .filter(|decl| {
                    decl.name == symbol_key && decl.scope == Scope::Local && decl.span.start <= offset
                })
                .max_by_key(|decl| decl.span.start)
            {
                return Some(DeclRef { file, decl });
            }
        }
    }

    let mut best: Option<(usize, DeclRef<'a>)> = None;
    for (uri, file) in files {
        let Some(tokens) = file.tokens.as_ref() else {
            continue;
        };
        for decl in sail_parser::parse_tokens(tokens).decls.into_iter().filter(|decl| {
            decl.name == symbol_key && (decl.scope == Scope::TopLevel || decl.kind == DeclKind::EnumMember)
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
                _ => best = Some((score, DeclRef { file, decl })),
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

fn best_callable_signature<'a, I>(
    files: I,
    current_uri: &Url,
    name: &str,
) -> Option<CallableSignature>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut best: Option<(usize, CallableSignature)> = None;
    for (uri, file) in files {
        for sig in collect_callable_signatures(file) {
            if sig.name != name {
                continue;
            }
            let mut score = uri_prefix_score(current_uri, uri) * 16;
            if uri == current_uri {
                score += 8;
            }
            if sig.return_type.is_some() {
                score += 4;
            }
            if !sig.params.is_empty() {
                score += 2;
            }
            if sig.label.contains(':') {
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

fn collect_callable_hover_snippets<'a, I>(files: I, current_uri: &Url, name: &str) -> Vec<String>
where
    I: IntoIterator<Item = (&'a Url, &'a File)>,
{
    let mut rows = Vec::<(usize, String)>::new();
    for (uri, file) in files {
        for sig in collect_callable_signatures(file) {
            if sig.name != name {
                continue;
            }
            let mut score = uri_prefix_score(current_uri, uri) * 16;
            if uri == current_uri {
                score += 8;
            }
            if sig.return_type.is_some() {
                score += 4;
            }
            if !sig.params.is_empty() {
                score += 2;
            }
            if sig.label.contains(':') {
                score += 1;
            }
            rows.push((score, sig.label.trim().to_string()));
        }
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let mut snippets = Vec::<String>::new();
    for (_, snippet) in rows {
        if snippets.iter().any(|seen| seen == &snippet) {
            continue;
        }
        snippets.push(snippet);
    }
    snippets
}

fn find_token_index_by_span_start(tokens: &[(Token, Span)], span_start: usize) -> Option<usize> {
    tokens.iter().position(|(_, span)| span.start == span_start)
}

fn binding_type_hint(file: &File, decl: &Decl) -> Option<String> {
    let tokens = file.tokens.as_ref()?;
    let idx = find_token_index_by_span_start(tokens, decl.span.start)?;
    let text = file.source.text();

    if idx + 2 < tokens.len() && tokens[idx + 1].0 == Token::Colon {
        let mut end_idx = idx + 2;
        while end_idx < tokens.len() {
            let token = &tokens[end_idx].0;
            if *token == Token::Equal || *token == Token::Semicolon || token_starts_declaration(token)
            {
                break;
            }
            end_idx += 1;
        }
        if end_idx > idx + 2 {
            return Some(
                text[tokens[idx + 2].1.start..tokens[end_idx - 1].1.end]
                    .trim()
                    .to_string(),
            );
        }
    }

    let mut eq_idx = None;
    let mut i = idx + 1;
    while i < tokens.len() {
        let token = &tokens[i].0;
        if *token == Token::Equal {
            eq_idx = Some(i);
            break;
        }
        if *token == Token::Semicolon || token_starts_declaration(token) {
            break;
        }
        i += 1;
    }
    let eq_idx = eq_idx?;
    infer_binding_type(&tokens.get(eq_idx + 1)?.0).map(str::to_string)
}

enum EnumInfo {
    Member {
        enum_name: String,
    },
}

fn enum_info_for_symbol(file: &File, symbol: &str) -> Option<EnumInfo> {
    let tokens = file.tokens.as_ref()?;
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
    use tower_lsp::lsp_types::Url;

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
            "add",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert!(markdown.contains("```sail\nval add : int -> int\n```"));
        assert!(markdown.contains("```sail\nfunction add(x)\n```"));
        assert!(markdown.contains("---"));
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
            "x",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert_eq!(markdown.trim(), "```sail\nlet x : bits(32)\n```");
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
            "Red",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert_eq!(markdown.trim(), "```sail\ncolor::Red\n```");
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
            "'n",
        )
        .expect("hover");
        let markdown = hover_markdown(hover);
        assert_eq!(markdown.trim(), "```sail\n'n\n```");
    }
}
