use crate::{Span, Token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    TopLevel,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclKind {
    Function,
    Value,
    Mapping,
    Overload,
    Register,
    Type,
    Struct,
    Union,
    Bitfield,
    Enum,
    EnumMember,
    Newtype,
    Let,
    Var,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeclRole {
    Declaration,
    Definition,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decl {
    pub name: String,
    pub kind: DeclKind,
    pub role: DeclRole,
    pub scope: Scope,
    pub span: Span,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedFile {
    pub decls: Vec<Decl>,
    pub type_aliases: Vec<TypeAlias>,
    pub call_sites: Vec<CallSite>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypeAlias {
    pub sub: String,
    pub sup: String,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallSite {
    pub caller: Option<String>,
    pub callee: String,
    pub callee_span: Span,
    pub open_span: Span,
    pub close_span: Option<Span>,
    pub arg_separator_spans: Vec<Span>,
}

fn push_decl(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    scope: Scope,
    span: Span,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope,
        span,
    });
}

fn scope_for_depth(block_depth: i32) -> Scope {
    if block_depth == 0 {
        Scope::TopLevel
    } else {
        Scope::Local
    }
}

fn parse_enum_members(
    tokens: &[(Token, Span)],
    mut idx: usize,
    scope: Scope,
    parsed: &mut ParsedFile,
) -> usize {
    while idx < tokens.len() {
        match &tokens[idx].0 {
            Token::RightCurlyBracket => return idx,
            Token::Id(name) => {
                push_decl(
                    parsed,
                    DeclKind::EnumMember,
                    DeclRole::Definition,
                    name,
                    scope,
                    tokens[idx].1,
                );
                idx += 1;
                if idx < tokens.len() && tokens[idx].0 == Token::Comma {
                    idx += 1;
                }
            }
            _ => idx += 1,
        }
    }
    idx
}

pub fn parse_tokens(tokens: &[(Token, Span)]) -> ParsedFile {
    let mut parsed = ParsedFile::default();
    let mut i = 0usize;
    let mut block_depth = 0_i32;

    while i < tokens.len() {
        match &tokens[i].0 {
            Token::LeftCurlyBracket | Token::LeftCurlyBar => {
                block_depth += 1;
                i += 1;
                continue;
            }
            Token::RightCurlyBracket | Token::RightCurlyBar => {
                block_depth = (block_depth - 1).max(0);
                i += 1;
                continue;
            }
            _ => {}
        }

        let scope = scope_for_depth(block_depth);
        match &tokens[i].0 {
            Token::KwScattered => {
                if i + 2 < tokens.len() {
                    match (&tokens[i + 1].0, &tokens[i + 2].0) {
                        (Token::KwFunction, Token::Id(name)) => {
                            push_decl(
                                &mut parsed,
                                DeclKind::Function,
                                DeclRole::Declaration,
                                name,
                                scope,
                                tokens[i + 2].1,
                            );
                            i += 3;
                            continue;
                        }
                        (Token::KwMapping, Token::Id(name)) => {
                            push_decl(
                                &mut parsed,
                                DeclKind::Mapping,
                                DeclRole::Declaration,
                                name,
                                scope,
                                tokens[i + 2].1,
                            );
                            i += 3;
                            continue;
                        }
                        (Token::KwUnion, Token::Id(name)) => {
                            push_decl(
                                &mut parsed,
                                DeclKind::Union,
                                DeclRole::Declaration,
                                name,
                                scope,
                                tokens[i + 2].1,
                            );
                            i += 3;
                            continue;
                        }
                        (Token::KwEnum, Token::Id(name)) => {
                            push_decl(
                                &mut parsed,
                                DeclKind::Enum,
                                DeclRole::Declaration,
                                name,
                                scope,
                                tokens[i + 2].1,
                            );
                            i += 3;
                            continue;
                        }
                        _ => {}
                    }
                }
            }
            Token::KwFunction => {
                if i + 2 < tokens.len() && tokens[i + 1].0 == Token::KwClause {
                    if let Token::Id(name) = &tokens[i + 2].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Function,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 2].1,
                        );
                        i += 3;
                        continue;
                    }
                } else if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Function,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwVal => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Value,
                            DeclRole::Declaration,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwMapping => {
                if i + 2 < tokens.len() && tokens[i + 1].0 == Token::KwClause {
                    if let Token::Id(name) = &tokens[i + 2].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Mapping,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 2].1,
                        );
                        i += 3;
                        continue;
                    }
                } else if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Mapping,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwOverload => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Overload,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwRegister => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Register,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwType => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Type,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        if i + 3 < tokens.len() && tokens[i + 2].0 == Token::Equal {
                            if let Token::Id(sup) = &tokens[i + 3].0 {
                                parsed.type_aliases.push(TypeAlias {
                                    sub: name.clone(),
                                    sup: sup.clone(),
                                    span: tokens[i + 1].1,
                                });
                            }
                        }
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwStruct => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Struct,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwUnion => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Union,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwBitfield => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Bitfield,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwEnum => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Enum,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        let mut j = i + 2;
                        if j + 1 < tokens.len()
                            && tokens[j].0 == Token::Equal
                            && tokens[j + 1].0 == Token::LeftCurlyBracket
                        {
                            j = parse_enum_members(tokens, j + 2, scope, &mut parsed);
                        }
                        i = j.saturating_add(1);
                        continue;
                    }
                }
            }
            Token::KwNewtype => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Newtype,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwLet => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Let,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwVar => {
                if i + 1 < tokens.len() {
                    if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Var,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            _ => {}
        }
        i += 1;
    }

    let callable_decls = parsed
        .decls
        .iter()
        .filter(|decl| {
            decl.scope == Scope::TopLevel
                && decl.role == DeclRole::Definition
                && matches!(
                    decl.kind,
                    DeclKind::Function | DeclKind::Value | DeclKind::Mapping | DeclKind::Overload
                )
        })
        .map(|decl| (decl.name.clone(), decl.span.start))
        .collect::<Vec<_>>();

    for i in 0..tokens.len().saturating_sub(1) {
        let (tok0, span0) = (&tokens[i].0, &tokens[i].1);
        let (tok1, span1) = (&tokens[i + 1].0, &tokens[i + 1].1);
        let (Token::Id(callee), Token::LeftBracket) = (tok0, tok1) else {
            continue;
        };

        let caller = callable_decls
            .iter()
            .rev()
            .find(|(_, start)| *start <= span0.start)
            .map(|(name, _)| name.clone());

        let mut depth = 1_i32;
        let mut j = i + 2;
        let mut close_span = None;
        let mut arg_separator_spans = Vec::new();

        while j < tokens.len() {
            let (token, span) = (&tokens[j].0, &tokens[j].1);
            if matches!(
                token,
                Token::LeftBracket
                    | Token::LeftSquareBracket
                    | Token::LeftCurlyBracket
                    | Token::LeftCurlyBar
                    | Token::LeftSquareBar
            ) {
                depth += 1;
            } else if matches!(
                token,
                Token::RightBracket
                    | Token::RightSquareBracket
                    | Token::RightCurlyBracket
                    | Token::RightCurlyBar
                    | Token::RightSquareBar
            ) {
                depth -= 1;
                if depth == 0 {
                    close_span = Some(*span);
                    break;
                }
            } else if *token == Token::Comma && depth == 1 {
                arg_separator_spans.push(*span);
            }
            j += 1;
        }

        parsed.call_sites.push(CallSite {
            caller,
            callee: callee.clone(),
            callee_span: *span0,
            open_span: *span1,
            close_span,
            arg_separator_spans,
        });
    }

    parsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use chumsky::Parser;

    #[test]
    fn parses_top_level_and_local_bindings_with_scope() {
        let source = r#"
function foo() = {
  let x = 1;
}
let y = 2
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parsed = parse_tokens(&tokens);
        assert!(parsed.decls.iter().any(|d| d.name == "foo"
            && d.kind == DeclKind::Function
            && d.role == DeclRole::Definition
            && d.scope == Scope::TopLevel));
        assert!(parsed.decls.iter().any(|d| d.name == "x"
            && d.kind == DeclKind::Let
            && d.role == DeclRole::Definition
            && d.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|d| d.name == "y"
            && d.kind == DeclKind::Let
            && d.role == DeclRole::Definition
            && d.scope == Scope::TopLevel));
    }

    #[test]
    fn parses_enum_members() {
        let source = "enum color = { Red, Green, Blue }";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parsed = parse_tokens(&tokens);
        assert!(parsed
            .decls
            .iter()
            .any(|d| d.name == "color" && d.kind == DeclKind::Enum));
        assert!(parsed
            .decls
            .iter()
            .any(|d| d.name == "Red" && d.kind == DeclKind::EnumMember));
    }

    #[test]
    fn parses_type_alias_edges() {
        let source = "type child = parent\ntype plain = bits";
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parsed = parse_tokens(&tokens);
        assert!(parsed
            .type_aliases
            .iter()
            .any(|a| a.sub == "child" && a.sup == "parent"));
    }

    #[test]
    fn parses_scattered_and_newtype_declarations() {
        let source = r#"
scattered union my_u
scattered enum my_e
newtype register_index = Mk_index : bits(5)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parsed = parse_tokens(&tokens);
        assert!(parsed.decls.iter().any(|d| d.name == "my_u"
            && d.kind == DeclKind::Union
            && d.role == DeclRole::Declaration));
        assert!(parsed.decls.iter().any(|d| d.name == "my_e"
            && d.kind == DeclKind::Enum
            && d.role == DeclRole::Declaration));
        assert!(parsed.decls.iter().any(|d| d.name == "register_index"
            && d.kind == DeclKind::Newtype
            && d.role == DeclRole::Definition));
    }

    #[test]
    fn parses_scattered_function_head_and_clause_roles() {
        let source = r#"
scattered function foo
function clause foo(x) = x
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let parsed = parse_tokens(&tokens);
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo" && d.kind == DeclKind::Function && d.role == DeclRole::Declaration
        }));
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo" && d.kind == DeclKind::Function && d.role == DeclRole::Definition
        }));
    }
}
