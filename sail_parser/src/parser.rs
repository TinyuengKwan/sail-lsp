use crate::{
    ast::{
        BlockItem as AstBlockItem, Call as AstCall, CallableDef as AstCallableDef, CallableDefKind,
        CallableSpec as AstCallableSpec, CallableSpecKind, Expr as AstExpr,
        FieldExpr as AstFieldExpr, FieldPattern as AstFieldPattern, ForeachExpr as AstForeachExpr,
        LoopMeasure as AstLoopMeasure, MappingBody as AstMappingBody, MatchCase as AstMatchCase,
        NamedDef as AstNamedDef, NamedDefKind, Pattern as AstPattern,
        ScatteredClauseDef as AstScatteredClauseDef, ScatteredClauseKind,
        ScatteredDef as AstScatteredDef, ScatteredKind, SourceFile,
        TerminationMeasureKind as AstTerminationMeasureKind, TopLevelDef,
        TypeAliasDef as AstTypeAliasDef, TypeExpr as AstTypeExpr, VectorUpdate as AstVectorUpdate,
    },
    Span, Token,
};
use std::collections::{HashMap, HashSet};

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
    Parameter,
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
    pub is_scattered: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ParsedFile {
    pub decls: Vec<Decl>,
    pub type_aliases: Vec<TypeAlias>,
    pub call_sites: Vec<CallSite>,
    pub typed_bindings: Vec<TypedBinding>,
    pub callable_heads: Vec<CallableHead>,
    pub symbol_occurrences: Vec<SymbolOccurrence>,
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TypedBinding {
    pub name: String,
    pub name_span: Span,
    pub ty_span: Span,
    pub scope: Scope,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableParam {
    pub span: Span,
    pub name: Option<String>,
    pub name_span: Option<Span>,
    pub ty_span: Option<Span>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallableHead {
    pub name: String,
    pub kind: DeclKind,
    pub name_span: Span,
    pub label_span: Span,
    pub params: Vec<CallableParam>,
    pub return_type_span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SymbolOccurrenceKind {
    Value,
    Type,
    TypeVar,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SymbolOccurrence {
    pub name: String,
    pub kind: SymbolOccurrenceKind,
    pub span: Span,
    pub scope: Option<Scope>,
    pub role: Option<DeclRole>,
    pub target_span: Option<Span>,
}

fn push_decl(
    parsed: &mut ParsedFile,
    kind: DeclKind,
    role: DeclRole,
    name: &str,
    scope: Scope,
    span: Span,
    is_scattered: bool,
) {
    parsed.decls.push(Decl {
        name: name.to_string(),
        kind,
        role,
        scope,
        span,
        is_scattered,
    });
}

fn scope_for_depth(block_depth: i32) -> Scope {
    if block_depth == 0 {
        Scope::TopLevel
    } else {
        Scope::Local
    }
}

fn token_is_open_bracket(token: &Token) -> bool {
    matches!(
        token,
        Token::LeftBracket
            | Token::LeftSquareBracket
            | Token::LeftCurlyBracket
            | Token::LeftCurlyBar
            | Token::LeftSquareBar
    )
}

fn token_is_close_bracket(token: &Token) -> bool {
    matches!(
        token,
        Token::RightBracket
            | Token::RightSquareBracket
            | Token::RightCurlyBracket
            | Token::RightCurlyBar
            | Token::RightSquareBar
    )
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

fn span_for_indices(tokens: &[(Token, Span)], start: usize, end: usize) -> Span {
    Span::new(tokens[start].1.start, tokens[end].1.end)
}

fn find_matching_round_bracket(tokens: &[(Token, Span)], open_idx: usize) -> Option<usize> {
    if tokens.get(open_idx).map(|(token, _)| token) != Some(&Token::LeftBracket) {
        return None;
    }

    let mut depth = 0_i32;
    for (idx, (token, _)) in tokens.iter().enumerate().skip(open_idx) {
        match token {
            Token::LeftBracket => depth += 1,
            Token::RightBracket => {
                depth -= 1;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }

    None
}

fn strip_wrapping_parens(tokens: &[(Token, Span)], start: usize, end: usize) -> (usize, usize) {
    let mut start = start;
    let mut end = end;
    while start < end
        && tokens[start].0 == Token::LeftBracket
        && find_matching_round_bracket(tokens, start) == Some(end)
    {
        start += 1;
        end = end.saturating_sub(1);
    }
    (start, end)
}

fn split_top_level_segments<F>(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
    is_separator: F,
) -> Vec<(usize, usize)>
where
    F: Fn(&Token) -> bool,
{
    if start > end {
        return Vec::new();
    }

    let mut segments = Vec::new();
    let mut segment_start = start;
    let mut depth = 0_i32;

    for idx in start..=end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && is_separator(token) {
            if segment_start < idx {
                segments.push((segment_start, idx - 1));
            }
            segment_start = idx + 1;
        }
    }

    if segment_start <= end {
        segments.push((segment_start, end));
    }

    segments
}

fn find_top_level_token<F>(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
    predicate: F,
) -> Option<usize>
where
    F: Fn(&Token) -> bool,
{
    if start > end {
        return None;
    }

    let mut depth = 0_i32;
    for idx in start..=end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth -= 1;
        }

        if depth == 0 && predicate(token) {
            return Some(idx);
        }
    }

    None
}

fn find_declaration_end(tokens: &[(Token, Span)], start_idx: usize) -> usize {
    let mut depth = 0_i32;
    let mut end_idx = start_idx;
    for idx in (start_idx + 1)..tokens.len() {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth = (depth - 1).max(0);
        }

        if depth == 0 && (*token == Token::Semicolon || token_starts_declaration(token)) {
            break;
        }
        end_idx = idx;
    }
    end_idx
}

fn find_function_head_end(tokens: &[(Token, Span)], start_idx: usize) -> usize {
    let mut depth = 0_i32;
    let mut end_idx = start_idx;
    for idx in (start_idx + 1)..tokens.len() {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
        } else if token_is_close_bracket(token) {
            depth = (depth - 1).max(0);
        }

        if depth == 0 && (*token == Token::Equal || *token == Token::Semicolon) {
            break;
        }
        if depth == 0 && idx > start_idx + 1 && token_starts_declaration(token) {
            break;
        }
        end_idx = idx;
    }
    end_idx
}

fn parse_binding_segment(
    tokens: &[(Token, Span)],
    start: usize,
    end: usize,
) -> Option<CallableParam> {
    if start > end {
        return None;
    }

    let span = span_for_indices(tokens, start, end);
    let (start, end) = strip_wrapping_parens(tokens, start, end);
    if start > end {
        return Some(CallableParam {
            span,
            name: None,
            name_span: None,
            ty_span: None,
        });
    }

    let colon = find_top_level_token(tokens, start, end, |token| *token == Token::Colon);
    let name_end = colon.map_or(end, |idx| idx.saturating_sub(1));

    let mut depth = 0_i32;
    let mut found_name = None;
    for idx in start..=name_end {
        let token = &tokens[idx].0;
        if token_is_open_bracket(token) {
            depth += 1;
            continue;
        }
        if token_is_close_bracket(token) {
            depth -= 1;
            continue;
        }
        if depth == 0 {
            if let Token::Id(name) = token {
                found_name = Some((name.clone(), tokens[idx].1));
                break;
            }
        }
    }

    let ty_span = colon.and_then(|colon_idx| {
        if colon_idx < end {
            Some(span_for_indices(tokens, colon_idx + 1, end))
        } else {
            None
        }
    });

    Some(CallableParam {
        span,
        name: found_name.as_ref().map(|(name, _)| name.clone()),
        name_span: found_name.map(|(_, span)| span),
        ty_span,
    })
}

fn parse_function_head(
    tokens: &[(Token, Span)],
    kw_idx: usize,
    name_idx: usize,
    name: &str,
) -> CallableHead {
    let head_end = find_function_head_end(tokens, kw_idx);
    let label_span = span_for_indices(tokens, kw_idx, head_end);

    let mut params = Vec::new();
    let mut cursor = name_idx + 1;

    if cursor < tokens.len() && tokens[cursor].0 == Token::LeftBracket {
        if let Some(close_idx) = find_matching_round_bracket(tokens, cursor) {
            if cursor + 1 < close_idx {
                for (segment_start, segment_end) in
                    split_top_level_segments(tokens, cursor + 1, close_idx - 1, |token| {
                        *token == Token::Comma
                    })
                {
                    if let Some(param) = parse_binding_segment(tokens, segment_start, segment_end) {
                        params.push(param);
                    }
                }
            }
            cursor = close_idx + 1;
        }
    } else if cursor <= head_end {
        let param_end = find_top_level_token(tokens, cursor, head_end, |token| {
            *token == Token::RightArrow
        })
        .map_or(head_end, |idx| idx.saturating_sub(1));

        for idx in cursor..=param_end {
            if let Token::Id(param_name) = &tokens[idx].0 {
                params.push(CallableParam {
                    span: tokens[idx].1,
                    name: Some(param_name.clone()),
                    name_span: Some(tokens[idx].1),
                    ty_span: None,
                });
            }
        }
    }

    let return_type_span = if cursor <= head_end {
        find_top_level_token(tokens, cursor, head_end, |token| {
            *token == Token::RightArrow
        })
        .and_then(|arrow_idx| {
            if arrow_idx < head_end {
                Some(span_for_indices(tokens, arrow_idx + 1, head_end))
            } else {
                None
            }
        })
    } else {
        None
    };

    CallableHead {
        name: name.to_string(),
        kind: DeclKind::Function,
        name_span: tokens[name_idx].1,
        label_span,
        params,
        return_type_span,
    }
}

fn parse_value_like_head(
    tokens: &[(Token, Span)],
    kw_idx: usize,
    name_idx: usize,
    name: &str,
    kind: DeclKind,
) -> Option<CallableHead> {
    let end_idx = find_declaration_end(tokens, kw_idx);
    let colon_idx = find_top_level_token(tokens, name_idx + 1, end_idx, |token| {
        *token == Token::Colon
    })?;

    let type_segments = split_top_level_segments(tokens, colon_idx + 1, end_idx, |token| {
        matches!(token, Token::RightArrow | Token::DoubleArrow)
    });
    if type_segments.is_empty() {
        return None;
    }

    let mut params = Vec::new();
    if type_segments.len() > 1 {
        for (segment_idx, (segment_start, segment_end)) in type_segments[..type_segments.len() - 1]
            .iter()
            .copied()
            .enumerate()
        {
            let (inner_start, inner_end) =
                strip_wrapping_parens(tokens, segment_start, segment_end);
            if segment_idx == 0 && (inner_start, inner_end) != (segment_start, segment_end) {
                for (sub_start, sub_end) in
                    split_top_level_segments(tokens, inner_start, inner_end, |token| {
                        *token == Token::Comma
                    })
                {
                    params.push(CallableParam {
                        span: span_for_indices(tokens, sub_start, sub_end),
                        name: None,
                        name_span: None,
                        ty_span: Some(span_for_indices(tokens, sub_start, sub_end)),
                    });
                }
            } else {
                params.push(CallableParam {
                    span: span_for_indices(tokens, segment_start, segment_end),
                    name: None,
                    name_span: None,
                    ty_span: Some(span_for_indices(tokens, segment_start, segment_end)),
                });
            }
        }
    }

    let (_, return_end) = *type_segments.last()?;
    let return_start = type_segments.last()?.0;

    Some(CallableHead {
        name: name.to_string(),
        kind,
        name_span: tokens[name_idx].1,
        label_span: span_for_indices(tokens, kw_idx, end_idx),
        params,
        return_type_span: Some(span_for_indices(tokens, return_start, return_end)),
    })
}

fn maybe_push_typed_binding(
    parsed: &mut ParsedFile,
    name: &str,
    name_span: Span,
    ty_span: Option<Span>,
    scope: Scope,
) {
    let Some(ty_span) = ty_span else {
        return;
    };
    parsed.typed_bindings.push(TypedBinding {
        name: name.to_string(),
        name_span,
        ty_span,
        scope,
    });
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
                    false,
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
                if i + 1 < tokens.len() {
                    let mut j = i + 1;
                    let kind = match &tokens[j].0 {
                        Token::KwFunction => {
                            j += 1;
                            DeclKind::Function
                        }
                        Token::KwMapping => {
                            j += 1;
                            DeclKind::Mapping
                        }
                        Token::KwUnion => {
                            j += 1;
                            DeclKind::Union
                        }
                        Token::KwEnum => {
                            j += 1;
                            DeclKind::Enum
                        }
                        _ => DeclKind::Function,
                    };
                    if j < tokens.len() {
                        if let Token::Id(name) = &tokens[j].0 {
                            push_decl(
                                &mut parsed,
                                kind,
                                DeclRole::Declaration,
                                name,
                                scope,
                                tokens[j].1,
                                true,
                            );
                            i = j + 1;
                            continue;
                        }
                    }
                }
            }
            Token::KwFunction => {
                let mut j = i + 1;
                let mut is_scattered = false;
                if j < tokens.len() && tokens[j].0 == Token::KwClause {
                    j += 1;
                    is_scattered = true;
                }
                if j < tokens.len() {
                    if let Token::Id(name) = &tokens[j].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Function,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[j].1,
                            is_scattered,
                        );
                        let head = parse_function_head(tokens, i, j, name);
                        for param in &head.params {
                            if let (Some(param_name), Some(param_name_span)) =
                                (param.name.as_deref(), param.name_span)
                            {
                                maybe_push_typed_binding(
                                    &mut parsed,
                                    param_name,
                                    param_name_span,
                                    param.ty_span,
                                    Scope::Local,
                                );
                            }
                        }
                        parsed.callable_heads.push(head);
                        i = j + 1;
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
                            false,
                        );
                        if let Some(head) =
                            parse_value_like_head(tokens, i, i + 1, name, DeclKind::Value)
                        {
                            parsed.callable_heads.push(head);
                        }
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwMapping => {
                let mut j = i + 1;
                let mut is_scattered = false;
                if j < tokens.len() && tokens[j].0 == Token::KwClause {
                    j += 1;
                    is_scattered = true;
                }
                if j < tokens.len() {
                    if let Token::Id(name) = &tokens[j].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Mapping,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[j].1,
                            is_scattered,
                        );
                        if let Some(head) =
                            parse_value_like_head(tokens, i, j, name, DeclKind::Mapping)
                        {
                            parsed.callable_heads.push(head);
                        }
                        i = j + 1;
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
                            false,
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
                            false,
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
                            false,
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
                            false,
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
                            false,
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
                            false,
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
                            false,
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
                            false,
                        );
                        i += 2;
                        continue;
                    }
                }
            }
            Token::KwLet => {
                if i + 1 < tokens.len() {
                    let binding_end =
                        find_top_level_token(tokens, i + 1, tokens.len() - 1, |token| {
                            matches!(token, Token::Equal | Token::Semicolon | Token::KwIn)
                        })
                        .map_or(tokens.len() - 1, |idx| idx.saturating_sub(1));

                    if let Some(binding) = parse_binding_segment(tokens, i + 1, binding_end) {
                        if let (Some(name), Some(name_span)) =
                            (binding.name.as_deref(), binding.name_span)
                        {
                            if matches!(tokens[i + 1].0, Token::Id(_)) {
                                push_decl(
                                    &mut parsed,
                                    DeclKind::Let,
                                    DeclRole::Definition,
                                    name,
                                    scope,
                                    name_span,
                                    false,
                                );
                            }
                            maybe_push_typed_binding(
                                &mut parsed,
                                name,
                                name_span,
                                binding.ty_span,
                                scope,
                            );
                        }
                    } else if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Let,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                            false,
                        );
                    }
                    i += 2;
                    continue;
                }
            }
            Token::KwVar => {
                if i + 1 < tokens.len() {
                    let binding_end =
                        find_top_level_token(tokens, i + 1, tokens.len() - 1, |token| {
                            matches!(token, Token::Equal | Token::Semicolon | Token::KwIn)
                        })
                        .map_or(tokens.len() - 1, |idx| idx.saturating_sub(1));

                    if let Some(binding) = parse_binding_segment(tokens, i + 1, binding_end) {
                        if let (Some(name), Some(name_span)) =
                            (binding.name.as_deref(), binding.name_span)
                        {
                            if matches!(tokens[i + 1].0, Token::Id(_)) {
                                push_decl(
                                    &mut parsed,
                                    DeclKind::Var,
                                    DeclRole::Definition,
                                    name,
                                    scope,
                                    name_span,
                                    false,
                                );
                            }
                            maybe_push_typed_binding(
                                &mut parsed,
                                name,
                                name_span,
                                binding.ty_span,
                                scope,
                            );
                        }
                    } else if let Token::Id(name) = &tokens[i + 1].0 {
                        push_decl(
                            &mut parsed,
                            DeclKind::Var,
                            DeclRole::Definition,
                            name,
                            scope,
                            tokens[i + 1].1,
                            false,
                        );
                    }
                    i += 2;
                    continue;
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

        // Exclude identifiers immediately preceded by function, val, mapping, etc.
        if i > 0 {
            let prev_tok = &tokens[i - 1].0;
            if matches!(
                prev_tok,
                Token::KwFunction
                    | Token::KwVal
                    | Token::KwMapping
                    | Token::KwOverload
                    | Token::KwScattered
                    | Token::KwType
                    | Token::KwStruct
                    | Token::KwUnion
                    | Token::KwEnum
                    | Token::KwNewtype
            ) {
                continue;
            }
        }

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

fn decl_kind_for_scattered(kind: ScatteredKind) -> DeclKind {
    match kind {
        ScatteredKind::Function => DeclKind::Function,
        ScatteredKind::Mapping => DeclKind::Mapping,
        ScatteredKind::Union => DeclKind::Union,
        ScatteredKind::Enum => DeclKind::Enum,
    }
}

fn decl_kind_for_named(kind: NamedDefKind) -> DeclKind {
    match kind {
        NamedDefKind::Register => DeclKind::Register,
        NamedDefKind::Overload => DeclKind::Overload,
        NamedDefKind::Struct => DeclKind::Struct,
        NamedDefKind::Union => DeclKind::Union,
        NamedDefKind::Bitfield => DeclKind::Bitfield,
        NamedDefKind::Enum => DeclKind::Enum,
        NamedDefKind::Newtype => DeclKind::Newtype,
        NamedDefKind::Let => DeclKind::Let,
        NamedDefKind::Var => DeclKind::Var,
    }
}

fn callable_params_from_type_expr(ty: &(AstTypeExpr, Span)) -> Vec<CallableParam> {
    match &ty.0 {
        AstTypeExpr::Tuple(items) => items
            .iter()
            .map(|item| CallableParam {
                span: item.1,
                name: None,
                name_span: None,
                ty_span: Some(item.1),
            })
            .collect(),
        _ => vec![CallableParam {
            span: ty.1,
            name: None,
            name_span: None,
            ty_span: Some(ty.1),
        }],
    }
}

fn unwrapped_type_expr<'a>(ty: &'a (AstTypeExpr, Span)) -> &'a (AstTypeExpr, Span) {
    match &ty.0 {
        AstTypeExpr::Forall { body, .. } => unwrapped_type_expr(body),
        AstTypeExpr::Effect { ty, .. } => unwrapped_type_expr(ty),
        _ => ty,
    }
}

fn callable_head_from_spec(spec: &AstCallableSpec) -> CallableHead {
    callable_head_from_signature(
        &spec.name.0,
        match spec.kind {
            CallableSpecKind::Value => DeclKind::Value,
            CallableSpecKind::Mapping => DeclKind::Mapping,
        },
        spec.name.1,
        &spec.signature,
    )
}

fn callable_head_from_signature(
    name: &str,
    kind: DeclKind,
    name_span: Span,
    signature: &(AstTypeExpr, Span),
) -> CallableHead {
    let signature = unwrapped_type_expr(signature);
    let (params, return_type_span) = match &signature.0 {
        AstTypeExpr::Arrow { params, ret, .. } => {
            let mut callable_params = Vec::new();
            for param in params {
                callable_params.extend(callable_params_from_type_expr(param));
            }
            (callable_params, Some(ret.1))
        }
        _ => (Vec::new(), Some(signature.1)),
    };

    CallableHead {
        name: name.to_string(),
        kind,
        name_span,
        label_span: Span::new(name_span.start, signature.1.end),
        params,
        return_type_span,
    }
}

fn callable_param_from_pattern(pattern: &(AstPattern, Span)) -> CallableParam {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            let mut param = callable_param_from_pattern(inner);
            param.span = pattern.1;
            param
        }
        AstPattern::Typed(inner, ty) => {
            let mut param = callable_param_from_pattern(inner);
            param.span = pattern.1;
            param.ty_span = Some(ty.1);
            param
        }
        AstPattern::AsBinding { binding, .. } => CallableParam {
            span: pattern.1,
            name: Some(binding.0.clone()),
            name_span: Some(binding.1),
            ty_span: None,
        },
        AstPattern::Ident(name) => CallableParam {
            span: pattern.1,
            name: Some(name.clone()),
            name_span: Some(pattern.1),
            ty_span: None,
        },
        _ => CallableParam {
            span: pattern.1,
            name: None,
            name_span: None,
            ty_span: None,
        },
    }
}

fn collect_typed_bindings_from_pattern(
    pattern: &(AstPattern, Span),
    scope: Scope,
    parsed: &mut ParsedFile,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            collect_typed_bindings_from_pattern(inner, scope, parsed);
        }
        AstPattern::Typed(inner, ty) => {
            if let AstPattern::Ident(name) = &inner.0 {
                parsed.typed_bindings.push(TypedBinding {
                    name: name.clone(),
                    name_span: inner.1,
                    ty_span: ty.1,
                    scope,
                });
            }
            collect_typed_bindings_from_pattern(inner, scope, parsed);
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_typed_bindings_from_pattern(item, scope, parsed);
            }
        }
        AstPattern::App { args, .. } => {
            for arg in args {
                collect_typed_bindings_from_pattern(arg, scope, parsed);
            }
        }
        AstPattern::AsBinding { pattern, .. } => {
            collect_typed_bindings_from_pattern(pattern, scope, parsed);
        }
        AstPattern::Struct { fields, .. } => {
            for field in fields {
                if let AstFieldPattern::Binding { pattern, .. } = &field.0 {
                    collect_typed_bindings_from_pattern(pattern, scope, parsed);
                }
            }
        }
        AstPattern::Infix { lhs, rhs, .. } => {
            collect_typed_bindings_from_pattern(lhs, scope, parsed);
            collect_typed_bindings_from_pattern(rhs, scope, parsed);
        }
        AstPattern::AsType(inner, _) => {
            collect_typed_bindings_from_pattern(inner, scope, parsed);
        }
        _ => {}
    }
}

fn collect_decl_names_from_pattern(
    pattern: &(AstPattern, Span),
    kind: DeclKind,
    scope: Scope,
    parsed: &mut ParsedFile,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern: inner, .. } => {
            collect_decl_names_from_pattern(inner, kind, scope, parsed);
        }
        AstPattern::Ident(name) => {
            push_decl(
                parsed,
                kind,
                DeclRole::Definition,
                name,
                scope,
                pattern.1,
                false,
            );
        }
        AstPattern::Typed(inner, _) | AstPattern::AsType(inner, _) => {
            collect_decl_names_from_pattern(inner, kind, scope, parsed);
        }
        AstPattern::AsBinding { pattern, binding } => {
            collect_decl_names_from_pattern(pattern, kind, scope, parsed);
            push_decl(
                parsed,
                kind,
                DeclRole::Definition,
                &binding.0,
                scope,
                binding.1,
                false,
            );
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_decl_names_from_pattern(item, kind, scope, parsed);
            }
        }
        AstPattern::App { args, .. } => {
            for arg in args {
                collect_decl_names_from_pattern(arg, kind, scope, parsed);
            }
        }
        AstPattern::Struct { fields, .. } => {
            for field in fields {
                match &field.0 {
                    AstFieldPattern::Binding { pattern, .. } => {
                        collect_decl_names_from_pattern(pattern, kind, scope, parsed);
                    }
                    AstFieldPattern::Shorthand(name) => {
                        push_decl(
                            parsed,
                            kind,
                            DeclRole::Definition,
                            &name.0,
                            scope,
                            name.1,
                            false,
                        );
                    }
                    AstFieldPattern::Wild(_) => {}
                }
            }
        }
        AstPattern::Infix { lhs, rhs, .. } => {
            collect_decl_names_from_pattern(lhs, kind, scope, parsed);
            collect_decl_names_from_pattern(rhs, kind, scope, parsed);
        }
        _ => {}
    }
}

fn callee_name_and_span(expr: &(AstExpr, Span)) -> Option<(String, Span)> {
    match &expr.0 {
        AstExpr::Ident(name) => Some((name.clone(), expr.1)),
        AstExpr::Field { field, .. } => Some((field.0.clone(), field.1)),
        _ => None,
    }
}

fn var_target_name(expr: &(AstExpr, Span)) -> Option<(String, Span)> {
    match &expr.0 {
        AstExpr::Ident(name) => Some((name.clone(), expr.1)),
        AstExpr::Cast { expr, .. } => var_target_name(expr),
        _ => None,
    }
}

fn typed_var_target(expr: &(AstExpr, Span)) -> Option<(String, Span, Span)> {
    match &expr.0 {
        AstExpr::Cast { expr, ty } => var_target_name(expr).map(|(name, span)| (name, span, ty.1)),
        _ => None,
    }
}

fn collect_field_expr_metadata(
    field: &(AstFieldExpr, Span),
    caller: Option<&str>,
    parsed: &mut ParsedFile,
) {
    if let AstFieldExpr::Assignment { target, value } = &field.0 {
        collect_expr_metadata(target, caller, parsed);
        collect_expr_metadata(value, caller, parsed);
    }
}

fn collect_vector_update_metadata(
    update: &(AstVectorUpdate, Span),
    caller: Option<&str>,
    parsed: &mut ParsedFile,
) {
    match &update.0 {
        AstVectorUpdate::Assignment { target, value } => {
            collect_expr_metadata(target, caller, parsed);
            collect_expr_metadata(value, caller, parsed);
        }
        AstVectorUpdate::RangeAssignment { start, end, value } => {
            collect_expr_metadata(start, caller, parsed);
            collect_expr_metadata(end, caller, parsed);
            collect_expr_metadata(value, caller, parsed);
        }
        AstVectorUpdate::Shorthand(_) => {}
    }
}

fn collect_case_metadata(
    case: &(AstMatchCase, Span),
    caller: Option<&str>,
    parsed: &mut ParsedFile,
) {
    collect_decl_names_from_pattern(&case.0.pattern, DeclKind::Let, Scope::Local, parsed);
    collect_typed_bindings_from_pattern(&case.0.pattern, Scope::Local, parsed);
    if let Some(guard) = &case.0.guard {
        collect_expr_metadata(guard, caller, parsed);
    }
    collect_expr_metadata(&case.0.body, caller, parsed);
}

fn collect_call_metadata(call: &AstCall, caller: Option<&str>, parsed: &mut ParsedFile) {
    if let Some((callee, callee_span)) = callee_name_and_span(&call.callee) {
        parsed.call_sites.push(CallSite {
            caller: caller.map(|name| name.to_string()),
            callee,
            callee_span,
            open_span: call.open_span,
            close_span: Some(call.close_span),
            arg_separator_spans: call.arg_separator_spans.clone(),
        });
    }
    collect_expr_metadata(&call.callee, caller, parsed);
    for arg in &call.args {
        collect_expr_metadata(arg, caller, parsed);
    }
}

fn collect_mapping_body_metadata(
    body: &AstMappingBody,
    caller: Option<&str>,
    parsed: &mut ParsedFile,
) {
    for arm in &body.arms {
        if let Some(pattern) = &arm.0.lhs_pattern {
            collect_decl_names_from_pattern(pattern, DeclKind::Let, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(pattern, Scope::Local, parsed);
        }
        if let Some(pattern) = &arm.0.rhs_pattern {
            collect_decl_names_from_pattern(pattern, DeclKind::Let, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(pattern, Scope::Local, parsed);
        }
        collect_expr_metadata(&arm.0.lhs, caller, parsed);
        collect_expr_metadata(&arm.0.rhs, caller, parsed);
        if let Some(guard) = &arm.0.guard {
            collect_expr_metadata(guard, caller, parsed);
        }
    }
}

fn collect_callable_clause_metadata(def: &AstCallableDef, caller: &str, parsed: &mut ParsedFile) {
    if let Some(rec_measure) = &def.rec_measure {
        collect_decl_names_from_pattern(
            &rec_measure.0.pattern,
            DeclKind::Let,
            Scope::Local,
            parsed,
        );
        collect_typed_bindings_from_pattern(&rec_measure.0.pattern, Scope::Local, parsed);
        collect_expr_metadata(&rec_measure.0.body, Some(caller), parsed);
    }

    if def.clauses.is_empty() {
        for param in &def.params {
            collect_decl_names_from_pattern(param, DeclKind::Parameter, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(param, Scope::Local, parsed);
        }
        if let Some(body) = &def.body {
            collect_expr_metadata(body, Some(caller), parsed);
        }
        if let Some(mapping_body) = &def.mapping_body {
            collect_mapping_body_metadata(mapping_body, Some(caller), parsed);
        }
        return;
    }

    for clause in &def.clauses {
        for pattern in &clause.0.patterns {
            collect_decl_names_from_pattern(pattern, DeclKind::Parameter, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(pattern, Scope::Local, parsed);
        }
        if let Some(guard) = &clause.0.guard {
            collect_expr_metadata(guard, Some(caller), parsed);
        }
        if let Some(body) = &clause.0.body {
            collect_expr_metadata(body, Some(caller), parsed);
        }
        if let Some(mapping_body) = &clause.0.mapping_body {
            collect_mapping_body_metadata(mapping_body, Some(caller), parsed);
        }
    }
}

fn collect_termination_measure_metadata(kind: &AstTerminationMeasureKind, parsed: &mut ParsedFile) {
    match kind {
        AstTerminationMeasureKind::Function { pattern, body } => {
            collect_decl_names_from_pattern(pattern, DeclKind::Let, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(pattern, Scope::Local, parsed);
            collect_expr_metadata(body, None, parsed);
        }
        AstTerminationMeasureKind::Loop { measures } => {
            for measure in measures {
                match &measure.0 {
                    AstLoopMeasure::Until(expr)
                    | AstLoopMeasure::Repeat(expr)
                    | AstLoopMeasure::While(expr) => collect_expr_metadata(expr, None, parsed),
                }
            }
        }
    }
}

fn collect_expr_metadata(expr: &(AstExpr, Span), caller: Option<&str>, parsed: &mut ParsedFile) {
    match &expr.0 {
        AstExpr::Attribute { expr, .. } => collect_expr_metadata(expr, caller, parsed),
        AstExpr::Assign { lhs, rhs } | AstExpr::Infix { lhs, rhs, .. } => {
            collect_expr_metadata(lhs, caller, parsed);
            collect_expr_metadata(rhs, caller, parsed);
        }
        AstExpr::Let { binding, body } => {
            collect_decl_names_from_pattern(&binding.pattern, DeclKind::Let, Scope::Local, parsed);
            collect_typed_bindings_from_pattern(&binding.pattern, Scope::Local, parsed);
            collect_expr_metadata(&binding.value, caller, parsed);
            collect_expr_metadata(body, caller, parsed);
        }
        AstExpr::Var {
            target,
            value,
            body,
        } => {
            if let Some((name, span)) = var_target_name(target) {
                push_decl(
                    parsed,
                    DeclKind::Var,
                    DeclRole::Definition,
                    &name,
                    Scope::Local,
                    span,
                    false,
                );
            }
            if let Some((name, span, ty_span)) = typed_var_target(target) {
                parsed.typed_bindings.push(TypedBinding {
                    name,
                    name_span: span,
                    ty_span,
                    scope: Scope::Local,
                });
            }
            collect_expr_metadata(target, caller, parsed);
            collect_expr_metadata(value, caller, parsed);
            collect_expr_metadata(body, caller, parsed);
        }
        AstExpr::Block(items) => {
            for item in items {
                match &item.0 {
                    AstBlockItem::Let(binding) => {
                        collect_decl_names_from_pattern(
                            &binding.pattern,
                            DeclKind::Let,
                            Scope::Local,
                            parsed,
                        );
                        collect_typed_bindings_from_pattern(&binding.pattern, Scope::Local, parsed);
                        collect_expr_metadata(&binding.value, caller, parsed);
                    }
                    AstBlockItem::Var { target, value } => {
                        if let Some((name, span)) = var_target_name(target) {
                            push_decl(
                                parsed,
                                DeclKind::Var,
                                DeclRole::Definition,
                                &name,
                                Scope::Local,
                                span,
                                false,
                            );
                        }
                        if let Some((name, span, ty_span)) = typed_var_target(target) {
                            parsed.typed_bindings.push(TypedBinding {
                                name,
                                name_span: span,
                                ty_span,
                                scope: Scope::Local,
                            });
                        }
                        collect_expr_metadata(target, caller, parsed);
                        collect_expr_metadata(value, caller, parsed);
                    }
                    AstBlockItem::Expr(expr) => collect_expr_metadata(expr, caller, parsed),
                }
            }
        }
        AstExpr::Return(expr)
        | AstExpr::Throw(expr)
        | AstExpr::Exit(Some(expr))
        | AstExpr::Prefix { expr, .. }
        | AstExpr::Cast { expr, .. }
        | AstExpr::Field { expr, .. } => collect_expr_metadata(expr, caller, parsed),
        AstExpr::Assert { condition, message } => {
            collect_expr_metadata(condition, caller, parsed);
            if let Some(message) = message {
                collect_expr_metadata(message, caller, parsed);
            }
        }
        AstExpr::Exit(None) => {}
        AstExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_metadata(cond, caller, parsed);
            collect_expr_metadata(then_branch, caller, parsed);
            if let Some(else_branch) = else_branch {
                collect_expr_metadata(else_branch, caller, parsed);
            }
        }
        AstExpr::Match { scrutinee, cases } | AstExpr::Try { scrutinee, cases } => {
            collect_expr_metadata(scrutinee, caller, parsed);
            for case in cases {
                collect_case_metadata(case, caller, parsed);
            }
        }
        AstExpr::Foreach(AstForeachExpr { body, .. }) => {
            collect_expr_metadata(body, caller, parsed);
        }
        AstExpr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                collect_expr_metadata(measure, caller, parsed);
            }
            collect_expr_metadata(body, caller, parsed);
            collect_expr_metadata(until, caller, parsed);
        }
        AstExpr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                collect_expr_metadata(measure, caller, parsed);
            }
            collect_expr_metadata(cond, caller, parsed);
            collect_expr_metadata(body, caller, parsed);
        }
        AstExpr::Call(call) => collect_call_metadata(call, caller, parsed),
        AstExpr::Index { expr, index } => {
            collect_expr_metadata(expr, caller, parsed);
            collect_expr_metadata(index, caller, parsed);
        }
        AstExpr::Slice { expr, start, end } => {
            collect_expr_metadata(expr, caller, parsed);
            collect_expr_metadata(start, caller, parsed);
            collect_expr_metadata(end, caller, parsed);
        }
        AstExpr::VectorSlice {
            expr,
            start,
            length,
        } => {
            collect_expr_metadata(expr, caller, parsed);
            collect_expr_metadata(start, caller, parsed);
            collect_expr_metadata(length, caller, parsed);
        }
        AstExpr::Struct { fields, .. } => {
            for field in fields {
                collect_field_expr_metadata(field, caller, parsed);
            }
        }
        AstExpr::Update { base, fields } => {
            collect_expr_metadata(base, caller, parsed);
            for field in fields {
                collect_field_expr_metadata(field, caller, parsed);
            }
        }
        AstExpr::List(items) | AstExpr::Vector(items) | AstExpr::Tuple(items) => {
            for item in items {
                collect_expr_metadata(item, caller, parsed);
            }
        }
        AstExpr::VectorUpdate { base, updates } => {
            collect_expr_metadata(base, caller, parsed);
            for update in updates {
                collect_vector_update_metadata(update, caller, parsed);
            }
        }
        AstExpr::Literal(_)
        | AstExpr::Ident(_)
        | AstExpr::TypeVar(_)
        | AstExpr::Ref(_)
        | AstExpr::Config(_)
        | AstExpr::SizeOf(_)
        | AstExpr::Constraint(_)
        | AstExpr::Error(_) => {}
    }
}

fn decl_occurrence_kind(kind: DeclKind) -> Option<SymbolOccurrenceKind> {
    match kind {
        DeclKind::Function
        | DeclKind::Value
        | DeclKind::Mapping
        | DeclKind::Overload
        | DeclKind::Register
        | DeclKind::Parameter
        | DeclKind::EnumMember
        | DeclKind::Let
        | DeclKind::Var => Some(SymbolOccurrenceKind::Value),
        DeclKind::Type
        | DeclKind::Struct
        | DeclKind::Union
        | DeclKind::Bitfield
        | DeclKind::Enum
        | DeclKind::Newtype => Some(SymbolOccurrenceKind::Type),
    }
}

struct SymbolOccurrenceCollector {
    top_level_values: HashSet<String>,
    top_level_types: HashSet<String>,
    occurrences: Vec<SymbolOccurrence>,
    value_scopes: Vec<HashMap<String, Span>>,
    type_var_scopes: Vec<HashMap<String, Span>>,
}

impl SymbolOccurrenceCollector {
    fn new(parsed: &ParsedFile) -> Self {
        let mut top_level_values = HashSet::new();
        let mut top_level_types = HashSet::new();
        let mut occurrences = Vec::new();

        for decl in &parsed.decls {
            let Some(kind) = decl_occurrence_kind(decl.kind) else {
                continue;
            };
            if decl.scope == Scope::TopLevel {
                match kind {
                    SymbolOccurrenceKind::Value => {
                        top_level_values.insert(decl.name.clone());
                    }
                    SymbolOccurrenceKind::Type => {
                        top_level_types.insert(decl.name.clone());
                    }
                    SymbolOccurrenceKind::TypeVar => {}
                }
                occurrences.push(SymbolOccurrence {
                    name: decl.name.clone(),
                    kind,
                    span: decl.span,
                    scope: Some(Scope::TopLevel),
                    role: Some(decl.role),
                    target_span: None,
                });
            }
        }

        Self {
            top_level_values,
            top_level_types,
            occurrences,
            value_scopes: Vec::new(),
            type_var_scopes: Vec::new(),
        }
    }

    fn push_value_scope(&mut self) {
        self.value_scopes.push(HashMap::new());
    }

    fn pop_value_scope(&mut self) {
        self.value_scopes.pop();
    }

    fn push_type_var_scope(&mut self) {
        self.type_var_scopes.push(HashMap::new());
    }

    fn pop_type_var_scope(&mut self) {
        self.type_var_scopes.pop();
    }

    fn define_value(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.value_scopes.last_mut() {
            scope.insert(name.to_string(), span);
        }
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Value,
            span,
            scope: Some(Scope::Local),
            role: Some(DeclRole::Definition),
            target_span: Some(span),
        });
    }

    fn define_type_var(&mut self, name: &str, span: Span) {
        if let Some(scope) = self.type_var_scopes.last_mut() {
            scope.insert(name.to_string(), span);
        }
        self.occurrences.push(SymbolOccurrence {
            name: format!("'{name}"),
            kind: SymbolOccurrenceKind::TypeVar,
            span,
            scope: Some(Scope::Local),
            role: Some(DeclRole::Definition),
            target_span: Some(span),
        });
    }

    fn resolve_value(&self, name: &str) -> (Option<Scope>, Option<Span>) {
        for scope in self.value_scopes.iter().rev() {
            if let Some(span) = scope.get(name).copied() {
                return (Some(Scope::Local), Some(span));
            }
        }
        if self.top_level_values.contains(name) {
            (Some(Scope::TopLevel), None)
        } else {
            (None, None)
        }
    }

    fn resolve_type_var(&self, name: &str) -> Option<Span> {
        for scope in self.type_var_scopes.iter().rev() {
            if let Some(span) = scope.get(name).copied() {
                return Some(span);
            }
        }
        None
    }

    fn record_value_ref(&mut self, name: &str, span: Span) {
        let (scope, target_span) = self.resolve_value(name);
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Value,
            span,
            scope,
            role: None,
            target_span,
        });
    }

    fn record_type_ref(&mut self, name: &str, span: Span) {
        self.occurrences.push(SymbolOccurrence {
            name: name.to_string(),
            kind: SymbolOccurrenceKind::Type,
            span,
            scope: self
                .top_level_types
                .contains(name)
                .then_some(Scope::TopLevel),
            role: None,
            target_span: None,
        });
    }

    fn record_type_var_ref(&mut self, name: &str, span: Span) {
        let target_span = self.resolve_type_var(name);
        self.occurrences.push(SymbolOccurrence {
            name: format!("'{name}"),
            kind: SymbolOccurrenceKind::TypeVar,
            span,
            scope: target_span.map(|_| Scope::Local),
            role: None,
            target_span,
        });
    }
}

fn collect_pattern_symbol_occurrences(
    pattern: &(AstPattern, Span),
    collector: &mut SymbolOccurrenceCollector,
    bindings: &mut Vec<(String, Span)>,
) {
    match &pattern.0 {
        AstPattern::Attribute { pattern, .. } => {
            collect_pattern_symbol_occurrences(pattern, collector, bindings);
        }
        AstPattern::Wild | AstPattern::Literal(_) | AstPattern::Error(_) => {}
        AstPattern::Ident(name) => bindings.push((name.clone(), pattern.1)),
        AstPattern::TypeVar(name) => collector.record_type_var_ref(name, pattern.1),
        AstPattern::Typed(inner, ty) | AstPattern::AsType(inner, ty) => {
            collect_pattern_symbol_occurrences(inner, collector, bindings);
            collect_type_symbol_occurrences(ty, collector);
        }
        AstPattern::Tuple(items) | AstPattern::List(items) | AstPattern::Vector(items) => {
            for item in items {
                collect_pattern_symbol_occurrences(item, collector, bindings);
            }
        }
        AstPattern::App { callee, args } => {
            collector.record_value_ref(&callee.0, callee.1);
            for arg in args {
                collect_pattern_symbol_occurrences(arg, collector, bindings);
            }
        }
        AstPattern::Index { name, index } => {
            collector.record_value_ref(&name.0, name.1);
            collect_type_symbol_occurrences(index, collector);
        }
        AstPattern::RangeIndex { name, start, end } => {
            collector.record_value_ref(&name.0, name.1);
            collect_type_symbol_occurrences(start, collector);
            collect_type_symbol_occurrences(end, collector);
        }
        AstPattern::Struct { name, fields } => {
            if let Some(name) = name {
                collector.record_type_ref(&name.0, name.1);
            }
            for field in fields {
                match &field.0 {
                    AstFieldPattern::Binding { pattern, .. } => {
                        collect_pattern_symbol_occurrences(pattern, collector, bindings);
                    }
                    AstFieldPattern::Shorthand(name) => bindings.push((name.0.clone(), name.1)),
                    AstFieldPattern::Wild(_) => {}
                }
            }
        }
        AstPattern::Infix { lhs, op, rhs } => {
            collect_pattern_symbol_occurrences(lhs, collector, bindings);
            collector.record_value_ref(&op.0, op.1);
            collect_pattern_symbol_occurrences(rhs, collector, bindings);
        }
        AstPattern::AsBinding { pattern, binding } => {
            collect_pattern_symbol_occurrences(pattern, collector, bindings);
            bindings.push((binding.0.clone(), binding.1));
        }
    }
}

fn visit_var_target_symbol_occurrences(
    target: &(AstExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) -> Option<(String, Span)> {
    match &target.0 {
        AstExpr::Ident(name) => Some((name.clone(), target.1)),
        AstExpr::Cast { expr, ty } => {
            collect_type_symbol_occurrences(ty, collector);
            visit_var_target_symbol_occurrences(expr, collector)
        }
        _ => {
            collect_expr_symbol_occurrences(target, collector);
            None
        }
    }
}

fn collect_type_param_defs(
    params: &crate::ast::TypeParamSpec,
    collector: &mut SymbolOccurrenceCollector,
) {
    for param in &params.params {
        collector.define_type_var(&param.name.0, param.name.1);
    }
    if let Some(tail) = &params.tail {
        match tail {
            crate::ast::TypeParamTail::Type(ty) | crate::ast::TypeParamTail::Constraint(ty) => {
                collect_type_symbol_occurrences(ty, collector);
            }
        }
    }
}

fn collect_type_symbol_occurrences(
    ty: &(AstTypeExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &ty.0 {
        AstTypeExpr::Wild
        | AstTypeExpr::Literal(_)
        | AstTypeExpr::Dec
        | AstTypeExpr::Inc
        | AstTypeExpr::Config(_)
        | AstTypeExpr::Error(_) => {}
        AstTypeExpr::Named(name) => collector.record_type_ref(name, ty.1),
        AstTypeExpr::TypeVar(name) => collector.record_type_var_ref(name, ty.1),
        AstTypeExpr::Forall {
            vars,
            constraint,
            body,
        } => {
            collector.push_type_var_scope();
            for var in vars {
                collector.define_type_var(&var.name.0, var.name.1);
            }
            if let Some(constraint) = constraint {
                collect_type_symbol_occurrences(constraint, collector);
            }
            collect_type_symbol_occurrences(body, collector);
            collector.pop_type_var_scope();
        }
        AstTypeExpr::Existential {
            binder,
            constraint,
            body,
        } => {
            collector.push_type_var_scope();
            collector.define_type_var(&binder.name.0, binder.name.1);
            if let Some(constraint) = constraint {
                collect_type_symbol_occurrences(constraint, collector);
            }
            collect_type_symbol_occurrences(body, collector);
            collector.pop_type_var_scope();
        }
        AstTypeExpr::Effect { ty, .. } => collect_type_symbol_occurrences(ty, collector),
        AstTypeExpr::App { callee, args } => {
            collector.record_type_ref(&callee.0, callee.1);
            for arg in args {
                collect_type_symbol_occurrences(arg, collector);
            }
        }
        AstTypeExpr::Tuple(items) | AstTypeExpr::Set(items) => {
            for item in items {
                collect_type_symbol_occurrences(item, collector);
            }
        }
        AstTypeExpr::Register(inner) | AstTypeExpr::Prefix { ty: inner, .. } => {
            collect_type_symbol_occurrences(inner, collector);
        }
        AstTypeExpr::Infix { lhs, op, rhs } => {
            collect_type_symbol_occurrences(lhs, collector);
            collector.record_type_ref(&op.0, op.1);
            collect_type_symbol_occurrences(rhs, collector);
        }
        AstTypeExpr::Conditional {
            cond,
            then_ty,
            else_ty,
        } => {
            collect_type_symbol_occurrences(cond, collector);
            collect_type_symbol_occurrences(then_ty, collector);
            collect_type_symbol_occurrences(else_ty, collector);
        }
        AstTypeExpr::Arrow { params, ret, .. } => {
            for param in params {
                collect_type_symbol_occurrences(param, collector);
            }
            collect_type_symbol_occurrences(ret, collector);
        }
    }
}

fn collect_field_expr_symbol_occurrences(
    field: &(AstFieldExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &field.0 {
        AstFieldExpr::Assignment { target, value } => {
            collect_expr_symbol_occurrences(target, collector);
            collect_expr_symbol_occurrences(value, collector);
        }
        AstFieldExpr::Shorthand(name) => collector.record_value_ref(&name.0, name.1),
    }
}

fn collect_vector_update_symbol_occurrences(
    update: &(AstVectorUpdate, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &update.0 {
        AstVectorUpdate::Assignment { target, value } => {
            collect_expr_symbol_occurrences(target, collector);
            collect_expr_symbol_occurrences(value, collector);
        }
        AstVectorUpdate::RangeAssignment { start, end, value } => {
            collect_expr_symbol_occurrences(start, collector);
            collect_expr_symbol_occurrences(end, collector);
            collect_expr_symbol_occurrences(value, collector);
        }
        AstVectorUpdate::Shorthand(name) => collector.record_value_ref(&name.0, name.1),
    }
}

fn collect_case_symbol_occurrences(
    case: &(AstMatchCase, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    let mut bindings = Vec::new();
    collect_pattern_symbol_occurrences(&case.0.pattern, collector, &mut bindings);
    collector.push_value_scope();
    for (name, span) in bindings {
        collector.define_value(&name, span);
    }
    if let Some(guard) = &case.0.guard {
        collect_expr_symbol_occurrences(guard, collector);
    }
    collect_expr_symbol_occurrences(&case.0.body, collector);
    collector.pop_value_scope();
}

fn collect_mapping_body_symbol_occurrences(
    body: &AstMappingBody,
    collector: &mut SymbolOccurrenceCollector,
) {
    for arm in &body.arms {
        let mut bindings = Vec::new();
        if let Some(pattern) = &arm.0.lhs_pattern {
            collect_pattern_symbol_occurrences(pattern, collector, &mut bindings);
        }
        if let Some(pattern) = &arm.0.rhs_pattern {
            collect_pattern_symbol_occurrences(pattern, collector, &mut bindings);
        }
        collector.push_value_scope();
        for (name, span) in bindings {
            collector.define_value(&name, span);
        }
        if let Some(guard) = &arm.0.guard {
            collect_expr_symbol_occurrences(guard, collector);
        }
        collect_expr_symbol_occurrences(&arm.0.lhs, collector);
        collect_expr_symbol_occurrences(&arm.0.rhs, collector);
        collector.pop_value_scope();
    }
}

fn collect_expr_symbol_occurrences(
    expr: &(AstExpr, Span),
    collector: &mut SymbolOccurrenceCollector,
) {
    match &expr.0 {
        AstExpr::Attribute { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Assign { lhs, rhs } => {
            collect_expr_symbol_occurrences(lhs, collector);
            collect_expr_symbol_occurrences(rhs, collector);
        }
        AstExpr::Let { binding, body } => {
            collect_expr_symbol_occurrences(&binding.value, collector);
            let mut bindings = Vec::new();
            collect_pattern_symbol_occurrences(&binding.pattern, collector, &mut bindings);
            collector.push_value_scope();
            for (name, span) in bindings {
                collector.define_value(&name, span);
            }
            collect_expr_symbol_occurrences(body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Var {
            target,
            value,
            body,
        } => {
            let binding = visit_var_target_symbol_occurrences(target, collector);
            collect_expr_symbol_occurrences(value, collector);
            collector.push_value_scope();
            if let Some((name, span)) = binding {
                collector.define_value(&name, span);
            }
            collect_expr_symbol_occurrences(body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Block(items) => {
            collector.push_value_scope();
            for item in items {
                match &item.0 {
                    AstBlockItem::Let(binding) => {
                        collect_expr_symbol_occurrences(&binding.value, collector);
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(
                            &binding.pattern,
                            collector,
                            &mut bindings,
                        );
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    AstBlockItem::Var { target, value } => {
                        let binding = visit_var_target_symbol_occurrences(target, collector);
                        collect_expr_symbol_occurrences(value, collector);
                        if let Some((name, span)) = binding {
                            collector.define_value(&name, span);
                        }
                    }
                    AstBlockItem::Expr(expr) => collect_expr_symbol_occurrences(expr, collector),
                }
            }
            collector.pop_value_scope();
        }
        AstExpr::Return(expr)
        | AstExpr::Throw(expr)
        | AstExpr::Exit(Some(expr))
        | AstExpr::Prefix { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Assert { condition, message } => {
            collect_expr_symbol_occurrences(condition, collector);
            if let Some(message) = message {
                collect_expr_symbol_occurrences(message, collector);
            }
        }
        AstExpr::Cast { expr, ty } => {
            collect_expr_symbol_occurrences(expr, collector);
            collect_type_symbol_occurrences(ty, collector);
        }
        AstExpr::Field { expr, .. } => collect_expr_symbol_occurrences(expr, collector),
        AstExpr::Exit(None) => {}
        AstExpr::If {
            cond,
            then_branch,
            else_branch,
        } => {
            collect_expr_symbol_occurrences(cond, collector);
            collect_expr_symbol_occurrences(then_branch, collector);
            if let Some(else_branch) = else_branch {
                collect_expr_symbol_occurrences(else_branch, collector);
            }
        }
        AstExpr::Match { scrutinee, cases } | AstExpr::Try { scrutinee, cases } => {
            collect_expr_symbol_occurrences(scrutinee, collector);
            for case in cases {
                collect_case_symbol_occurrences(case, collector);
            }
        }
        AstExpr::Foreach(foreach) => {
            collect_expr_symbol_occurrences(&foreach.start, collector);
            collect_expr_symbol_occurrences(&foreach.end, collector);
            if let Some(step) = &foreach.step {
                collect_expr_symbol_occurrences(step, collector);
            }
            if let Some(ty) = &foreach.ty {
                collect_type_symbol_occurrences(ty, collector);
            }
            collector.push_value_scope();
            collector.define_value(&foreach.iterator.0, foreach.iterator.1);
            collect_expr_symbol_occurrences(&foreach.body, collector);
            collector.pop_value_scope();
        }
        AstExpr::Repeat {
            measure,
            body,
            until,
        } => {
            if let Some(measure) = measure {
                collect_expr_symbol_occurrences(measure, collector);
            }
            collect_expr_symbol_occurrences(body, collector);
            collect_expr_symbol_occurrences(until, collector);
        }
        AstExpr::While {
            measure,
            cond,
            body,
        } => {
            if let Some(measure) = measure {
                collect_expr_symbol_occurrences(measure, collector);
            }
            collect_expr_symbol_occurrences(cond, collector);
            collect_expr_symbol_occurrences(body, collector);
        }
        AstExpr::Infix { lhs, op, rhs } => {
            collect_expr_symbol_occurrences(lhs, collector);
            collector.record_value_ref(&op.0, op.1);
            collect_expr_symbol_occurrences(rhs, collector);
        }
        AstExpr::Config(_) | AstExpr::Literal(_) | AstExpr::Error(_) => {}
        AstExpr::Ident(name) => collector.record_value_ref(name, expr.1),
        AstExpr::TypeVar(name) => collector.record_type_var_ref(name, expr.1),
        AstExpr::Ref(name) => collector.record_value_ref(&name.0, name.1),
        AstExpr::Call(call) => {
            collect_expr_symbol_occurrences(&call.callee, collector);
            for arg in &call.args {
                collect_expr_symbol_occurrences(arg, collector);
            }
        }
        AstExpr::SizeOf(ty) | AstExpr::Constraint(ty) => {
            collect_type_symbol_occurrences(ty, collector);
        }
        AstExpr::Index { expr, index } => {
            collect_expr_symbol_occurrences(expr, collector);
            collect_expr_symbol_occurrences(index, collector);
        }
        AstExpr::Slice { expr, start, end } => {
            collect_expr_symbol_occurrences(expr, collector);
            collect_expr_symbol_occurrences(start, collector);
            collect_expr_symbol_occurrences(end, collector);
        }
        AstExpr::VectorSlice {
            expr,
            start,
            length,
        } => {
            collect_expr_symbol_occurrences(expr, collector);
            collect_expr_symbol_occurrences(start, collector);
            collect_expr_symbol_occurrences(length, collector);
        }
        AstExpr::Struct { name, fields } => {
            if let Some(name) = name {
                collector.record_type_ref(&name.0, name.1);
            }
            for field in fields {
                collect_field_expr_symbol_occurrences(field, collector);
            }
        }
        AstExpr::Update { base, fields } => {
            collect_expr_symbol_occurrences(base, collector);
            for field in fields {
                collect_field_expr_symbol_occurrences(field, collector);
            }
        }
        AstExpr::List(items) | AstExpr::Vector(items) | AstExpr::Tuple(items) => {
            for item in items {
                collect_expr_symbol_occurrences(item, collector);
            }
        }
        AstExpr::VectorUpdate { base, updates } => {
            collect_expr_symbol_occurrences(base, collector);
            for update in updates {
                collect_vector_update_symbol_occurrences(update, collector);
            }
        }
    }
}

fn collect_named_detail_symbol_occurrences(
    detail: &crate::ast::NamedDefDetail,
    collector: &mut SymbolOccurrenceCollector,
) {
    match detail {
        crate::ast::NamedDefDetail::Enum { members, functions } => {
            for member in members {
                if let Some(value) = &member.0.value {
                    collect_expr_symbol_occurrences(value, collector);
                }
            }
            for function in functions {
                collect_type_symbol_occurrences(&function.0.ty, collector);
            }
        }
        crate::ast::NamedDefDetail::Struct { fields } => {
            for field in fields {
                collect_type_symbol_occurrences(&field.0.ty, collector);
            }
        }
        crate::ast::NamedDefDetail::Union { variants } => {
            for variant in variants {
                match &variant.0.payload {
                    crate::ast::UnionPayload::Type(ty) => {
                        collect_type_symbol_occurrences(ty, collector)
                    }
                    crate::ast::UnionPayload::Struct { fields } => {
                        for field in fields {
                            collect_type_symbol_occurrences(&field.0.ty, collector);
                        }
                    }
                }
            }
        }
        crate::ast::NamedDefDetail::Bitfield { fields } => {
            for field in fields {
                collect_type_symbol_occurrences(&field.0.high, collector);
                if let Some(low) = &field.0.low {
                    collect_type_symbol_occurrences(low, collector);
                }
            }
        }
    }
}

fn collect_symbol_occurrences_from_ast(
    ast: &SourceFile,
    parsed: &ParsedFile,
) -> Vec<SymbolOccurrence> {
    let mut collector = SymbolOccurrenceCollector::new(parsed);

    for (item, _) in &ast.items {
        match item {
            TopLevelDef::Scattered(def) => {
                if let Some(params) = &def.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                    if let Some(signature) = &def.signature {
                        collect_type_symbol_occurrences(signature, &mut collector);
                    }
                    collector.pop_type_var_scope();
                } else if let Some(signature) = &def.signature {
                    collect_type_symbol_occurrences(signature, &mut collector);
                }
            }
            TopLevelDef::ScatteredClause(def) => {
                if let Some(ty) = &def.ty {
                    collect_type_symbol_occurrences(ty, &mut collector);
                }
            }
            TopLevelDef::CallableSpec(spec) => {
                collect_type_symbol_occurrences(&spec.signature, &mut collector);
            }
            TopLevelDef::CallableDef(def) => {
                if let Some(rec_measure) = &def.rec_measure {
                    let mut bindings = Vec::new();
                    collect_pattern_symbol_occurrences(
                        &rec_measure.0.pattern,
                        &mut collector,
                        &mut bindings,
                    );
                    collector.push_value_scope();
                    for (name, span) in bindings {
                        collector.define_value(&name, span);
                    }
                    collect_expr_symbol_occurrences(&rec_measure.0.body, &mut collector);
                    collector.pop_value_scope();
                }

                if def.clauses.is_empty() {
                    collector.push_value_scope();
                    if let Some(signature) = &def.signature {
                        collect_type_symbol_occurrences(signature, &mut collector);
                    }
                    for param in &def.params {
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(param, &mut collector, &mut bindings);
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    if let Some(return_ty) = &def.return_ty {
                        collect_type_symbol_occurrences(return_ty, &mut collector);
                    }
                    if let Some(body) = &def.body {
                        collect_expr_symbol_occurrences(body, &mut collector);
                    }
                    if let Some(mapping_body) = &def.mapping_body {
                        collect_mapping_body_symbol_occurrences(mapping_body, &mut collector);
                    }
                    collector.pop_value_scope();
                    continue;
                }

                for clause in &def.clauses {
                    collector.push_value_scope();
                    let mut pushed_type_scope = false;
                    if let Some(quantifier) = &clause.0.quantifier {
                        collector.push_type_var_scope();
                        pushed_type_scope = true;
                        for var in &quantifier.vars {
                            collector.define_type_var(&var.name.0, var.name.1);
                        }
                        if let Some(constraint) = &quantifier.constraint {
                            collect_type_symbol_occurrences(constraint, &mut collector);
                        }
                    }
                    for pattern in &clause.0.patterns {
                        let mut bindings = Vec::new();
                        collect_pattern_symbol_occurrences(pattern, &mut collector, &mut bindings);
                        for (name, span) in bindings {
                            collector.define_value(&name, span);
                        }
                    }
                    if let Some(return_ty) = &clause.0.return_ty {
                        collect_type_symbol_occurrences(return_ty, &mut collector);
                    }
                    if let Some(guard) = &clause.0.guard {
                        collect_expr_symbol_occurrences(guard, &mut collector);
                    }
                    if let Some(body) = &clause.0.body {
                        collect_expr_symbol_occurrences(body, &mut collector);
                    }
                    if let Some(mapping_body) = &clause.0.mapping_body {
                        collect_mapping_body_symbol_occurrences(mapping_body, &mut collector);
                    }
                    if pushed_type_scope {
                        collector.pop_type_var_scope();
                    }
                    collector.pop_value_scope();
                }
            }
            TopLevelDef::TypeAlias(alias) => {
                if let Some(params) = &alias.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                    if let Some(target) = &alias.target {
                        collect_type_symbol_occurrences(target, &mut collector);
                    }
                    collector.pop_type_var_scope();
                } else if let Some(target) = &alias.target {
                    collect_type_symbol_occurrences(target, &mut collector);
                }
            }
            TopLevelDef::Named(def) => {
                let has_type_params = def.params.is_some();
                if let Some(params) = &def.params {
                    collector.push_type_var_scope();
                    collect_type_param_defs(&params.0, &mut collector);
                }
                if let Some(ty) = &def.ty {
                    collect_type_symbol_occurrences(ty, &mut collector);
                }
                if let Some(detail) = &def.detail {
                    collect_named_detail_symbol_occurrences(detail, &mut collector);
                }
                if matches!(def.kind, NamedDefKind::Overload) {
                    for member in &def.members {
                        collector.record_value_ref(&member.0, member.1);
                    }
                }
                if let Some(value) = &def.value {
                    collect_expr_symbol_occurrences(value, &mut collector);
                }
                if has_type_params {
                    collector.pop_type_var_scope();
                }
            }
            TopLevelDef::Default(_)
            | TopLevelDef::Fixity(_)
            | TopLevelDef::Instantiation(_)
            | TopLevelDef::Directive(_)
            | TopLevelDef::End(_) => {}
            TopLevelDef::Constraint(def) => {
                collect_type_symbol_occurrences(&def.ty, &mut collector);
            }
            TopLevelDef::TerminationMeasure(def) => match &def.kind {
                AstTerminationMeasureKind::Function { pattern, body } => {
                    let mut bindings = Vec::new();
                    collect_pattern_symbol_occurrences(pattern, &mut collector, &mut bindings);
                    collector.push_value_scope();
                    for (name, span) in bindings {
                        collector.define_value(&name, span);
                    }
                    collect_expr_symbol_occurrences(body, &mut collector);
                    collector.pop_value_scope();
                }
                AstTerminationMeasureKind::Loop { measures } => {
                    for measure in measures {
                        match &measure.0 {
                            AstLoopMeasure::Until(expr)
                            | AstLoopMeasure::Repeat(expr)
                            | AstLoopMeasure::While(expr) => {
                                collect_expr_symbol_occurrences(expr, &mut collector)
                            }
                        }
                    }
                }
            },
        }
    }

    collector.occurrences
}

fn ast_alias_target_name(alias: &AstTypeAliasDef) -> Option<String> {
    match &unwrapped_type_expr(alias.target.as_ref()?).0 {
        AstTypeExpr::Named(name) => Some(name.clone()),
        AstTypeExpr::App { callee, .. } => Some(callee.0.clone()),
        _ => None,
    }
}

impl ParsedFile {
    pub fn from_ast(ast: &SourceFile) -> Self {
        let mut parsed = ParsedFile::default();

        for (item, _) in &ast.items {
            match item {
                TopLevelDef::Scattered(AstScatteredDef { kind, name, .. }) => {
                    push_decl(
                        &mut parsed,
                        decl_kind_for_scattered(*kind),
                        DeclRole::Declaration,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        true,
                    );
                }
                TopLevelDef::ScatteredClause(AstScatteredClauseDef {
                    kind: ScatteredClauseKind::Enum,
                    member,
                    ..
                }) => {
                    push_decl(
                        &mut parsed,
                        DeclKind::EnumMember,
                        DeclRole::Definition,
                        &member.0,
                        Scope::TopLevel,
                        member.1,
                        true,
                    );
                }
                TopLevelDef::ScatteredClause(_) => {}
                TopLevelDef::CallableSpec(spec) => {
                    let kind = match spec.kind {
                        CallableSpecKind::Value => DeclKind::Value,
                        CallableSpecKind::Mapping => DeclKind::Mapping,
                    };
                    push_decl(
                        &mut parsed,
                        kind,
                        DeclRole::Declaration,
                        &spec.name.0,
                        Scope::TopLevel,
                        spec.name.1,
                        false,
                    );
                    parsed.callable_heads.push(callable_head_from_spec(spec));
                }
                TopLevelDef::CallableDef(def) => {
                    let AstCallableDef {
                        kind,
                        name,
                        signature,
                        params,
                        return_ty,
                        ..
                    } = def;
                    let (decl_kind, is_scattered) = match kind {
                        CallableDefKind::Function => (DeclKind::Function, false),
                        CallableDefKind::FunctionClause => (DeclKind::Function, true),
                        CallableDefKind::Mapping => (DeclKind::Mapping, false),
                        CallableDefKind::MappingClause => (DeclKind::Mapping, true),
                    };
                    push_decl(
                        &mut parsed,
                        decl_kind,
                        DeclRole::Definition,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        is_scattered,
                    );
                    parsed.callable_heads.push(CallableHead {
                        name: name.0.clone(),
                        kind: decl_kind,
                        name_span: name.1,
                        label_span: return_ty
                            .as_ref()
                            .map_or(name.1, |ty| Span::new(name.1.start, ty.1.end)),
                        params: params.iter().map(callable_param_from_pattern).collect(),
                        return_type_span: return_ty.as_ref().map(|ty| ty.1),
                    });
                    if matches!(decl_kind, DeclKind::Mapping) {
                        if let Some(signature) = signature {
                            parsed.callable_heads.pop();
                            parsed.callable_heads.push(callable_head_from_signature(
                                &name.0, decl_kind, name.1, signature,
                            ));
                        }
                    }
                    collect_callable_clause_metadata(def, &name.0, &mut parsed);
                }
                TopLevelDef::TypeAlias(alias) => {
                    push_decl(
                        &mut parsed,
                        DeclKind::Type,
                        DeclRole::Definition,
                        &alias.name.0,
                        Scope::TopLevel,
                        alias.name.1,
                        false,
                    );
                    if let Some(target) = ast_alias_target_name(alias) {
                        parsed.type_aliases.push(TypeAlias {
                            sub: alias.name.0.clone(),
                            sup: target,
                            span: alias.name.1,
                        });
                    }
                }
                TopLevelDef::Named(AstNamedDef {
                    kind,
                    name,
                    members,
                    ty,
                    value,
                    ..
                }) => {
                    let decl_kind = decl_kind_for_named(*kind);
                    push_decl(
                        &mut parsed,
                        decl_kind,
                        DeclRole::Definition,
                        &name.0,
                        Scope::TopLevel,
                        name.1,
                        false,
                    );
                    if *kind == NamedDefKind::Enum {
                        for member in members {
                            push_decl(
                                &mut parsed,
                                DeclKind::EnumMember,
                                DeclRole::Definition,
                                &member.0,
                                Scope::TopLevel,
                                member.1,
                                false,
                            );
                        }
                    }
                    if matches!(kind, NamedDefKind::Let | NamedDefKind::Var) {
                        if let Some(ty) = ty {
                            parsed.typed_bindings.push(TypedBinding {
                                name: name.0.clone(),
                                name_span: name.1,
                                ty_span: ty.1,
                                scope: Scope::TopLevel,
                            });
                        }
                    }
                    if let Some(value) = value {
                        collect_expr_metadata(value, None, &mut parsed);
                    }
                }
                TopLevelDef::Default(_)
                | TopLevelDef::Fixity(_)
                | TopLevelDef::Instantiation(_)
                | TopLevelDef::Directive(_)
                | TopLevelDef::End(_)
                | TopLevelDef::Constraint(_) => {}
                TopLevelDef::TerminationMeasure(def) => {
                    collect_termination_measure_metadata(&def.kind, &mut parsed);
                }
            }
        }

        parsed.symbol_occurrences = collect_symbol_occurrences_from_ast(ast, &parsed);
        parsed
    }
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

    #[test]
    fn derives_top_level_metadata_from_ast() {
        let source = r#"
val foo : (int, int) -> int
function foo(x : bits(32), y : int) -> int = x
type child = parent
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo"
                && d.kind == DeclKind::Value
                && d.role == DeclRole::Declaration
                && d.scope == Scope::TopLevel
        }));
        assert!(parsed.decls.iter().any(|d| {
            d.name == "foo"
                && d.kind == DeclKind::Function
                && d.role == DeclRole::Definition
                && d.scope == Scope::TopLevel
        }));
        assert!(parsed
            .type_aliases
            .iter()
            .any(|alias| alias.sub == "child" && alias.sup == "parent"));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "x"));
        assert!(parsed
            .callable_heads
            .iter()
            .any(|head| head.name == "foo" && head.kind == DeclKind::Function));
    }

    #[test]
    fn derives_callable_heads_from_quantified_effectful_specs() {
        let source = r#"
val write_ram : forall 'n, 0 < 'n <= max_mem_access . (write_kind, xlenbits, atom('n), bits(8 * 'n), mem_meta) -> bool effect {wmv, wmvt}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let head = parsed
            .callable_heads
            .iter()
            .find(|head| head.name == "write_ram")
            .expect("callable head");
        assert_eq!(head.kind, DeclKind::Value);
        assert_eq!(head.params.len(), 5, "{head:#?}");
        assert!(head.return_type_span.is_some());
    }

    #[test]
    fn derives_calls_from_termination_measure_defs() {
        let source = r#"
termination_measure helper x = call(x)
termination_measure loop until done(x), while guard(x)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.call_sites.iter().any(|call| call.callee == "call"));
        assert!(parsed.call_sites.iter().any(|call| call.callee == "done"));
        assert!(parsed.call_sites.iter().any(|call| call.callee == "guard"));
    }

    #[test]
    fn derives_enum_members_from_ast() {
        let source = r#"
enum color = Red | Green
scattered enum extension
enum clause extension = Ext_M
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "Red" && decl.kind == DeclKind::EnumMember));
        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "Green" && decl.kind == DeclKind::EnumMember));
        assert!(parsed.decls.iter().any(|decl| decl.name == "Ext_M"
            && decl.kind == DeclKind::EnumMember
            && decl.is_scattered));
    }

    #[test]
    fn derives_as_pattern_bindings_from_ast() {
        let source = r#"
function match_cons_as(ys) = match ys {
  x :: xs as zs => zs,
  [||] => ys
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|decl| decl.name == "x"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|decl| decl.name == "xs"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
        assert!(parsed.decls.iter().any(|decl| decl.name == "zs"
            && decl.kind == DeclKind::Let
            && decl.scope == Scope::Local));
    }

    #[test]
    fn derives_body_metadata_from_ast() {
        let source = r#"
function foo(x : int) = {
  let y : bits(8) = bar(x);
  var z = baz(y);
  match z {
    Some(v) => qux(v),
    None() => y
  }
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.decls.iter().any(|decl| {
            decl.name == "y"
                && decl.kind == DeclKind::Let
                && decl.scope == Scope::Local
                && decl.role == DeclRole::Definition
        }));
        assert!(parsed.decls.iter().any(|decl| {
            decl.name == "z"
                && decl.kind == DeclKind::Var
                && decl.scope == Scope::Local
                && decl.role == DeclRole::Definition
        }));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "x" && binding.scope == Scope::Local));
        assert!(parsed
            .typed_bindings
            .iter()
            .any(|binding| binding.name == "y" && binding.scope == Scope::Local));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "bar" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "baz" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.as_deref() == Some("foo") && call.callee == "qux" }));
    }

    #[test]
    fn derives_top_level_initializer_calls_from_ast() {
        let source = r#"
register r : int = mk_reg(init())
let result : int = wrap(read_reg(r))
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "mk_reg" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "init" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "wrap" }));
        assert!(parsed
            .call_sites
            .iter()
            .any(|call| { call.caller.is_none() && call.callee == "read_reg" }));
    }

    #[test]
    fn derives_mapping_body_calls_from_ast() {
        let source = r#"mapping clause assembly = use_bits(0x12) <-> "ok""#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        assert!(parsed.call_sites.iter().any(|call| {
            call.caller.as_deref() == Some("assembly") && call.callee == "use_bits"
        }));
    }

    #[test]
    fn derives_mapping_arm_calls_from_structured_ast() {
        let source = r#"
mapping encdec_reg : regidx <-> bits(5) = {
  forwards Regidx(r) => zero_extend(r),
  backwards r if allow_reg(r) => decode_reg(r)
}
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let callees = parsed
            .call_sites
            .iter()
            .filter(|call| call.caller.as_deref() == Some("encdec_reg"))
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();
        assert!(callees.contains(&"zero_extend"));
        assert!(callees.contains(&"allow_reg"));
        assert!(callees.contains(&"decode_reg"));
    }

    #[test]
    fn derives_metadata_from_function_and_clauses_and_rec_measures() {
        let source = r#"
function { xs => dec(xs) } foo forall 'n. (x if guard(x)) -> bits('n) = body(x)
and foo y = other(y)
"#;
        let tokens = crate::lexer().parse(source).into_result().unwrap();
        let ast = crate::parse_source(&tokens).into_result().unwrap();
        let parsed = ParsedFile::from_ast(&ast);

        let callees = parsed
            .call_sites
            .iter()
            .map(|call| call.callee.as_str())
            .collect::<Vec<_>>();
        assert!(callees.contains(&"dec"));
        assert!(callees.contains(&"guard"));
        assert!(callees.contains(&"body"));
        assert!(callees.contains(&"other"));
        assert!(parsed
            .decls
            .iter()
            .any(|decl| decl.name == "x" && decl.kind == DeclKind::Parameter));
    }
}
